//! User settings, stored as JSON in the `kv` table. Secrets (API keys) live in the system
//! keyring instead (see `providers::secrets`).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::model::{CitationStyle, Follow};
use crate::providers::{ProviderConfig, Task};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ThemePref {
    #[default]
    Dark,
    Light,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub theme: ThemePref,
    pub ui_scale: f32,
    /// Topic tabs under the search bar, in order.
    pub tabs: Vec<String>,
    /// Hide AI-only articles in Discover.
    pub hide_ai_only: bool,
    /// Contact email sent to the research APIs (polite pool). Empty = none sent.
    pub contact_email: String,
    /// Configured model providers.
    pub providers: Vec<ProviderConfig>,
    /// Which provider (by `ProviderConfig::id`) handles each task.
    pub task_providers: BTreeMap<Task, String>,
    pub citation_style: CitationStyle,
    /// History (recently read + past feed + caches) disk limit, in megabytes.
    pub history_limit_mb: u64,
    /// How many past days of Discover headlines to keep (still subject to the size limit).
    pub feed_days_kept: u32,
    /// Research cards per tab generated each day.
    pub daily_research_per_tab: u32,
    /// News stories per tab generated each day.
    pub daily_news_per_tab: u32,
    /// AI-only articles mixed into each research tab per day.
    pub daily_ai_articles_per_tab: u32,
    /// Generate Discover automatically when the app opens on a new day.
    pub auto_daily: bool,
    /// Use the desktop's title bar instead of the app's own.
    pub system_title_bar: bool,
    pub sidebar_open: bool,
    pub sidebar_width: f32,
    pub ask_panel_width: f32,
    /// Topics and authors followed for notifications.
    pub follows: Vec<Follow>,
    /// Extra news RSS feeds (outlet feeds) used alongside GDELT.
    pub news_feeds: Vec<String>,
    /// Allow local AI CLIs (Claude Code etc.) to do extra research themselves (web tools).
    pub cli_extra_research: bool,
    /// Has the first-run welcome been dismissed.
    pub welcomed: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            theme: ThemePref::Dark,
            ui_scale: 1.0,
            tabs: ["For You", "Tech", "Space", "Chemistry", "Biology", "Astrophysics", "Computer Vision", "Climate", "Medicine"]
                .into_iter()
                .map(String::from)
                .collect(),
            hide_ai_only: false,
            contact_email: String::new(),
            providers: Vec::new(),
            task_providers: BTreeMap::new(),
            citation_style: CitationStyle::Harvard,
            history_limit_mb: 500,
            feed_days_kept: 14,
            daily_research_per_tab: 12,
            daily_news_per_tab: 6,
            daily_ai_articles_per_tab: 2,
            auto_daily: true,
            system_title_bar: false,
            sidebar_open: true,
            sidebar_width: 230.0,
            ask_panel_width: 360.0,
            follows: Vec::new(),
            news_feeds: default_news_feeds(),
            cli_extra_research: false,
            welcomed: false,
        }
    }
}

pub fn default_news_feeds() -> Vec<String> {
    [
        "https://feeds.bbci.co.uk/news/science_and_environment/rss.xml",
        "https://feeds.bbci.co.uk/news/technology/rss.xml",
        "https://www.theguardian.com/science/rss",
        "https://www.theguardian.com/technology/rss",
        "https://feeds.npr.org/1019/rss.xml",
        "https://feeds.npr.org/1007/rss.xml",
        "https://www.sciencedaily.com/rss/all.xml",
        "https://phys.org/rss-feed/",
        "https://www.nasa.gov/news-release/feed/",
        "https://rss.nytimes.com/services/xml/rss/nyt/Science.xml",
        "https://feeds.arstechnica.com/arstechnica/science",
        "https://www.aljazeera.com/xml/rss/all.xml",
        "https://www.newscientist.com/subject/space/feed/",
        "https://theconversation.com/global/articles.atom",
    ]
    .into_iter()
    .map(String::from)
    .collect()
}

impl Settings {
    /// The provider configured for a task, falling back to the first enabled provider.
    pub fn provider_for(&self, task: Task) -> Option<&ProviderConfig> {
        self.task_providers
            .get(&task)
            .and_then(|id| self.providers.iter().find(|p| &p.id == id && p.enabled))
            .or_else(|| self.providers.iter().find(|p| p.enabled))
    }
}
