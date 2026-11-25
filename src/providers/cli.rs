//! Local AI CLIs using the user's own login: Claude Code, Gemini CLI, Codex.
//!
//! Each call runs the CLI as a child process in a fresh, empty temporary working directory (so
//! it never picks up a project's CLAUDE.md / GEMINI.md / AGENTS.md or touches user files), feeds
//! the prompt on stdin, and kills the child on timeout or when the future is dropped.
//!
//! Invocations:
//! - Claude Code: `claude -p --output-format stream-json --verbose [--include-partial-messages]
//!   --no-session-persistence --strict-mcp-config --disable-slash-commands --permission-mode dontAsk
//!   --system-prompt <system> --tools "" [--model M]` (with `allow_tools`: `--tools WebSearch,WebFetch
//!   --allowedTools WebSearch,WebFetch`). Text deltas come from `stream_event` lines; the final
//!   text from the `result` line.
//! - Gemini CLI: `gemini --output-format json [-m M] -p <short instruction>` with the system prompt
//!   and prompt on stdin (Gemini appends `-p` to stdin). Reads the `response` field (falls back to
//!   raw stdout for older versions). Non-interactive mode refuses tools that need approval
//!   (file edits, shell), so it cannot change anything.
//! - Codex: `codex exec --skip-git-repo-check --sandbox read-only --color never
//!   --output-last-message <tmp>/last.txt [-m M] -` with the prompt on stdin.

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use futures::future::BoxFuture;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc::UnboundedSender;

use super::detect::{binary_name, find_binary};
use super::stream::{strip_think, truncate};
use super::{CompletionRequest, ModelProvider, ProviderConfig, ProviderError, ProviderKind};

const JSON_HINT: &str = "Reply with a single valid JSON object only: no prose before or after it, no code fences.";

pub struct CliProvider {
    pub config: ProviderConfig,
    /// Kill the CLI after this long (articles can take many minutes).
    pub timeout: Duration,
}

impl CliProvider {
    pub fn new(config: ProviderConfig) -> Self {
        CliProvider { config, timeout: Duration::from_secs(15 * 60) }
    }
}

/// A fresh temporary working directory, removed on drop.
struct TempWorkDir(PathBuf);

impl TempWorkDir {
    fn new() -> std::io::Result<Self> {
        let dir = std::env::temp_dir().join(format!("gateless-cli-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir)?;
        Ok(TempWorkDir(dir))
    }
}

impl Drop for TempWorkDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn system_text(req: &CompletionRequest) -> String {
    let mut s = req.system.trim().to_string();
    if req.json {
        if !s.is_empty() {
            s.push_str("\n\n");
        }
        s.push_str(JSON_HINT);
    }
    s
}

/// What we learned from Claude Code's stream-json output.
#[derive(Default)]
struct ClaudeStream {
    streamed: String,
    result: Option<String>,
    last_assistant: Option<String>,
    error: Option<String>,
}

impl ClaudeStream {
    fn handle_line(&mut self, line: &str, deltas: &Option<UnboundedSender<String>>) {
        let Ok(v) = serde_json::from_str::<Value>(line) else { return };
        match v["type"].as_str() {
            Some("stream_event") => {
                // Only the main conversation's text (not sub-agents').
                if !v["parent_tool_use_id"].is_null() {
                    return;
                }
                let ev = &v["event"];
                if ev["type"] == "content_block_delta" && ev["delta"]["type"] == "text_delta" {
                    if let Some(t) = ev["delta"]["text"].as_str() {
                        self.streamed.push_str(t);
                        if let Some(tx) = deltas {
                            let _ = tx.send(t.to_string());
                        }
                    }
                }
            }
            Some("assistant") => {
                if !v["parent_tool_use_id"].is_null() {
                    return;
                }
                let text: String = v["message"]["content"]
                    .as_array()
                    .map(|c| c.iter().filter(|b| b["type"] == "text").filter_map(|b| b["text"].as_str()).collect::<Vec<_>>().join(""))
                    .unwrap_or_default();
                if !text.is_empty() {
                    self.last_assistant = Some(text);
                }
            }
            Some("result") => {
                let text = v["result"].as_str().map(String::from);
                if v["is_error"].as_bool().unwrap_or(false) {
                    let sub = v["subtype"].as_str().unwrap_or("error");
                    self.error = Some(text.filter(|t| !t.is_empty()).unwrap_or_else(|| sub.to_string()));
                } else {
                    self.result = text;
                }
            }
            _ => {}
        }
    }
}

impl CliProvider {
    async fn run(&self, req: &CompletionRequest, deltas: Option<UnboundedSender<String>>) -> anyhow::Result<String> {
        let kind = self.config.kind;
        let name = binary_name(kind).ok_or_else(|| anyhow::anyhow!("{} is not a CLI provider", kind.label()))?;
        // Inside a Flatpak the CLIs live on the host: run them there with flatpak-spawn.
        let in_flatpak = std::path::Path::new("/.flatpak-info").exists();
        let bin = if in_flatpak { std::path::PathBuf::from(name) } else { find_binary(name).ok_or_else(|| ProviderError::NotInstalled(kind.label().to_string()))? };
        let dir = TempWorkDir::new().map_err(|e| ProviderError::Process(format!("temp dir: {e}")))?;
        let model = self.config.model.trim();
        let system = system_text(req);

        let mut cmd = if in_flatpak {
            let mut c = Command::new("flatpak-spawn");
            c.args(["--host", "--directory=/tmp"]).arg(&bin);
            c
        } else {
            Command::new(&bin)
        };
        let stdin_text: String;
        match kind {
            ProviderKind::ClaudeCli => {
                cmd.args(["-p", "--output-format", "stream-json", "--verbose"]);
                if deltas.is_some() {
                    cmd.arg("--include-partial-messages");
                }
                cmd.args(["--no-session-persistence", "--strict-mcp-config", "--disable-slash-commands"]);
                cmd.args(["--permission-mode", "dontAsk"]);
                if req.allow_tools {
                    cmd.args(["--tools", "WebSearch,WebFetch", "--allowedTools", "WebSearch,WebFetch"]);
                } else {
                    cmd.args(["--tools", ""]);
                }
                if !system.is_empty() {
                    cmd.args(["--system-prompt", &system]);
                }
                if !model.is_empty() {
                    cmd.args(["--model", model]);
                }
                stdin_text = req.prompt.clone();
            }
            ProviderKind::GeminiCli => {
                cmd.args(["--output-format", "json"]);
                if !model.is_empty() {
                    cmd.args(["-m", model]);
                }
                cmd.args(["-p", "Follow the instructions above and reply with the requested text only."]);
                stdin_text = if system.is_empty() { req.prompt.clone() } else { format!("{system}\n\n---\n\n{}", req.prompt) };
            }
            ProviderKind::CodexCli => {
                cmd.args(["exec", "--skip-git-repo-check", "--sandbox", "read-only", "--color", "never"]);
                cmd.arg("--output-last-message").arg(dir.0.join("last.txt"));
                if !model.is_empty() {
                    cmd.args(["-m", model]);
                }
                cmd.arg("-");
                stdin_text = if system.is_empty() { req.prompt.clone() } else { format!("{system}\n\n---\n\n{}", req.prompt) };
            }
            other => anyhow::bail!("{} is not a CLI provider", other.label()),
        }
        if !in_flatpak {
            cmd.current_dir(&dir.0);
        }
        // TODO(flatpak): Codex's --output-last-message file is written on the host's /tmp view.
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        let mut child = cmd.spawn().map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                ProviderError::NotInstalled(kind.label().to_string())
            } else {
                ProviderError::Process(format!("could not start {name}: {e}"))
            }
        })?;
        let stdin = child.stdin.take();
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();

        let write = async move {
            if let Some(mut s) = stdin {
                let _ = s.write_all(stdin_text.as_bytes()).await;
                let _ = s.shutdown().await;
            }
        };
        let read_err = async move {
            let mut s = String::new();
            if let Some(mut e) = stderr {
                let _ = e.read_to_string(&mut s).await;
            }
            s
        };
        let deltas_ref = &deltas;
        let read_out = async move {
            let mut claude = ClaudeStream::default();
            let mut raw = String::new();
            if let Some(out) = stdout {
                let mut lines = BufReader::new(out).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    if kind == ProviderKind::ClaudeCli {
                        claude.handle_line(&line, deltas_ref);
                    } else {
                        raw.push_str(&line);
                        raw.push('\n');
                    }
                }
            }
            (claude, raw)
        };
        let work = async {
            let (_, err, (claude, raw)) = tokio::join!(write, read_err, read_out);
            let status = child.wait().await;
            (status, err, claude, raw)
        };
        let (status, stderr_text, claude, raw) = match tokio::time::timeout(self.timeout, work).await {
            Ok(r) => r,
            Err(_) => {
                let _ = child.start_kill();
                return Err(ProviderError::Process(format!("{name} took longer than {} minutes and was stopped", self.timeout.as_secs() / 60)).into());
            }
        };
        let status = status.map_err(|e| ProviderError::Process(e.to_string()))?;

        let text = match kind {
            ProviderKind::ClaudeCli => {
                if let Some(e) = claude.error {
                    return Err(ProviderError::Process(format!("Claude Code: {}", truncate(&e, 400))).into());
                }
                let text = claude.result.or(claude.last_assistant).unwrap_or(claude.streamed.clone());
                if text.trim().is_empty() && !status.success() {
                    return Err(ProviderError::Process(format!("claude exited with {status}: {}", truncate(stderr_text.trim(), 400))).into());
                }
                // Nothing was streamed (e.g. deltas unsupported): send the text as one delta.
                if claude.streamed.is_empty() {
                    if let Some(tx) = &deltas {
                        let _ = tx.send(text.clone());
                    }
                }
                text
            }
            ProviderKind::GeminiCli => {
                let text = parse_gemini_output(&raw);
                if text.trim().is_empty() {
                    return Err(ProviderError::Process(format!("gemini exited with {status}: {}", truncate(stderr_text.trim(), 400))).into());
                }
                if let Some(tx) = &deltas {
                    let _ = tx.send(text.clone());
                }
                text
            }
            _ => {
                let last = std::fs::read_to_string(dir.0.join("last.txt")).unwrap_or_default();
                let text = if last.trim().is_empty() { raw.clone() } else { last };
                if !status.success() || text.trim().is_empty() {
                    return Err(ProviderError::Process(format!("codex exited with {status}: {}", truncate(stderr_text.trim(), 400))).into());
                }
                if let Some(tx) = &deltas {
                    let _ = tx.send(text.clone());
                }
                text
            }
        };
        Ok(strip_think(&text))
    }
}

/// Gemini CLI `--output-format json` prints `{"response": "...", "stats": {...}}`, possibly
/// after log lines; older versions print plain text.
fn parse_gemini_output(raw: &str) -> String {
    if let Some(start) = raw.find('{') {
        if let Some(end) = raw.rfind('}') {
            if let Ok(v) = serde_json::from_str::<Value>(&raw[start..=end]) {
                if let Some(r) = v["response"].as_str() {
                    return r.to_string();
                }
                if let Some(e) = v["error"]["message"].as_str() {
                    return format!("Error: {e}");
                }
            }
        }
    }
    raw.trim().to_string()
}

impl ModelProvider for CliProvider {
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
        Box::pin(self.run(req, deltas))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_stream_parsing() {
        let mut s = ClaudeStream::default();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let d = Some(tx);
        s.handle_line(r#"{"type":"system","subtype":"init"}"#, &d);
        s.handle_line(r#"{"type":"stream_event","parent_tool_use_id":null,"event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hel"}}}"#, &d);
        s.handle_line(r#"{"type":"stream_event","parent_tool_use_id":null,"event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"lo"}}}"#, &d);
        s.handle_line("not json", &d);
        s.handle_line(r#"{"type":"result","subtype":"success","is_error":false,"result":"hello"}"#, &d);
        assert_eq!(s.streamed, "hello");
        assert_eq!(s.result.as_deref(), Some("hello"));
        assert_eq!(rx.try_recv().unwrap(), "hel");
        let mut e = ClaudeStream::default();
        e.handle_line(r#"{"type":"result","subtype":"error_during_execution","is_error":true,"result":"Not logged in"}"#, &None);
        assert_eq!(e.error.as_deref(), Some("Not logged in"));
    }

    #[test]
    fn gemini_output_parsing() {
        assert_eq!(parse_gemini_output("Loaded cached credentials.\n{\"response\": \"Hi\", \"stats\": {}}\n"), "Hi");
        assert_eq!(parse_gemini_output("plain text answer\n"), "plain text answer");
    }

    #[tokio::test]
    async fn missing_cli_is_not_installed() {
        let mut cfg = ProviderConfig::new(ProviderKind::CodexCli);
        cfg.model.clear();
        let p = CliProvider::new(cfg);
        if find_binary("codex").is_none() {
            let err = p.complete(&CompletionRequest { prompt: "hi".into(), ..Default::default() }, None).await.unwrap_err();
            assert!(err.to_string().contains("not installed"), "{err}");
        }
    }

    /// Live check (uses the user's Claude subscription; tiny prompt):
    /// `cargo test providers::cli -- --ignored --nocapture`
    #[tokio::test]
    #[ignore]
    async fn live_claude_cli() {
        let mut cfg = ProviderConfig::new(ProviderKind::ClaudeCli);
        cfg.model = "haiku".into();
        let p = CliProvider::new(cfg);
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let req = CompletionRequest { system: "Be terse.".into(), prompt: "Reply with exactly: ok gateless".into(), max_tokens: 20, ..Default::default() };
        let out = p.complete(&req, Some(tx)).await.expect("claude -p");
        let mut streamed = String::new();
        while let Ok(d) = rx.try_recv() {
            streamed.push_str(&d);
        }
        println!("reply: {out:?} streamed: {streamed:?}");
        assert!(out.to_lowercase().contains("ok gateless"));
        let jreq = CompletionRequest { prompt: "Give {\"n\": 2+2} evaluated.".into(), json: true, ..Default::default() };
        let j = p.complete(&jreq, None).await.expect("claude json");
        println!("json: {j}");
    }
}
