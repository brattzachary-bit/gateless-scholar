//! The shared data model. Everything the UI, storage, research layer, writing pipeline and
//! exporters exchange is defined here, so each part codes against one set of types.
//!
//! The most important rule of the app lives here too: every passage of text carries its
//! [`Origin`], and everything that displays or exports text is driven from that field.

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------------------------
// Papers, news items and images (what the research layer finds)
// ---------------------------------------------------------------------------------------------

/// What kind of item a card or search result is. Drives the badge on feed cards.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum ContentKind {
    /// A real paper with a legal free full text (open access, preprint, author version).
    FreeFullText,
    /// A real paper whose free content is only the abstract. Opening it produces an
    /// "AI interpretation – not the original paper".
    #[default]
    AbstractOnly,
    /// An article written entirely by the AI from several sources (no single original paper).
    AiArticle,
    /// A news story (one combined article built from many outlets).
    News,
}

impl ContentKind {
    pub fn badge(self) -> &'static str {
        match self {
            ContentKind::FreeFullText => "Free full text",
            ContentKind::AbstractOnly => "Abstract only – full text AI-assisted",
            ContentKind::AiArticle => "AI article",
            ContentKind::News => "News",
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            ContentKind::FreeFullText => "free_full_text",
            ContentKind::AbstractOnly => "abstract_only",
            ContentKind::AiArticle => "ai_article",
            ContentKind::News => "news",
        }
    }
    pub fn parse(s: &str) -> Self {
        match s {
            "free_full_text" => ContentKind::FreeFullText,
            "ai_article" => ContentKind::AiArticle,
            "news" => ContentKind::News,
            _ => ContentKind::AbstractOnly,
        }
    }
}

/// A real image with its source and licence. Copyrighted images are link-only
/// (`embeddable == false`): the UI shows a link card instead of the picture.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct ImageRef {
    /// Direct URL of the image file (full size).
    pub url: String,
    /// Smaller rendition for cards, when the host offers one.
    pub thumb_url: Option<String>,
    /// The page describing the image (e.g. the Wikimedia Commons file page, or the paper's figure page).
    pub page_url: String,
    pub title: String,
    /// Author / credit line as the source states it.
    pub credit: String,
    /// Licence short name, e.g. "CC BY-SA 4.0", "Public domain".
    pub licence: String,
    pub licence_url: Option<String>,
    /// True when the image is a figure of the paper itself; false when it is the closest
    /// relevant image found elsewhere ("may not be from the paper").
    pub from_paper: bool,
    /// False for copyrighted images: never embed, show as a link only.
    pub embeddable: bool,
}

/// A paper or a news article, as found by the research layer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct Paper {
    /// Stable key: "doi:10.x/y" (lowercase), else "arxiv:2401.01234", else "openalex:W123",
    /// else "s2:<id>", else "url:<sha256 prefix>". Use [`Paper::make_key`].
    pub key: String,
    pub doi: Option<String>,
    pub title: String,
    pub abstract_text: Option<String>,
    pub authors: Vec<String>,
    pub year: Option<i32>,
    pub date: Option<NaiveDate>,
    /// Journal / conference / outlet name.
    pub venue: Option<String>,
    pub publisher: Option<String>,
    /// Landing page of the record (publisher page, arXiv abs page, news article URL).
    pub url: Option<String>,
    /// Best legal free full-text location (HTML or PDF), from Unpaywall / arXiv / CORE / Europe PMC.
    pub oa_url: Option<String>,
    /// Unpaywall-style status: "gold", "green", "hybrid", "bronze", "closed", ...
    pub oa_status: Option<String>,
    /// Version of the free text: "publishedVersion", "acceptedVersion", "submittedVersion" (preprint).
    pub oa_version: Option<String>,
    pub oa_licence: Option<String>,
    pub cited_by_count: Option<u32>,
    pub subjects: Vec<String>,
    pub openalex_id: Option<String>,
    pub s2_id: Option<String>,
    pub arxiv_id: Option<String>,
    pub pmcid: Option<String>,
    /// Which APIs returned this record ("openalex", "semanticscholar", "arxiv", "crossref", "core", "gdelt", "rss", ...).
    pub found_in: Vec<String>,
    pub image: Option<ImageRef>,
    pub kind: ContentKind,
    /// Language code when known (news).
    pub language: Option<String>,
}

impl Paper {
    /// Build the stable key from the best identifier available.
    pub fn make_key(&self) -> String {
        if let Some(doi) = &self.doi {
            return format!("doi:{}", normalize_doi(doi));
        }
        if let Some(a) = &self.arxiv_id {
            return format!("arxiv:{}", a.trim());
        }
        if let Some(o) = &self.openalex_id {
            return format!("openalex:{}", o.rsplit('/').next().unwrap_or(o));
        }
        if let Some(s) = &self.s2_id {
            return format!("s2:{s}");
        }
        let basis = self.url.clone().unwrap_or_else(|| self.title.to_lowercase());
        format!("url:{}", short_hash(&basis))
    }

    pub fn has_free_full_text(&self) -> bool {
        self.oa_url.is_some()
    }

    /// "Smith, J., Doe, A. and Lee, K." style author line for cards.
    pub fn author_line(&self, max: usize) -> String {
        if self.authors.is_empty() {
            return self.venue.clone().unwrap_or_default();
        }
        let mut names: Vec<&str> = self.authors.iter().take(max).map(String::as_str).collect();
        let more = self.authors.len() > max;
        let mut s = match names.len() {
            1 => names[0].to_string(),
            _ => {
                let last = names.pop().unwrap_or_default();
                format!("{} and {}", names.join(", "), last)
            }
        };
        if more {
            s = format!("{}, et al.", self.authors[..max].join(", "));
        }
        s
    }
}

pub fn normalize_doi(doi: &str) -> String {
    let d = doi.trim().to_lowercase();
    for p in ["https://doi.org/", "http://doi.org/", "https://dx.doi.org/", "http://dx.doi.org/", "doi:"] {
        if let Some(rest) = d.strip_prefix(p) {
            return rest.to_string();
        }
    }
    d
}

pub fn short_hash(s: &str) -> String {
    use sha2::{Digest, Sha256};
    let h = Sha256::digest(s.as_bytes());
    hex::encode(&h[..8])
}

// ---------------------------------------------------------------------------------------------
// Sources and chunks (what the research layer hands to the model)
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum SourceKind {
    /// The paper the article is about.
    Target,
    /// A related paper.
    #[default]
    Paper,
    /// A paper that cites the target; its chunks are the sentences describing the target.
    Citing,
    /// A news article.
    News,
    /// Another open web source (e.g. Wikipedia, an agency page).
    Web,
}

/// One numbered source an article may cite. `id` is the number used in the text: "[3]".
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct Source {
    pub id: u32,
    pub kind: SourceKind,
    /// The Paper key when the source is a paper or news item known to the app (links to it in-app).
    pub paper_key: Option<String>,
    pub title: String,
    pub authors: Vec<String>,
    pub year: Option<i32>,
    pub venue: Option<String>,
    pub publisher: Option<String>,
    pub url: Option<String>,
    pub doi: Option<String>,
    /// Date the source was retrieved (Harvard "Accessed" date for web sources).
    pub accessed: Option<NaiveDate>,
    /// Date published (news).
    pub date: Option<NaiveDate>,
}

impl Source {
    pub fn from_paper(id: u32, kind: SourceKind, p: &Paper) -> Self {
        Source {
            id,
            kind,
            paper_key: Some(p.key.clone()),
            title: p.title.clone(),
            authors: p.authors.clone(),
            year: p.year,
            venue: p.venue.clone(),
            publisher: p.publisher.clone(),
            url: p.url.clone().or_else(|| p.doi.as_ref().map(|d| format!("https://doi.org/{d}"))),
            doi: p.doi.clone(),
            accessed: Some(Utc::now().date_naive()),
            date: p.date,
        }
    }
    pub fn link(&self) -> Option<String> {
        self.doi.as_ref().map(|d| format!("https://doi.org/{d}")).or_else(|| self.url.clone())
    }
}

/// A passage of retrieved text, tagged with the source it came from.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct Chunk {
    pub source_id: u32,
    pub text: String,
    /// Section heading of the source the chunk came from, when known ("Methods", "Abstract").
    pub section: Option<String>,
    /// BM25 (or later embedding) relevance score; higher is better.
    pub score: f32,
}

/// Everything the research layer gathered for one piece of writing.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct SourcePack {
    pub sources: Vec<Source>,
    /// Ranked best first.
    pub chunks: Vec<Chunk>,
    /// The legal free full text of the target, split into its original paragraphs, when one
    /// exists. The writing pipeline lightly rewords these (origin = Reworded / Original).
    pub full_text: Option<FullText>,
    /// Images found for the article (real, licensed).
    pub images: Vec<ImageRef>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct FullText {
    /// Where it was read from.
    pub url: String,
    pub licence: Option<String>,
    /// "publishedVersion", "acceptedVersion", "submittedVersion".
    pub version: Option<String>,
    pub sections: Vec<FullTextSection>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct FullTextSection {
    pub heading: String,
    pub paragraphs: Vec<String>,
}

// ---------------------------------------------------------------------------------------------
// Articles (what the writing pipeline produces and the reader shows)
// ---------------------------------------------------------------------------------------------

/// Where a passage of text came from. Stored for every passage, drives all styling and export.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum Origin {
    /// Verbatim from the source. Normal style.
    Original,
    /// The original, lightly reworded by the AI for flow. Underlined; the original wording is kept.
    Reworded,
    /// Written by the AI. Distinct colour plus a margin bar.
    #[default]
    AiWritten,
}

impl Origin {
    pub fn label(self) -> &'static str {
        match self {
            Origin::Original => "Original text",
            Origin::Reworded => "AI-reworded original",
            Origin::AiWritten => "AI-written",
        }
    }
}

/// One span of text (typically a sentence) with a single origin.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct Passage {
    pub text: String,
    pub origin: Origin,
    /// Source numbers cited by this passage (all valid: they map to `Article::sources`).
    pub citations: Vec<u32>,
    /// For `Reworded`: the original wording.
    pub original: Option<String>,
    /// A factual claim with no valid citation. Shown with an "unverified" mark.
    pub unverified: bool,
}

impl Passage {
    pub fn ai(text: impl Into<String>, citations: Vec<u32>) -> Self {
        Passage { text: text.into(), origin: Origin::AiWritten, citations, original: None, unverified: false }
    }
    pub fn original(text: impl Into<String>) -> Self {
        Passage { text: text.into(), origin: Origin::Original, citations: vec![], original: None, unverified: false }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Block {
    Paragraph(Vec<Passage>),
    /// A sub-heading inside a section.
    Heading(String),
    Chart(ChartSpec),
    Image(ImageRef),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct Section {
    pub heading: String,
    pub blocks: Vec<Block>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ArticleKind {
    /// Only the abstract was free: the AI's own interpretation of the topic.
    /// Labelled "AI interpretation – not the original paper".
    #[default]
    Interpretation,
    /// A legal free full text, shown with limited AI rewording for flow.
    FullText,
    /// Written entirely by the AI from several sources (Discover "AI article").
    AiOnly,
    /// A combined news story built from 10–20 outlets.
    News,
    /// Learn AI: a plain-language explanation for non-specialists of an abstract or article.
    Learn,
}

impl ArticleKind {
    pub fn label(self) -> &'static str {
        match self {
            ArticleKind::Interpretation => "AI interpretation – not the original paper",
            ArticleKind::FullText => "Original full text – lightly reworded for flow",
            ArticleKind::AiOnly => "AI article – written from the sources listed",
            ArticleKind::News => "AI news summary – combined from the outlets listed",
            ArticleKind::Learn => "Learn AI – a plain-language explanation written by the AI",
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            ArticleKind::Interpretation => "interpretation",
            ArticleKind::FullText => "full_text",
            ArticleKind::AiOnly => "ai_only",
            ArticleKind::News => "news",
            ArticleKind::Learn => "learn",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct Article {
    pub id: String,
    pub kind: ArticleKind,
    /// The paper this article is about (Interpretation / FullText), or the feed item key.
    pub paper_key: Option<String>,
    /// Our own title. For interpretations this is never the original title presented as
    /// the authors' work; it reads like "AI interpretation: <topic>".
    pub title: String,
    pub standfirst: Option<String>,
    /// The original paper's title, authors and link, always shown for Interpretation/FullText.
    pub original_title: Option<String>,
    pub original_authors: Vec<String>,
    pub original_url: Option<String>,
    /// Byline for AI-written content: "Gateless Scholar + Claude".
    pub byline: String,
    /// Provider and model used, e.g. "Claude Code CLI (sonnet)".
    pub generated_by: String,
    pub created: DateTime<Utc>,
    pub sections: Vec<Section>,
    pub sources: Vec<Source>,
    pub hero_image: Option<ImageRef>,
    /// Validation notes: dropped invalid citations, unverified sentences, rejected charts.
    pub validation: ValidationReport,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct ValidationReport {
    pub invalid_citations_removed: u32,
    pub unverified_sentences: u32,
    pub charts_rejected: u32,
    pub rewordings_reverted: u32,
    pub notes: Vec<String>,
}

impl Article {
    pub fn passages(&self) -> impl Iterator<Item = &Passage> {
        self.sections.iter().flat_map(|s| s.blocks.iter()).flat_map(|b| match b {
            Block::Paragraph(p) => p.iter().collect::<Vec<_>>(),
            _ => vec![],
        })
    }
    pub fn plain_text(&self) -> String {
        let mut out = String::new();
        for s in &self.sections {
            for b in &s.blocks {
                if let Block::Paragraph(ps) = b {
                    for p in ps {
                        out.push_str(&p.text);
                        out.push(' ');
                    }
                }
            }
        }
        out
    }
}

// ---------------------------------------------------------------------------------------------
// Charts (the AI emits a small spec, the app renders it in its own style)
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChartKind {
    Bar,
    Line,
    Venn,
    Stock,
    Flow,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChartSpec {
    pub kind: ChartKind,
    pub title: String,
    #[serde(default)]
    pub x_label: Option<String>,
    #[serde(default)]
    pub y_label: Option<String>,
    /// Bar / line / stock: series of labelled points.
    #[serde(default)]
    pub series: Vec<ChartSeries>,
    /// Venn: 2–3 sets with sizes, plus overlaps.
    #[serde(default)]
    pub sets: Vec<VennSet>,
    /// Flow diagram: nodes and edges.
    #[serde(default)]
    pub nodes: Vec<String>,
    #[serde(default)]
    pub edges: Vec<(usize, usize, String)>,
    /// Sources the numbers come from (validated to exist and to contain the numbers).
    #[serde(default)]
    pub citations: Vec<u32>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChartSeries {
    pub name: String,
    /// (x label, y value)
    pub points: Vec<(String, f64)>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VennSet {
    pub label: String,
    pub size: f64,
    /// Overlap with other sets by index: (other index, overlap size).
    #[serde(default)]
    pub overlaps: Vec<(usize, f64)>,
}

// ---------------------------------------------------------------------------------------------
// Feed
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum FeedKind {
    #[default]
    Research,
    News,
}

impl FeedKind {
    pub fn as_str(self) -> &'static str {
        match self {
            FeedKind::Research => "research",
            FeedKind::News => "news",
        }
    }
    pub fn parse(s: &str) -> Self {
        if s == "news" { FeedKind::News } else { FeedKind::Research }
    }
}

/// One card in Discover. Headline and short abstract are generated daily; the full article
/// only when opened or added to Read Later.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct FeedItem {
    pub id: String,
    pub day: NaiveDate,
    pub feed: FeedKind,
    pub tab: String,
    pub kind: ContentKind,
    /// Real papers: the original title. AI articles and news: the AI-written headline.
    pub headline: String,
    /// Real papers: the original abstract. AI articles / news: an AI-written short summary.
    pub summary: String,
    /// True when `summary` (and headline for AI items) was written by the AI.
    pub summary_ai: bool,
    /// Real paper the card is about, if any.
    pub paper: Option<Paper>,
    /// For AI articles / news: the papers or news articles the story will be written from.
    pub basis: Vec<Paper>,
    pub image: Option<ImageRef>,
    pub created: DateTime<Utc>,
}

impl FeedItem {
    pub fn author_line(&self, provider_name: &str) -> String {
        match (&self.paper, self.kind) {
            (Some(p), ContentKind::FreeFullText | ContentKind::AbstractOnly) => p.author_line(4),
            _ => format!("Gateless Scholar + {provider_name}"),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Search
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum SearchScope {
    #[default]
    Journals,
    News,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum SortBy {
    #[default]
    Relevance,
    Newest,
    MostCited,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct SearchFilters {
    pub from_year: Option<i32>,
    pub to_year: Option<i32>,
    pub free_full_text_only: bool,
    /// Empty = all kinds.
    pub kinds: Vec<ContentKind>,
    pub subject: Option<String>,
    pub sort: SortBy,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct SearchQuery {
    pub text: String,
    pub scope: SearchScope,
    pub filters: SearchFilters,
    /// 0-based page for infinite scroll.
    pub page: u32,
    pub per_page: u32,
}

// ---------------------------------------------------------------------------------------------
// Notes, projects, bookshelves
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct Note {
    pub id: i64,
    pub project_id: Option<i64>,
    pub title: String,
    /// Plain text with links written as `[[article:<id>|Title]]` or `[[paper:<key>|Title]]`.
    pub body: String,
    /// Quoted passages keep their origin marking.
    pub quotes: Vec<NoteQuote>,
    pub created: DateTime<Utc>,
    pub updated: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct NoteQuote {
    pub article_id: Option<String>,
    pub paper_key: Option<String>,
    pub passages: Vec<Passage>,
    /// The references the quoted AI text relied on (shown with the "rewrite in your own words" prompt).
    pub references: Vec<Source>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct Project {
    pub id: i64,
    pub name: String,
    pub description: String,
    pub created: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct Bookshelf {
    pub id: i64,
    pub name: String,
    pub project_id: Option<i64>,
}

/// Saved-item lists that are kept permanently.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SavedList {
    Favourites,
    ReadLater,
    Bookshelf(i64),
    Project(i64),
    /// Linked from a note: kept permanently.
    NoteLink,
}

impl SavedList {
    pub fn key(self) -> String {
        match self {
            SavedList::Favourites => "fav".into(),
            SavedList::ReadLater => "later".into(),
            SavedList::Bookshelf(id) => format!("shelf:{id}"),
            SavedList::Project(id) => format!("project:{id}"),
            SavedList::NoteLink => "note".into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct Notification {
    pub id: i64,
    pub title: String,
    pub body: String,
    pub paper: Option<Paper>,
    pub created: DateTime<Utc>,
    pub read: bool,
}

/// A topic or author the user follows for notifications.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Follow {
    Topic(String),
    Author(String),
}

// ---------------------------------------------------------------------------------------------
// Documents: anything that can be opened in the reader, saved or queued
// ---------------------------------------------------------------------------------------------

/// Something the reader can open: a real paper (search result or feed card), or an AI/news
/// feed item. Saved lists, history and the generation queue all store a `Doc` snapshot.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Doc {
    Paper(Paper),
    Feed(FeedItem),
}

impl Doc {
    /// Stable key: the paper key for papers (also for feed cards about a real paper, so a
    /// paper opened from search and from Discover shares one article), "feed:<id>" otherwise.
    pub fn key(&self) -> String {
        match self {
            Doc::Paper(p) => p.key.clone(),
            Doc::Feed(f) => match &f.paper {
                Some(p) if matches!(f.kind, ContentKind::FreeFullText | ContentKind::AbstractOnly) => p.key.clone(),
                _ => format!("feed:{}", f.id),
            },
        }
    }
    pub fn title(&self) -> &str {
        match self {
            Doc::Paper(p) => &p.title,
            Doc::Feed(f) => &f.headline,
        }
    }
    pub fn kind(&self) -> ContentKind {
        match self {
            Doc::Paper(p) => p.kind,
            Doc::Feed(f) => f.kind,
        }
    }
    pub fn paper(&self) -> Option<&Paper> {
        match self {
            Doc::Paper(p) => Some(p),
            Doc::Feed(f) => f.paper.as_ref(),
        }
    }
    pub fn abstract_text(&self) -> String {
        match self {
            Doc::Paper(p) => p.abstract_text.clone().unwrap_or_default(),
            Doc::Feed(f) => f.summary.clone(),
        }
    }
    pub fn image(&self) -> Option<&ImageRef> {
        match self {
            Doc::Paper(p) => p.image.as_ref(),
            Doc::Feed(f) => f.image.as_ref().or(f.paper.as_ref().and_then(|p| p.image.as_ref())),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum CitationStyle {
    #[default]
    Harvard,
    Apa,
    Vancouver,
}

impl CitationStyle {
    pub const ALL: [CitationStyle; 3] = [CitationStyle::Harvard, CitationStyle::Apa, CitationStyle::Vancouver];
    pub fn label(self) -> &'static str {
        match self {
            CitationStyle::Harvard => "Harvard",
            CitationStyle::Apa => "APA 7",
            CitationStyle::Vancouver => "Vancouver",
        }
    }
}

/// An Ask-panel answer: passages (with origin and citations) plus the sources they cite.
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct Answer {
    pub passages: Vec<Passage>,
    pub sources: Vec<Source>,
    pub generated_by: String,
}

