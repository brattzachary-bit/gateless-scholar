//! API-key providers: Anthropic Messages API, Google Gemini (`generateContent` /
//! `streamGenerateContent`), and any OpenAI-compatible `/chat/completions` endpoint.
//! All three stream over SSE when the caller passes a `deltas` channel.

use futures::future::BoxFuture;
use serde_json::{Value, json};
use tokio::sync::mpsc::UnboundedSender;

use super::stream::{check_status, for_each_line, http_client, sse_data, strip_think};
use super::{CompletionRequest, ModelProvider, ProviderConfig, ProviderError, ProviderKind};

const JSON_HINT: &str = "\n\nReply with a single valid JSON object only: no prose, no code fences.";

pub struct HttpProvider {
    pub config: ProviderConfig,
    api_key: String,
    client: reqwest::Client,
}

impl HttpProvider {
    pub fn new(config: ProviderConfig, api_key: String) -> Self {
        HttpProvider { config, api_key, client: http_client() }
    }

    fn base_url(&self) -> String {
        let base = self
            .config
            .base_url
            .clone()
            .filter(|s| !s.trim().is_empty())
            .or_else(|| self.config.kind.default_base_url().map(String::from))
            .unwrap_or_default();
        base.trim_end_matches('/').to_string()
    }

    fn model(&self) -> String {
        if self.config.model.trim().is_empty() {
            self.config.kind.default_model().to_string()
        } else {
            self.config.model.trim().to_string()
        }
    }

    fn max_tokens(req: &CompletionRequest) -> u32 {
        if req.max_tokens == 0 { 4096 } else { req.max_tokens }
    }

    // ----------------------------------------------------------------------------------------
    // Anthropic
    // ----------------------------------------------------------------------------------------

    async fn anthropic(&self, req: &CompletionRequest, deltas: Option<UnboundedSender<String>>) -> anyhow::Result<String> {
        let url = format!("{}/v1/messages", self.base_url());
        let mut system = req.system.clone();
        if req.json {
            system.push_str(JSON_HINT);
        }
        let stream = deltas.is_some();
        let mut body = json!({
            "model": self.model(),
            "max_tokens": Self::max_tokens(req),
            "messages": [{"role": "user", "content": req.prompt}],
            "stream": stream,
        });
        if !system.trim().is_empty() {
            body["system"] = json!(system);
        }
        if req.temperature > 0.0 {
            body["temperature"] = json!(req.temperature);
        }
        let send = |body: Value| {
            self.client
                .post(&url)
                .header("x-api-key", &self.api_key)
                .header("anthropic-version", "2023-06-01")
                .json(&body)
                .send()
        };
        let mut resp = send(body.clone()).await.map_err(|e| ProviderError::Http(e.to_string()))?;
        if resp.status().as_u16() == 400 && body.get("temperature").is_some() {
            // Some newer models reject sampling parameters: retry once without.
            let text = resp.text().await.unwrap_or_default();
            if text.contains("temperature") {
                body.as_object_mut().map(|o| o.remove("temperature"));
                resp = send(body).await.map_err(|e| ProviderError::Http(e.to_string()))?;
            } else {
                return Err(ProviderError::Http(format!("400: {}", super::stream::truncate(&text, 600))).into());
            }
        }
        let resp = check_status(resp).await?;
        if let Some(tx) = deltas {
            let mut out = String::new();
            let mut err: Option<String> = None;
            for_each_line(resp, |line| {
                let Some(data) = sse_data(line) else { return Ok(true) };
                let Ok(v) = serde_json::from_str::<Value>(data) else { return Ok(true) };
                match v["type"].as_str() {
                    Some("content_block_delta") => {
                        if let Some(t) = v["delta"]["text"].as_str() {
                            out.push_str(t);
                            let _ = tx.send(t.to_string());
                        }
                    }
                    Some("error") => {
                        err = Some(v["error"]["message"].as_str().unwrap_or("stream error").to_string());
                        return Ok(false);
                    }
                    Some("message_stop") => return Ok(false),
                    _ => {}
                }
                Ok(true)
            })
            .await?;
            if let Some(e) = err {
                return Err(ProviderError::Http(e).into());
            }
            Ok(out)
        } else {
            let v: Value = resp.json().await.map_err(|e| ProviderError::Parse(e.to_string()))?;
            let text: String = v["content"]
                .as_array()
                .map(|blocks| blocks.iter().filter_map(|b| b["text"].as_str()).collect::<Vec<_>>().join(""))
                .unwrap_or_default();
            if text.is_empty() && v["content"].is_null() {
                return Err(ProviderError::Parse(format!("unexpected reply: {}", super::stream::truncate(&v.to_string(), 300))).into());
            }
            Ok(text)
        }
    }

    // ----------------------------------------------------------------------------------------
    // Google Gemini
    // ----------------------------------------------------------------------------------------

    async fn gemini(&self, req: &CompletionRequest, deltas: Option<UnboundedSender<String>>) -> anyhow::Result<String> {
        let model = self.model();
        let model = model.strip_prefix("models/").unwrap_or(&model).to_string();
        let stream = deltas.is_some();
        let url = if stream {
            format!("{}/v1beta/models/{}:streamGenerateContent?alt=sse", self.base_url(), model)
        } else {
            format!("{}/v1beta/models/{}:generateContent", self.base_url(), model)
        };
        let mut gen_config = json!({
            "maxOutputTokens": Self::max_tokens(req),
            "temperature": req.temperature,
        });
        if req.json {
            gen_config["responseMimeType"] = json!("application/json");
        }
        let mut body = json!({
            "contents": [{"role": "user", "parts": [{"text": req.prompt}]}],
            "generationConfig": gen_config,
        });
        if !req.system.trim().is_empty() {
            body["systemInstruction"] = json!({"parts": [{"text": req.system}]});
        }
        let resp = self
            .client
            .post(&url)
            .header("x-goog-api-key", &self.api_key)
            .json(&body)
            .send()
            .await
            .map_err(|e| ProviderError::Http(e.to_string()))?;
        let resp = check_status(resp).await?;
        fn parts_text(v: &Value) -> String {
            v["candidates"][0]["content"]["parts"]
                .as_array()
                .map(|parts| {
                    parts
                        .iter()
                        .filter(|p| !p["thought"].as_bool().unwrap_or(false))
                        .filter_map(|p| p["text"].as_str())
                        .collect::<Vec<_>>()
                        .join("")
                })
                .unwrap_or_default()
        }
        if let Some(tx) = deltas {
            let mut out = String::new();
            for_each_line(resp, |line| {
                let Some(data) = sse_data(line) else { return Ok(true) };
                let Ok(v) = serde_json::from_str::<Value>(data) else { return Ok(true) };
                if let Some(msg) = v["error"]["message"].as_str() {
                    return Err(ProviderError::Http(msg.to_string()).into());
                }
                let t = parts_text(&v);
                if !t.is_empty() {
                    out.push_str(&t);
                    let _ = tx.send(t);
                }
                Ok(true)
            })
            .await?;
            Ok(out)
        } else {
            let v: Value = resp.json().await.map_err(|e| ProviderError::Parse(e.to_string()))?;
            if v["candidates"].is_null() {
                let reason = v["promptFeedback"]["blockReason"].as_str().unwrap_or("no candidates");
                return Err(ProviderError::Parse(format!("Gemini returned no text ({reason})")).into());
            }
            Ok(parts_text(&v))
        }
    }

    // ----------------------------------------------------------------------------------------
    // OpenAI-compatible
    // ----------------------------------------------------------------------------------------

    async fn openai(&self, req: &CompletionRequest, deltas: Option<UnboundedSender<String>>) -> anyhow::Result<String> {
        let url = format!("{}/chat/completions", self.base_url());
        let stream = deltas.is_some();
        let mut messages = Vec::new();
        let mut system = req.system.clone();
        if req.json {
            system.push_str(JSON_HINT);
        }
        if !system.trim().is_empty() {
            messages.push(json!({"role": "system", "content": system}));
        }
        messages.push(json!({"role": "user", "content": req.prompt}));
        let mut body = json!({
            "model": self.model(),
            "messages": messages,
            "max_tokens": Self::max_tokens(req),
            "temperature": req.temperature,
            "stream": stream,
        });
        if req.json {
            body["response_format"] = json!({"type": "json_object"});
        }
        // Endpoints differ in what they accept; on a 400 that names a parameter, adjust and retry.
        let mut resp = None;
        for _attempt in 0..3 {
            let mut rb = self.client.post(&url).json(&body);
            if !self.api_key.is_empty() {
                rb = rb.bearer_auth(&self.api_key);
            }
            let r = rb.send().await.map_err(|e| ProviderError::Http(e.to_string()))?;
            if r.status().as_u16() != 400 {
                resp = Some(r);
                break;
            }
            let text = r.text().await.unwrap_or_default();
            let obj = body.as_object_mut().expect("object");
            if text.contains("max_tokens") && obj.contains_key("max_tokens") {
                let v = obj.remove("max_tokens").unwrap_or(json!(4096));
                obj.insert("max_completion_tokens".into(), v);
            } else if text.contains("temperature") && obj.contains_key("temperature") {
                obj.remove("temperature");
            } else if text.contains("response_format") && obj.contains_key("response_format") {
                obj.remove("response_format");
            } else {
                return Err(ProviderError::Http(format!("400: {}", super::stream::truncate(&text, 600))).into());
            }
        }
        let resp = resp.ok_or_else(|| ProviderError::Http("the endpoint rejected the request".into()))?;
        let resp = check_status(resp).await?;
        if let Some(tx) = deltas {
            let mut out = String::new();
            for_each_line(resp, |line| {
                let Some(data) = sse_data(line) else { return Ok(true) };
                if data == "[DONE]" {
                    return Ok(false);
                }
                let Ok(v) = serde_json::from_str::<Value>(data) else { return Ok(true) };
                if let Some(msg) = v["error"]["message"].as_str() {
                    return Err(ProviderError::Http(msg.to_string()).into());
                }
                if let Some(t) = v["choices"][0]["delta"]["content"].as_str() {
                    if !t.is_empty() {
                        out.push_str(t);
                        let _ = tx.send(t.to_string());
                    }
                }
                Ok(true)
            })
            .await?;
            Ok(strip_think(&out))
        } else {
            let v: Value = resp.json().await.map_err(|e| ProviderError::Parse(e.to_string()))?;
            let text = v["choices"][0]["message"]["content"]
                .as_str()
                .ok_or_else(|| ProviderError::Parse(format!("unexpected reply: {}", super::stream::truncate(&v.to_string(), 300))))?;
            Ok(strip_think(text))
        }
    }
}

impl ModelProvider for HttpProvider {
    fn brand(&self) -> String {
        self.config.kind.brand().to_string()
    }
    fn describe(&self) -> String {
        self.config.describe()
    }
    fn is_small(&self) -> bool {
        self.config.small_model
    }
    fn complete<'a>(&'a self, req: &'a CompletionRequest, deltas: Option<UnboundedSender<String>>) -> BoxFuture<'a, anyhow::Result<String>> {
        Box::pin(async move {
            if self.api_key.trim().is_empty() && self.config.kind != ProviderKind::OpenAiCompatible {
                return Err(ProviderError::MissingApiKey.into());
            }
            match self.config.kind {
                ProviderKind::Anthropic => self.anthropic(req, deltas).await,
                ProviderKind::Gemini => self.gemini(req, deltas).await,
                ProviderKind::OpenAiCompatible => self.openai(req, deltas).await,
                other => anyhow::bail!("{} is not an HTTP API provider", other.label()),
            }
        })
    }
}
