//! "Use my PC": the desktop app can let your phone use the AI models set up on this computer
//! — including the Claude Code CLI with your own subscription login, or a local Ollama.
//!
//! Server (desktop, opt-in, only while the app is open): a tiny HTTP endpoint on the local
//! network / Tailscale, protected by a pairing code.
//!   GET  /v1/info      → which models this PC offers
//!   POST /v1/complete  → runs one completion; replies as newline-delimited JSON:
//!                        {"delta":"…"}… then {"done":"full text","brand":"Claude","describe":"…"}
//!                        or {"error":"…"}
//! Client (phone or another PC): `RelayProvider`, a normal `ModelProvider`.
//!
//! The pairing code is sent in a header; on plain Wi-Fi the traffic itself is not encrypted,
//! so use it on your own network, or over Tailscale (which encrypts everything).

use std::sync::{Arc, RwLock};
use std::time::Duration;

use anyhow::{Context, Result};
use futures::StreamExt;
use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc::UnboundedSender;

use crate::providers::{self, CompletionRequest, ModelProvider, ProviderConfig};

pub const DEFAULT_PORT: u16 = 7878;
const MAX_BODY: usize = 4 * 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RemoteModel {
    pub id: String,
    pub label: String,
    pub describe: String,
    pub brand: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Info {
    pub app: String,
    pub version: String,
    pub default_id: Option<String>,
    pub models: Vec<RemoteModel>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct WireRequest {
    #[serde(default)]
    provider: Option<String>,
    system: String,
    prompt: String,
    max_tokens: u32,
    temperature: f32,
    json: bool,
    allow_tools: bool,
}

/// A short, readable pairing code (about 80 bits).
pub fn new_pairing_code() -> String {
    const ALPHABET: &[u8] = b"abcdefghjkmnpqrstuvwxyz23456789";
    let bytes = uuid::Uuid::new_v4().into_bytes();
    let chars: String = bytes.iter().take(16).map(|b| ALPHABET[*b as usize % ALPHABET.len()] as char).collect();
    format!("{}-{}-{}-{}", &chars[0..4], &chars[4..8], &chars[8..12], &chars[12..16])
}

/// Addresses the phone can use to reach this PC (LAN and, if present, Tailscale).
pub fn local_addresses(port: u16) -> Vec<String> {
    let mut out = vec![];
    // The address the OS would use for outbound traffic (no packet is sent).
    if let Ok(s) = std::net::UdpSocket::bind("0.0.0.0:0")
        && s.connect("192.0.2.1:9").is_ok()
        && let Ok(a) = s.local_addr()
    {
        out.push(format!("http://{}:{port}", a.ip()));
    }
    #[cfg(not(target_os = "android"))]
    if let Ok(o) = std::process::Command::new("tailscale").args(["ip", "-4"]).output()
        && o.status.success()
    {
        for ip in String::from_utf8_lossy(&o.stdout).lines().map(str::trim).filter(|l| !l.is_empty()) {
            out.push(format!("http://{ip}:{port} (Tailscale)"));
        }
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Server (desktop)
// ---------------------------------------------------------------------------------------------

/// What the server offers; updated when settings change.
#[derive(Clone, Default)]
pub struct Offer {
    pub providers: Vec<ProviderConfig>,
    pub default_id: Option<String>,
}

pub struct Server {
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    pub offer: Arc<RwLock<Offer>>,
    pub port: u16,
    pub code: String,
}

impl Drop for Server {
    fn drop(&mut self) {
        if let Some(s) = self.shutdown.take() {
            let _ = s.send(());
        }
    }
}

/// Start the relay. Stops when the returned `Server` is dropped.
pub fn start(rt: &tokio::runtime::Handle, port: u16, code: String, offer: Offer) -> Result<Server> {
    let std_listener = std::net::TcpListener::bind(("0.0.0.0", port)).with_context(|| format!("port {port} is in use"))?;
    std_listener.set_nonblocking(true)?;
    let offer = Arc::new(RwLock::new(offer));
    let (tx, mut rx) = tokio::sync::oneshot::channel::<()>();
    let srv_offer = offer.clone();
    let srv_code = code.clone();
    rt.spawn(async move {
        let Ok(listener) = TcpListener::from_std(std_listener) else { return };
        loop {
            tokio::select! {
                _ = &mut rx => break,
                acc = listener.accept() => {
                    let Ok((stream, peer)) = acc else { continue };
                    let offer = srv_offer.clone();
                    let code = srv_code.clone();
                    tokio::spawn(async move {
                        if let Err(e) = handle(stream, &code, offer).await {
                            log::info!("relay: {peer}: {e:#}");
                        }
                    });
                }
            }
        }
    });
    Ok(Server { shutdown: Some(tx), offer, port, code })
}

async fn handle(mut stream: TcpStream, code: &str, offer: Arc<RwLock<Offer>>) -> Result<()> {
    let (method, path, headers, body) = tokio::time::timeout(Duration::from_secs(20), read_request(&mut stream)).await.context("timeout")??;
    let auth = headers.iter().find(|(k, _)| k == "authorization").map(|(_, v)| v.trim().trim_start_matches("Bearer ").trim().to_string()).unwrap_or_default();
    if !constant_time_eq(auth.as_bytes(), code.as_bytes()) {
        tokio::time::sleep(Duration::from_millis(800)).await;
        return respond(&mut stream, 401, "application/json", br#"{"error":"wrong pairing code"}"#).await;
    }
    match (method.as_str(), path.as_str()) {
        ("GET", "/v1/info") => {
            let o = offer.read().map(|o| o.clone()).unwrap_or_default();
            let models = o
                .providers
                .iter()
                .filter(|p| p.enabled)
                .map(|p| RemoteModel { id: p.id.clone(), label: p.label.clone(), describe: p.describe(), brand: p.kind.brand().to_string() })
                .collect();
            let info = Info { app: "Gateless Scholar".into(), version: env!("CARGO_PKG_VERSION").into(), default_id: o.default_id.clone(), models };
            respond(&mut stream, 200, "application/json", &serde_json::to_vec(&info)?).await
        }
        ("POST", "/v1/complete") => {
            let req: WireRequest = serde_json::from_slice(&body).context("bad request body")?;
            let o = offer.read().map(|o| o.clone()).unwrap_or_default();
            let wanted = req.provider.clone().filter(|s| !s.is_empty()).or(o.default_id.clone());
            let cfg = wanted.and_then(|id| o.providers.iter().find(|p| p.id == id && p.enabled).cloned()).or_else(|| o.providers.iter().find(|p| p.enabled).cloned());
            let Some(cfg) = cfg else {
                return respond(&mut stream, 503, "application/json", br#"{"error":"no AI model is set up on this PC"}"#).await;
            };
            stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/x-ndjson\r\nCache-Control: no-cache\r\nConnection: close\r\n\r\n").await?;
            let provider = match providers::build(&cfg) {
                Ok(p) => p,
                Err(e) => return write_line(&mut stream, &serde_json::json!({ "error": e.to_string() })).await,
            };
            let creq = CompletionRequest { system: req.system, prompt: req.prompt, max_tokens: req.max_tokens, temperature: req.temperature, json: req.json, allow_tools: req.allow_tools };
            let (dtx, mut drx) = tokio::sync::mpsc::unbounded_channel::<String>();
            let job = tokio::spawn(async move { provider.complete(&creq, Some(dtx)).await.map(|t| (t, provider.brand(), provider.describe())) });
            while let Some(d) = drx.recv().await {
                // The phone may have gone away; stop streaming then.
                if write_line(&mut stream, &serde_json::json!({ "delta": d })).await.is_err() {
                    job.abort();
                    return Ok(());
                }
            }
            match job.await {
                Ok(Ok((text, brand, describe))) => write_line(&mut stream, &serde_json::json!({ "done": text, "brand": brand, "describe": format!("{describe} on my PC") })).await,
                Ok(Err(e)) => write_line(&mut stream, &serde_json::json!({ "error": format!("{e:#}") })).await,
                Err(e) => write_line(&mut stream, &serde_json::json!({ "error": e.to_string() })).await,
            }
        }
        _ => respond(&mut stream, 404, "application/json", br#"{"error":"not found"}"#).await,
    }
}

type Request = (String, String, Vec<(String, String)>, Vec<u8>);

async fn read_request(stream: &mut TcpStream) -> Result<Request> {
    let mut buf = Vec::with_capacity(4096);
    let mut tmp = [0u8; 4096];
    let header_end = loop {
        let n = stream.read(&mut tmp).await?;
        anyhow::ensure!(n > 0, "connection closed");
        buf.extend_from_slice(&tmp[..n]);
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i;
        }
        anyhow::ensure!(buf.len() < 32 * 1024, "headers too large");
    };
    let head = String::from_utf8_lossy(&buf[..header_end]).to_string();
    let mut lines = head.split("\r\n");
    let first = lines.next().unwrap_or_default();
    let mut parts = first.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let path = parts.next().unwrap_or_default().to_string();
    let headers: Vec<(String, String)> = lines.filter_map(|l| l.split_once(':')).map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string())).collect();
    let len: usize = headers.iter().find(|(k, _)| k == "content-length").and_then(|(_, v)| v.parse().ok()).unwrap_or(0);
    anyhow::ensure!(len <= MAX_BODY, "body too large");
    let mut body = buf[header_end + 4..].to_vec();
    while body.len() < len {
        let n = stream.read(&mut tmp).await?;
        anyhow::ensure!(n > 0, "connection closed");
        body.extend_from_slice(&tmp[..n]);
    }
    body.truncate(len);
    Ok((method, path, headers, body))
}

async fn respond(stream: &mut TcpStream, status: u16, ctype: &str, body: &[u8]) -> Result<()> {
    let reason = match status {
        200 => "OK",
        401 => "Unauthorized",
        404 => "Not Found",
        _ => "Service Unavailable",
    };
    stream.write_all(format!("HTTP/1.1 {status} {reason}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).as_bytes()).await?;
    stream.write_all(body).await?;
    Ok(())
}

async fn write_line(stream: &mut TcpStream, v: &serde_json::Value) -> Result<()> {
    let mut line = serde_json::to_vec(v)?;
    line.push(b'\n');
    stream.write_all(&line).await?;
    stream.flush().await?;
    Ok(())
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() || a.is_empty() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

// ---------------------------------------------------------------------------------------------
// Client (phone)
// ---------------------------------------------------------------------------------------------

fn client() -> &'static reqwest::Client {
    static C: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    C.get_or_init(|| reqwest::Client::builder().connect_timeout(Duration::from_secs(8)).timeout(Duration::from_secs(20 * 60)).build().unwrap_or_default())
}

/// The base URL as typed ("192.168.1.20", "pc:7878", "http://…"), normalised.
pub fn normalise_url(s: &str) -> String {
    let s = s.trim().trim_end_matches('/').split_whitespace().next().unwrap_or("").to_string();
    let s = if s.starts_with("http://") || s.starts_with("https://") { s } else { format!("http://{s}") };
    let host_part = s.split("://").nth(1).unwrap_or("");
    if host_part.contains(':') { s } else { format!("{s}:{DEFAULT_PORT}") }
}

/// Ask the PC which models it offers (also checks the pairing code).
pub async fn fetch_info(base_url: &str, code: &str) -> Result<Info> {
    let r = client().get(format!("{}/v1/info", normalise_url(base_url))).bearer_auth(code.trim()).timeout(Duration::from_secs(10)).send().await.context("could not reach the PC — is Gateless Scholar open there with “Use this PC from my phone” on?")?;
    if r.status() == reqwest::StatusCode::UNAUTHORIZED {
        anyhow::bail!("the pairing code is wrong");
    }
    Ok(r.error_for_status()?.json().await?)
}

pub struct RelayProvider {
    pub config: ProviderConfig,
    code: String,
}

impl RelayProvider {
    pub fn new(config: ProviderConfig, code: String) -> Self {
        RelayProvider { config, code }
    }
}

impl ModelProvider for RelayProvider {
    fn brand(&self) -> String {
        self.config.remote_brand.clone().unwrap_or_else(|| "AI".into())
    }
    fn describe(&self) -> String {
        format!("{} via my PC", if self.config.model.is_empty() { "PC's default model".to_string() } else { self.config.label.clone() })
    }
    fn is_small(&self) -> bool {
        self.config.small_model
    }
    fn complete<'a>(&'a self, req: &'a CompletionRequest, deltas: Option<UnboundedSender<String>>) -> BoxFuture<'a, Result<String>> {
        Box::pin(async move {
            let base = normalise_url(self.config.base_url.as_deref().unwrap_or(""));
            let body = WireRequest {
                provider: (!self.config.model.is_empty()).then(|| self.config.model.clone()),
                system: req.system.clone(),
                prompt: req.prompt.clone(),
                max_tokens: req.max_tokens,
                temperature: req.temperature,
                json: req.json,
                allow_tools: req.allow_tools,
            };
            let r = client().post(format!("{base}/v1/complete")).bearer_auth(self.code.trim()).json(&body).send().await.context("could not reach your PC")?;
            if r.status() == reqwest::StatusCode::UNAUTHORIZED {
                anyhow::bail!("your PC rejected the pairing code");
            }
            if !r.status().is_success() {
                let v: serde_json::Value = r.json().await.unwrap_or_default();
                anyhow::bail!("on your PC: {}", v.get("error").and_then(|e| e.as_str()).unwrap_or("request failed"));
            }
            let mut stream = r.bytes_stream();
            let mut buf = Vec::new();
            while let Some(chunk) = stream.next().await {
                buf.extend_from_slice(&chunk?);
                while let Some(i) = buf.iter().position(|b| *b == b'\n') {
                    let line: Vec<u8> = buf.drain(..=i).collect();
                    let Ok(v) = serde_json::from_slice::<serde_json::Value>(&line) else { continue };
                    if let Some(d) = v.get("delta").and_then(|d| d.as_str()) {
                        if let Some(tx) = &deltas {
                            let _ = tx.send(d.to_string());
                        }
                    } else if let Some(t) = v.get("done").and_then(|d| d.as_str()) {
                        return Ok(t.to_string());
                    } else if let Some(e) = v.get("error").and_then(|d| d.as_str()) {
                        anyhow::bail!("on your PC: {e}");
                    }
                }
            }
            anyhow::bail!("your PC closed the connection before finishing")
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_are_normalised() {
        assert_eq!(normalise_url("192.168.1.5"), "http://192.168.1.5:7878");
        assert_eq!(normalise_url("http://pc:9000/"), "http://pc:9000");
        assert_eq!(normalise_url("http://100.1.2.3:7878 (Tailscale)"), "http://100.1.2.3:7878");
    }

    #[test]
    fn codes_compare_exactly() {
        let c = new_pairing_code();
        assert_eq!(c.len(), 19);
        assert!(constant_time_eq(c.as_bytes(), c.as_bytes()));
        assert!(!constant_time_eq(b"abcd", b"abce"));
        assert!(!constant_time_eq(b"", b""));
    }

    /// Full round trip against a local server with no models set up.
    #[test]
    fn relay_round_trip_reports_missing_model() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let code = new_pairing_code();
        let server = start(rt.handle(), 47878, code.clone(), Offer::default()).unwrap();
        rt.block_on(async {
            let info = fetch_info("127.0.0.1:47878", &code).await.unwrap();
            assert!(info.models.is_empty());
            assert!(fetch_info("127.0.0.1:47878", "wrong").await.is_err());
            let mut cfg = ProviderConfig::new(crate::providers::ProviderKind::Relay);
            cfg.base_url = Some("127.0.0.1:47878".into());
            let p = RelayProvider::new(cfg, code.clone());
            let err = p.complete(&CompletionRequest { prompt: "hi".into(), max_tokens: 5, ..Default::default() }, None).await.unwrap_err();
            assert!(err.to_string().contains("no AI model"), "{err}");
        });
        drop(server);
    }
}
