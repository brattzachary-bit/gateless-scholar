//! Shared helpers for HTTP providers: a configured client, error mapping, and line-by-line
//! reading of streamed responses (SSE for Anthropic/Gemini/OpenAI, NDJSON for Ollama).

use std::time::Duration;

use futures::StreamExt;

use super::ProviderError;

/// Long overall timeout: journal-length articles can take many minutes on slow models.
pub const LONG_TIMEOUT: Duration = Duration::from_secs(15 * 60);

pub fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(20))
        .timeout(LONG_TIMEOUT)
        .user_agent(concat!("GatelessScholar/", env!("CARGO_PKG_VERSION")))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

/// Turn a non-success response into a readable error (status + start of the body).
pub async fn check_status(resp: reqwest::Response) -> Result<reqwest::Response, ProviderError> {
    let status = resp.status();
    if status.is_success() {
        return Ok(resp);
    }
    let body = resp.text().await.unwrap_or_default();
    Err(ProviderError::Http(format!("{status}: {}", truncate(&body, 600))))
}

pub fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

/// Call `f` for every complete line of the response body (without the trailing newline).
/// `f` returns `false` to stop reading early.
pub async fn for_each_line<F>(resp: reqwest::Response, mut f: F) -> anyhow::Result<()>
where
    F: FnMut(&str) -> anyhow::Result<bool>,
{
    let mut stream = resp.bytes_stream();
    let mut buf: Vec<u8> = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| ProviderError::Http(e.to_string()))?;
        buf.extend_from_slice(&chunk);
        while let Some(pos) = buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = buf.drain(..=pos).collect();
            let line = String::from_utf8_lossy(&line);
            let line = line.trim_end_matches(['\n', '\r']);
            if !f(line)? {
                return Ok(());
            }
        }
    }
    if !buf.is_empty() {
        let line = String::from_utf8_lossy(&buf).to_string();
        f(line.trim_end_matches(['\n', '\r']))?;
    }
    Ok(())
}

/// The payload of an SSE `data:` line, if this is one.
pub fn sse_data(line: &str) -> Option<&str> {
    line.strip_prefix("data:").map(str::trim)
}

/// Remove `<think>…</think>` reasoning blocks some local models emit before their answer.
pub fn strip_think(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    loop {
        match rest.find("<think>") {
            Some(start) => {
                out.push_str(&rest[..start]);
                match rest[start..].find("</think>") {
                    Some(end) => rest = &rest[start + end + "</think>".len()..],
                    None => {
                        rest = "";
                    }
                }
            }
            None => {
                out.push_str(rest);
                break;
            }
        }
    }
    // A lone closing tag (the opening one was in the template) — keep only what follows it.
    if let Some(pos) = out.find("</think>") {
        out = out[pos + "</think>".len()..].to_string();
    }
    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_think_blocks() {
        assert_eq!(strip_think("<think>hmm</think>\nAnswer"), "Answer");
        assert_eq!(strip_think("a<think>x</think>b"), "ab");
        assert_eq!(strip_think("reasoning</think> final"), "final");
        assert_eq!(strip_think("plain"), "plain");
    }

    #[test]
    fn sse_lines() {
        assert_eq!(sse_data("data: {\"a\":1}"), Some("{\"a\":1}"));
        assert_eq!(sse_data("event: ping"), None);
    }

    #[test]
    fn truncate_respects_char_boundaries() {
        let s = "ééééé";
        let t = truncate(s, 3);
        assert!(t.starts_with('é'));
    }
}
