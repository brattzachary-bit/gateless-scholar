//! Local models through Ollama (default http://localhost:11434): `/api/chat` with NDJSON
//! streaming, `format: "json"` for JSON replies, and `list_models` via `/api/tags`.

use std::time::Duration;

use futures::future::BoxFuture;
use serde_json::{Value, json};
use tokio::sync::mpsc::UnboundedSender;

use super::stream::{check_status, for_each_line, http_client, strip_think};
use super::{CompletionRequest, ModelProvider, ProviderConfig, ProviderError, ProviderKind};

pub struct OllamaProvider {
    pub config: ProviderConfig,
    client: reqwest::Client,
}

impl OllamaProvider {
    pub fn new(config: ProviderConfig) -> Self {
        OllamaProvider { config, client: http_client() }
    }

    fn base_url(&self) -> String {
        self.config
            .base_url
            .clone()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| ProviderKind::Ollama.default_base_url().unwrap_or("http://localhost:11434").to_string())
            .trim_end_matches('/')
            .to_string()
    }
}

/// Context window to ask for: Ollama's default (2–4k tokens) silently truncates long prompts,
/// so size it from the prompt plus the reply budget.
fn context_size(req: &CompletionRequest) -> u32 {
    let prompt_tokens = ((req.system.len() + req.prompt.len()) as f32 / 3.2) as u32;
    let reply = if req.max_tokens == 0 { 2048 } else { req.max_tokens };
    let want = prompt_tokens + reply + 512;
    let rounded = want.div_ceil(2048) * 2048;
    rounded.clamp(4096, 32768)
}

/// Installed model names from `GET {base_url}/api/tags`.
pub async fn list_models(base_url: &str) -> anyhow::Result<Vec<String>> {
    list_models_with_timeout(base_url, Duration::from_secs(5)).await
}

pub async fn list_models_with_timeout(base_url: &str, timeout: Duration) -> anyhow::Result<Vec<String>> {
    let url = format!("{}/api/tags", base_url.trim_end_matches('/'));
    let client = reqwest::Client::builder().timeout(timeout).connect_timeout(timeout).build()?;
    let resp = client.get(&url).send().await.map_err(|e| ProviderError::Http(e.to_string()))?;
    let resp = check_status(resp).await?;
    let v: Value = resp.json().await.map_err(|e| ProviderError::Parse(e.to_string()))?;
    let mut names: Vec<String> = v["models"]
        .as_array()
        .map(|a| a.iter().filter_map(|m| m["name"].as_str().or(m["model"].as_str()).map(String::from)).collect())
        .unwrap_or_default();
    names.sort();
    Ok(names)
}

impl ModelProvider for OllamaProvider {
    fn brand(&self) -> String {
        format!("Ollama · {}", self.config.model)
    }
    fn describe(&self) -> String {
        self.config.describe()
    }
    fn is_small(&self) -> bool {
        self.config.small_model
    }
    fn complete<'a>(&'a self, req: &'a CompletionRequest, deltas: Option<UnboundedSender<String>>) -> BoxFuture<'a, anyhow::Result<String>> {
        Box::pin(async move {
            if self.config.model.trim().is_empty() {
                anyhow::bail!("no Ollama model chosen (Settings › Models)");
            }
            let url = format!("{}/api/chat", self.base_url());
            let mut messages = Vec::new();
            if !req.system.trim().is_empty() {
                messages.push(json!({"role": "system", "content": req.system}));
            }
            messages.push(json!({"role": "user", "content": req.prompt}));
            let mut options = json!({
                "temperature": req.temperature,
                "num_ctx": context_size(req),
            });
            if req.max_tokens > 0 {
                options["num_predict"] = json!(req.max_tokens);
            }
            let mut body = json!({
                "model": self.config.model.trim(),
                "messages": messages,
                "stream": true,
                "options": options,
                // No "think" flag: Ollama ≥0.9 returns reasoning in a separate `thinking` field
                // (ignored here), whereas `think: false` makes some qwen3 builds print their
                // reasoning untagged into `content`. Older servers inline <think> tags, which
                // strip_think removes.
            });
            if req.json {
                body["format"] = json!("json");
            }
            let resp = self.client.post(&url).json(&body).send().await.map_err(|e| ProviderError::Http(format!("{e} (is Ollama running?)")))?;
            let resp = check_status(resp).await?;
            let mut out = String::new();
            let mut err = None;
            for_each_line(resp, |line| {
                if line.trim().is_empty() {
                    return Ok(true);
                }
                let Ok(v) = serde_json::from_str::<Value>(line) else { return Ok(true) };
                if let Some(e) = v["error"].as_str() {
                    err = Some(e.to_string());
                    return Ok(false);
                }
                if let Some(t) = v["message"]["content"].as_str() {
                    if !t.is_empty() {
                        out.push_str(t);
                        if let Some(tx) = &deltas {
                            let _ = tx.send(t.to_string());
                        }
                    }
                }
                Ok(!v["done"].as_bool().unwrap_or(false))
            })
            .await?;
            if let Some(e) = err {
                return Err(ProviderError::Http(e).into());
            }
            Ok(strip_think(&out))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_grows_with_prompt() {
        let small = CompletionRequest { prompt: "hi".into(), max_tokens: 100, ..Default::default() };
        assert_eq!(context_size(&small), 4096);
        let big = CompletionRequest { prompt: "x".repeat(60_000), max_tokens: 2000, ..Default::default() };
        let n = context_size(&big);
        assert!(n > 18_000 && n <= 32768, "{n}");
    }

    /// Live check: `cargo test providers::ollama -- --ignored --nocapture`
    #[tokio::test]
    #[ignore]
    async fn live_ollama_completion() {
        let models = list_models("http://localhost:11434").await.expect("ollama running");
        println!("models: {models:?}");
        let mut cfg = ProviderConfig::new(ProviderKind::Ollama);
        cfg.model = "mistral-nemo:latest".into();
        let p = OllamaProvider::new(cfg);
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let req = CompletionRequest { system: "Be brief.".into(), prompt: "Name the largest planet in one word.".into(), max_tokens: 20, temperature: 0.0, ..Default::default() };
        let out = p.complete(&req, Some(tx)).await.expect("completion");
        let mut streamed = String::new();
        while let Ok(d) = rx.try_recv() {
            streamed.push_str(&d);
        }
        println!("reply: {out:?} streamed: {streamed:?}");
        assert!(out.to_lowercase().contains("jupiter"));
        let json_req = CompletionRequest { prompt: "Give {\"planet\": <largest planet>} as JSON.".into(), json: true, max_tokens: 40, ..Default::default() };
        let j = p.complete(&json_req, None).await.expect("json completion");
        println!("json: {j}");
        assert!(serde_json::from_str::<Value>(&j).is_ok());
    }
}
