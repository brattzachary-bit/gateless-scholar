//! Shared HTTP plumbing for the research layer: one `reqwest::Client`, a polite User-Agent,
//! per-host rate limiting, retry with backoff on 429/5xx, and a URL-keyed response cache in
//! SQLite (`Db::cache_get/cache_put`) with per-call TTLs.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use chrono::Utc;
use tokio::time::Instant;

use super::Research;

pub const BASE_UA: &str = "GatelessScholar/0.1 (+https://github.com/brattzachary-bit/gateless-scholar)";

/// Cache lifetimes.
pub const TTL_SEARCH: Duration = Duration::from_secs(6 * 3600);
pub const TTL_META: Duration = Duration::from_secs(7 * 24 * 3600);
pub const TTL_NEWS: Duration = Duration::from_secs(30 * 60);
pub const TTL_PAGE: Duration = Duration::from_secs(3 * 24 * 3600);

/// Largest body we accept for any single response (pages, PDFs, images).
pub const MAX_BODY: usize = 12 * 1024 * 1024;

static CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .connect_timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::limited(8))
        .gzip(true)
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
});

/// Next free request slot per host.
static NEXT_SLOT: LazyLock<Mutex<HashMap<String, Instant>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// Minimum interval between requests to one host.
fn host_interval(host: &str, keyed_s2: bool) -> Duration {
    let ms = match host {
        "api.semanticscholar.org" => {
            if keyed_s2 {
                1000
            } else {
                1100
            }
        }
        "export.arxiv.org" => 3000,
        "arxiv.org" => 1000,
        "api.gdeltproject.org" => 6000,
        "api.openalex.org" => 120,
        "api.crossref.org" => 150,
        "api.unpaywall.org" => 100,
        "commons.wikimedia.org" | "upload.wikimedia.org" | "en.wikipedia.org" => 200,
        "www.ebi.ac.uk" => 150,
        "api.core.ac.uk" => 1000,
        _ => 250,
    };
    Duration::from_millis(ms)
}

async fn wait_for_slot(host: &str, interval: Duration) {
    let slot = {
        let mut map = NEXT_SLOT.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let now = Instant::now();
        let slot = map.get(host).copied().filter(|t| *t > now).unwrap_or(now);
        map.insert(host.to_string(), slot + interval);
        slot
    };
    tokio::time::sleep_until(slot).await;
}

/// Options for one GET.
#[derive(Clone, Debug, Default)]
pub struct Get<'a> {
    pub ttl: Option<Duration>,
    pub headers: Vec<(&'static str, String)>,
    /// Use a browser-like Accept header (for article pages).
    pub page: bool,
    /// Extra cache-key suffix (e.g. when headers change the response).
    pub key_suffix: &'a str,
    /// Retries on 429/5xx/timeouts.
    pub retries: u32,
    /// Per-request timeout override (default 20 s).
    pub timeout: Option<Duration>,
}

impl Research {
    pub(crate) fn user_agent(&self) -> String {
        let email = self.config.contact_email.trim();
        if email.is_empty() { BASE_UA.to_string() } else { format!("{BASE_UA}; mailto:{email}") }
    }

    pub(crate) fn contact_email(&self) -> Option<&str> {
        let e = self.config.contact_email.trim();
        (!e.is_empty()).then_some(e)
    }

    /// GET a URL with cache, rate limit and retries. Returns the body bytes.
    pub(crate) async fn get_bytes(&self, url: &str, opts: Get<'_>) -> Result<Vec<u8>> {
        let cache_key = if opts.key_suffix.is_empty() { url.to_string() } else { format!("{url}#{}", opts.key_suffix) };
        let mut stale: Option<Vec<u8>> = None;
        if let Some(ttl) = opts.ttl {
            if let Some((body, fetched)) = self.db.cache_get(&cache_key) {
                let age = (Utc::now() - fetched).to_std().unwrap_or(Duration::ZERO);
                if age < ttl {
                    return Ok(body);
                }
                stale = Some(body);
            }
        }
        match self.fetch_network(url, &opts).await {
            Ok(body) if opts.page && looks_like_bot_wall(&body) => {
                // Respect bot walls: never try to get past them, and never cache them.
                bail!("{url}: blocked by a bot check; skipped")
            }
            Ok(body) => {
                if opts.ttl.is_some() {
                    if let Err(e) = self.db.cache_put(&cache_key, &body) {
                        log::warn!("research cache write failed: {e}");
                    }
                }
                Ok(body)
            }
            Err(e) => {
                if let Some(body) = stale {
                    log::warn!("research: {url} failed ({e:#}); using stale cache");
                    Ok(body)
                } else {
                    Err(e)
                }
            }
        }
    }

    async fn fetch_network(&self, url: &str, opts: &Get<'_>) -> Result<Vec<u8>> {
        let parsed = url::Url::parse(url).with_context(|| format!("bad url {url}"))?;
        if !matches!(parsed.scheme(), "http" | "https") {
            bail!("unsupported scheme in {url}");
        }
        let host = parsed.host_str().unwrap_or("").to_string();
        let interval = host_interval(&host, self.config.s2_api_key.is_some());
        let retries = opts.retries;
        let mut last_err = anyhow!("no attempt made");
        for attempt in 0..=retries {
            wait_for_slot(&host, interval).await;
            let mut req = CLIENT.get(url).header(reqwest::header::USER_AGENT, self.user_agent());
            if opts.page {
                req = req
                    .header(reqwest::header::ACCEPT, "text/html,application/xhtml+xml,application/xml;q=0.9,application/pdf;q=0.8,*/*;q=0.5")
                    .header(reqwest::header::ACCEPT_LANGUAGE, "en;q=0.9,*;q=0.5");
            }
            for (k, v) in &opts.headers {
                req = req.header(*k, v);
            }
            if let Some(t) = opts.timeout {
                req = req.timeout(t);
            }
            let result = req.send().await;
            let retry_after = match result {
                Ok(resp) => {
                    let status = resp.status();
                    if status.is_success() {
                        if let Some(len) = resp.content_length() {
                            if len as usize > MAX_BODY {
                                bail!("{url}: body too large ({len} bytes)");
                            }
                        }
                        return read_capped(resp, MAX_BODY).await.with_context(|| format!("reading {url}"));
                    }
                    let retryable = status.as_u16() == 429 || status.is_server_error();
                    last_err = anyhow!("{url}: HTTP {status}");
                    if !retryable {
                        return Err(last_err);
                    }
                    resp.headers()
                        .get(reqwest::header::RETRY_AFTER)
                        .and_then(|v| v.to_str().ok())
                        .and_then(|v| v.trim().parse::<u64>().ok())
                        .map(|s| Duration::from_secs(s.min(10)))
                }
                Err(e) => {
                    let retryable = e.is_timeout() || e.is_connect() || e.is_request();
                    last_err = anyhow::Error::new(e).context(url.to_string());
                    if !retryable {
                        return Err(last_err);
                    }
                    None
                }
            };
            if attempt < retries {
                let backoff = retry_after.unwrap_or_else(|| Duration::from_millis(1000 * (1 << attempt))).max(interval);
                log::debug!("research: retrying {url} in {backoff:?}");
                tokio::time::sleep(backoff).await;
            }
        }
        Err(last_err)
    }

    pub(crate) async fn get_json(&self, url: &str, ttl: Duration) -> Result<serde_json::Value> {
        self.get_json_with(url, Get { ttl: Some(ttl), retries: 2, ..Default::default() }).await
    }

    pub(crate) async fn get_json_with(&self, url: &str, opts: Get<'_>) -> Result<serde_json::Value> {
        let body = self.get_bytes(url, opts).await?;
        serde_json::from_slice(&body).with_context(|| {
            let preview: String = String::from_utf8_lossy(&body).chars().take(160).collect();
            format!("{url}: not JSON: {preview}")
        })
    }

    pub(crate) async fn get_text(&self, url: &str, ttl: Duration) -> Result<String> {
        let body = self.get_bytes(url, Get { ttl: Some(ttl), retries: 2, ..Default::default() }).await?;
        Ok(String::from_utf8_lossy(&body).into_owned())
    }

    /// Fetch a web page (article / landing page) with a browser-like Accept header.
    /// Returns (final bytes, looks-like-pdf).
    pub(crate) async fn get_page(&self, url: &str) -> Result<Vec<u8>> {
        self.get_bytes(url, Get { ttl: Some(TTL_PAGE), page: true, retries: 1, ..Default::default() }).await
    }
}

/// A CAPTCHA / bot-challenge interstitial instead of the page.
pub fn looks_like_bot_wall(body: &[u8]) -> bool {
    if body.starts_with(b"%PDF") || body.len() > 400_000 {
        return false;
    }
    let head = String::from_utf8_lossy(&body[..body.len().min(30_000)]).to_lowercase();
    head.contains("recaptcha/challengepage")
        || head.contains("cf-challenge")
        || head.contains("challenge-platform")
        || (head.contains("<title>just a moment") && head.contains("cloudflare"))
        || head.contains("captcha-delivery.com")
}

async fn read_capped(mut resp: reqwest::Response, cap: usize) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    while let Some(chunk) = resp.chunk().await? {
        if out.len() + chunk.len() > cap {
            bail!("body exceeds {cap} bytes");
        }
        out.extend_from_slice(&chunk);
    }
    Ok(out)
}

/// Percent-encode a query component.
pub fn enc(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

/// Download raw bytes without caching in SQLite (images go to the disk cache instead).
pub(crate) async fn download_uncached(r: &Research, url: &str, cap: usize) -> Result<Vec<u8>> {
    let parsed = url::Url::parse(url).with_context(|| format!("bad url {url}"))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        bail!("unsupported scheme in {url}");
    }
    let host = parsed.host_str().unwrap_or("").to_string();
    let mut last = anyhow!("no attempt");
    for attempt in 0..3u32 {
        wait_for_slot(&host, host_interval(&host, false)).await;
        match CLIENT.get(url).header(reqwest::header::USER_AGENT, r.user_agent()).send().await {
            Ok(resp) if resp.status().is_success() => {
                if resp.content_length().is_some_and(|l| l as usize > cap) {
                    bail!("{url}: image too large");
                }
                return read_capped(resp, cap).await;
            }
            Ok(resp) => {
                let st = resp.status();
                last = anyhow!("{url}: HTTP {st}");
                if !(st.as_u16() == 429 || st.is_server_error()) {
                    return Err(last);
                }
            }
            Err(e) => last = anyhow::Error::new(e).context(url.to_string()),
        }
        tokio::time::sleep(Duration::from_millis(800 * (1 << attempt))).await;
    }
    Err(last)
}
