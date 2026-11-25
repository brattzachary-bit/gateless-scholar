//! Model providers. The user picks one per task (a small model for headlines, a strong one
//! for full articles). Every provider only has to write from the sources it is given; the
//! research layer does the research.
//!
//! INTERFACE FILE: the types and signatures here are shared with the UI. Implementations live
//! in the sibling modules.

use std::fmt;
use std::sync::Arc;

use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::UnboundedSender;

pub mod cli;
pub mod detect;
pub mod http_api;
pub mod ollama;
pub mod secrets;
pub mod stream;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ProviderKind {
    /// `claude -p --output-format json` (Claude Code, the user's own login).
    ClaudeCli,
    /// `gemini -p` (Gemini CLI, the user's own login).
    GeminiCli,
    /// `codex exec` (OpenAI Codex CLI, the user's own login).
    CodexCli,
    /// Anthropic Messages API with an API key.
    Anthropic,
    /// Google Gemini API with an API key (free tier works).
    Gemini,
    /// Any OpenAI-compatible chat completions endpoint.
    OpenAiCompatible,
    /// Local Ollama server.
    Ollama,
    /// "Use my PC": the models set up in Gateless Scholar on your computer (e.g. Claude Code
    /// with your subscription), reached over the local network or Tailscale.
    Relay,
}

impl ProviderKind {
    pub const ALL: [ProviderKind; 8] = [
        ProviderKind::ClaudeCli,
        ProviderKind::GeminiCli,
        ProviderKind::CodexCli,
        ProviderKind::Anthropic,
        ProviderKind::Gemini,
        ProviderKind::OpenAiCompatible,
        ProviderKind::Ollama,
        ProviderKind::Relay,
    ];
    pub fn label(self) -> &'static str {
        match self {
            ProviderKind::ClaudeCli => "Claude Code CLI",
            ProviderKind::GeminiCli => "Gemini CLI",
            ProviderKind::CodexCli => "Codex CLI",
            ProviderKind::Anthropic => "Anthropic API",
            ProviderKind::Gemini => "Google Gemini API",
            ProviderKind::OpenAiCompatible => "OpenAI-compatible API",
            ProviderKind::Ollama => "Ollama (local)",
            ProviderKind::Relay => "Use my PC (Gateless Scholar desktop)",
        }
    }
    /// The name used in bylines: "Gateless Scholar + Claude".
    pub fn brand(self) -> &'static str {
        match self {
            ProviderKind::ClaudeCli | ProviderKind::Anthropic => "Claude",
            ProviderKind::GeminiCli | ProviderKind::Gemini => "Gemini",
            ProviderKind::CodexCli => "Codex",
            ProviderKind::OpenAiCompatible => "AI",
            ProviderKind::Ollama => "Ollama",
            ProviderKind::Relay => "AI",
        }
    }
    pub fn needs_api_key(self) -> bool {
        matches!(self, ProviderKind::Anthropic | ProviderKind::Gemini | ProviderKind::OpenAiCompatible | ProviderKind::Relay)
    }
    /// Can this kind run on the current platform? (The AI CLIs can't run on a phone.)
    pub fn available_here(self) -> bool {
        !(self.is_cli() && crate::platform::is_android())
    }
    pub fn is_cli(self) -> bool {
        matches!(self, ProviderKind::ClaudeCli | ProviderKind::GeminiCli | ProviderKind::CodexCli)
    }
    /// Default model name shown in settings ("" = the CLI's own default).
    pub fn default_model(self) -> &'static str {
        match self {
            ProviderKind::ClaudeCli | ProviderKind::GeminiCli | ProviderKind::CodexCli => "",
            ProviderKind::Anthropic => "claude-sonnet-5-5",
            ProviderKind::Gemini => "gemini-2.5-flash",
            ProviderKind::OpenAiCompatible => "gpt-4o-mini",
            ProviderKind::Ollama => "llama3.1",
            ProviderKind::Relay => "",
        }
    }
    pub fn default_base_url(self) -> Option<&'static str> {
        match self {
            ProviderKind::Anthropic => Some("https://api.anthropic.com"),
            ProviderKind::Gemini => Some("https://generativelanguage.googleapis.com"),
            ProviderKind::OpenAiCompatible => Some("https://api.openai.com/v1"),
            ProviderKind::Ollama => Some("http://localhost:11434"),
            ProviderKind::Relay => Some("http://192.168.1.2:7878"),
            _ => None,
        }
    }
}

/// A configured provider (saved in settings; its API key, if any, is in the keyring under `id`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub id: String,
    pub kind: ProviderKind,
    /// User-visible name, e.g. "Claude (CLI)".
    pub label: String,
    /// Model name; empty = provider default.
    pub model: String,
    /// Base URL for HTTP providers.
    pub base_url: Option<String>,
    pub enabled: bool,
    /// Treat as a small/weak model: write section by section with per-section sources.
    pub small_model: bool,
    /// "Use my PC": the brand of the PC's chosen model, for bylines ("Claude").
    #[serde(default)]
    pub remote_brand: Option<String>,
}

impl ProviderConfig {
    pub fn new(kind: ProviderKind) -> Self {
        ProviderConfig {
            id: format!("{:?}-{}", kind, &uuid::Uuid::new_v4().simple().to_string()[..8]).to_lowercase(),
            kind,
            label: kind.label().to_string(),
            model: kind.default_model().to_string(),
            base_url: kind.default_base_url().map(String::from),
            enabled: true,
            small_model: matches!(kind, ProviderKind::Ollama),
            remote_brand: None,
        }
    }
    pub fn describe(&self) -> String {
        if self.model.is_empty() { self.label.clone() } else { format!("{} ({})", self.label, self.model) }
    }
}

/// What a provider is used for. Each task can use a different provider.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Task {
    /// Daily Discover headlines and short abstracts.
    Headlines,
    /// Full research articles (interpretations, full-text rewording, AI articles).
    Articles,
    /// News stories.
    News,
    /// Ask-panel answers.
    Ask,
    /// AI search mode: query planning and curation.
    Search,
    /// Learn AI: plain-language explanations for non-specialists.
    Learn,
}

impl Task {
    pub const ALL: [Task; 6] = [Task::Headlines, Task::Articles, Task::News, Task::Ask, Task::Search, Task::Learn];
    pub fn label(self) -> &'static str {
        match self {
            Task::Headlines => "Discover headlines",
            Task::Articles => "Full research articles",
            Task::News => "News stories",
            Task::Ask => "Ask panel",
            Task::Search => "AI search mode",
            Task::Learn => "Learn AI (plain-language explanations)",
        }
    }
}

/// One completion request. Providers return plain text (the writing pipeline parses it).
#[derive(Clone, Debug, Default)]
pub struct CompletionRequest {
    pub system: String,
    pub prompt: String,
    pub max_tokens: u32,
    pub temperature: f32,
    /// Ask for a JSON reply (providers that support a JSON mode should enable it).
    pub json: bool,
    /// CLI providers only: allow the CLI to use its own web/search tools for extra research.
    /// The research layer's sources are always included in the prompt regardless.
    pub allow_tools: bool,
}

#[derive(Debug)]
pub enum ProviderError {
    NotInstalled(String),
    MissingApiKey,
    Http(String),
    Process(String),
    Parse(String),
    Cancelled,
}

impl fmt::Display for ProviderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProviderError::NotInstalled(s) => write!(f, "{s} is not installed or not on PATH"),
            ProviderError::MissingApiKey => write!(f, "no API key or pairing code saved for this provider (Settings › Models)"),
            ProviderError::Http(s) => write!(f, "HTTP error: {s}"),
            ProviderError::Process(s) => write!(f, "process error: {s}"),
            ProviderError::Parse(s) => write!(f, "could not read the model's reply: {s}"),
            ProviderError::Cancelled => write!(f, "cancelled"),
        }
    }
}
impl std::error::Error for ProviderError {}

pub trait ModelProvider: Send + Sync {
    /// Short brand for bylines ("Claude", "Gemini", "Ollama · llama3.1").
    fn brand(&self) -> String;
    /// Full description for "generated by" lines ("Claude Code CLI (sonnet)").
    fn describe(&self) -> String;
    /// Weak/small model hint: the writing pipeline then writes section by section.
    fn is_small(&self) -> bool;
    /// Run one completion. When `deltas` is given and the provider can stream, send text
    /// deltas as they arrive; the returned String is always the full text.
    fn complete<'a>(
        &'a self,
        req: &'a CompletionRequest,
        deltas: Option<UnboundedSender<String>>,
    ) -> BoxFuture<'a, anyhow::Result<String>>;
}

pub type SharedProvider = Arc<dyn ModelProvider>;

/// Build a provider from its saved configuration (reads its API key from the keyring if needed).
pub fn build(config: &ProviderConfig) -> anyhow::Result<SharedProvider> {
    match config.kind {
        ProviderKind::ClaudeCli | ProviderKind::GeminiCli | ProviderKind::CodexCli => {
            Ok(Arc::new(cli::CliProvider::new(config.clone())))
        }
        ProviderKind::Anthropic | ProviderKind::Gemini | ProviderKind::OpenAiCompatible => {
            let key = secrets::get_api_key(&config.id)?.ok_or(ProviderError::MissingApiKey)?;
            Ok(Arc::new(http_api::HttpProvider::new(config.clone(), key)))
        }
        ProviderKind::Ollama => Ok(Arc::new(ollama::OllamaProvider::new(config.clone()))),
        ProviderKind::Relay => {
            let code = secrets::get_api_key(&config.id)?.ok_or(ProviderError::MissingApiKey)?;
            Ok(Arc::new(crate::relay::RelayProvider::new(config.clone(), code)))
        }
    }
}
