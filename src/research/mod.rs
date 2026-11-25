//! The research layer: code does the research, not the model. Even a small local model then
//! only has to write from the numbered sources it is handed.
//!
//! Pipeline: query expansion → multi-API search → deduplicate by DOI/URL → find legal full
//! text → fetch citing papers and extract the sentences describing the target → chunk →
//! rank (BM25) → top chunks with source ids.
//!
//! Only legally available content: open APIs, open-access copies (Unpaywall, arXiv, Europe
//! PMC, CORE). No bypassing logins, no shadow libraries.
//!
//! INTERFACE FILE: `Research`'s public methods are what the app calls.

mod arxiv;
mod bm25;
mod crossref;
mod dedupe;
mod fulltext;
mod http;
mod news;
mod openalex;
mod semantic_scholar;
mod text;
mod wikimedia;

#[cfg(test)]
mod live_tests;

use std::collections::HashSet;
use std::future::Future;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use anyhow::Result;
use chrono::{Datelike, NaiveDate, Utc};
use futures::StreamExt;
use tokio::sync::mpsc::UnboundedSender;

use crate::db::Db;
use crate::model::*;

pub use wikimedia::is_free_licence;

use self::bm25::bm25_scores;
use self::dedupe::{dedupe, merge_into};
use self::news::{cluster_titles, domain_of, gdelt_query, link_only_image, same_event};
use self::openalex::OaSearch;
use self::text::{RawChunk, chunk_sections, chunk_text, expand_queries, html_paragraphs, keywords, normalize_title, token_set};

/// Progress reported while gathering, shown in the reader ("Finding sources… reading 8 papers…").
#[derive(Clone, Debug)]
pub struct Progress {
    pub text: String,
    /// 0.0..=1.0 of the research phase.
    pub fraction: f32,
}

/// Configuration the app passes in (from settings + keyring).
#[derive(Clone, Debug, Default)]
pub struct ResearchConfig {
    /// Contact email for polite pools (OpenAlex `mailto`, Crossref UA, Unpaywall `email`).
    /// Empty = not sent (Unpaywall then uses a generic placeholder it accepts).
    pub contact_email: String,
    /// CORE API key (optional; CORE is skipped without one).
    pub core_api_key: Option<String>,
    /// Semantic Scholar API key (optional; the public rate limit applies without one).
    pub s2_api_key: Option<String>,
    /// Outlet RSS feeds used for news alongside GDELT.
    pub news_feeds: Vec<String>,
}

/// A group of news articles from different outlets about one event.
#[derive(Clone, Debug, Default)]
pub struct NewsCluster {
    /// Representative title (from the outlets, not AI).
    pub title: String,
    pub articles: Vec<Paper>,
    pub image: Option<ImageRef>,
}

#[derive(Clone)]
pub struct Research {
    pub db: Db,
    pub config: ResearchConfig,
}

/// Cap on chunks handed to the model.
const MAX_CHUNKS: usize = 40;

/// Send a progress update (ignored when the receiver is gone).
fn report(tx: &UnboundedSender<Progress>, text: impl Into<String>, fraction: f32) {
    let _ = tx.send(Progress { text: text.into(), fraction: fraction.clamp(0.0, 1.0) });
}

/// Log and swallow a failing source so one API never fails the whole gather.
fn or_warn<T: Default>(what: &str, r: Result<T>) -> T {
    match r {
        Ok(v) => v,
        Err(e) => {
            log::warn!("research: {what} failed: {e:#}");
            T::default()
        }
    }
}

/// Run a fallible future with a deadline; a timeout becomes an error.
async fn deadline_r<T>(what: &str, secs: u64, f: impl Future<Output = Result<T>>) -> Result<T> {
    match tokio::time::timeout(Duration::from_secs(secs), f).await {
        Ok(v) => v,
        Err(_) => Err(anyhow::anyhow!("{what} timed out after {secs}s")),
    }
}

/// Run a future with a deadline; on timeout log and return the default.
async fn deadline<T: Default>(what: &str, secs: u64, f: impl Future<Output = T>) -> T {
    match tokio::time::timeout(Duration::from_secs(secs), f).await {
        Ok(v) => v,
        Err(_) => {
            log::warn!("research: {what} timed out after {secs}s");
            T::default()
        }
    }
}

fn today() -> NaiveDate {
    Utc::now().date_naive()
}

/// Search queries for a Discover tab / topic name. Broad tabs map to a few concrete queries.
pub fn topic_queries(topic: &str) -> Vec<String> {
    let t = topic.trim().to_lowercase();
    let v: &[&str] = match t.as_str() {
        "" | "for you" | "top" | "all" | "home" | "discover" | "science breakthrough" => &["machine learning", "climate change", "cancer therapy", "exoplanet", "quantum computing", "neuroscience brain"],
        "tech" | "technology" => &["artificial intelligence", "semiconductor", "robotics", "software engineering"],
        "ai" | "artificial intelligence" | "machine learning" => &["large language models", "deep learning", "reinforcement learning"],
        // No "space exploration": it matches the CS phrase "design space exploration".
        "space" => &["spacecraft", "exoplanet", "astronaut", "solar system", "planetary science", "space telescope"],
        "astrophysics" | "astronomy" => &["black hole", "galaxy formation", "cosmology", "gravitational waves"],
        "computer vision" => &["computer vision", "image segmentation", "object detection", "visual recognition"],
        "chemistry" => &["catalysis", "organic synthesis", "electrochemistry", "materials chemistry"],
        // Specific terms: bare "evolution" / "genome" pull in machine-learning papers.
        "biology" => &["cell biology", "gene expression", "evolutionary biology", "microbiome", "protein structure", "species ecology"],
        "physics" => &["quantum physics", "condensed matter", "particle physics"],
        "medicine" | "health" => &["clinical trial", "cancer therapy", "infectious disease", "public health"],
        "climate" | "environment" | "earth" => &["climate change", "biodiversity", "ocean warming"],
        "neuroscience" => &["neuroscience", "brain imaging", "neurons"],
        _ => &[],
    };
    if v.is_empty() { vec![topic.trim().to_string()] } else { v.iter().map(|s| s.to_string()).collect() }
}

/// Words too general to show that a picture is about the same thing.
fn is_generic_word(w: &str) -> bool {
    const GENERIC: &[&str] = &[
        "theori", "theory", "new", "study", "studi", "system", "model", "data", "state", "ai", "research", "scienc", "science", "method", "effect", "use",
        "high", "low", "first", "world", "human", "life", "structur", "structure", "power", "base", "base", "time", "chang", "change", "group", "process",
    ];
    GENERIC.contains(&w)
}

/// Wikipedia articles whose lead images make good general photos for a topic tab
/// (general articles often lead with a diagram; these lead with a photo).
pub fn topic_photo_queries(topic: &str) -> Vec<String> {
    let t = topic.trim().to_lowercase();
    let v: &[&str] = match t.as_str() {
        "space" => &["Outer space", "Space exploration", "International Space Station", "Hubble Space Telescope"],
        "astrophysics" | "astronomy" => &["Galaxy", "Astrophysics", "Nebula", "Andromeda Galaxy"],
        "biology" => &["Cell (biology)", "Molecular biology", "Microscope", "Biodiversity"],
        "chemistry" => &["Laboratory glassware", "Solution (chemistry)"],
        "tech" | "technology" => &["Integrated circuit", "Computer"],
        "computer vision" => &["Computer vision", "Digital camera"],
        "climate" | "environment" => &["Climate change", "Earth"],
        "medicine" | "health" => &["Medicine", "Hospital"],
        "physics" => &["Particle accelerator", "Physics"],
        "neuroscience" => &["Neuron", "Human brain"],
        "" | "for you" | "science" => &["Laboratory glassware", "Microscope", "Hubble Space Telescope", "DNA", "Particle accelerator"],
        _ => &[],
    };
    let mut out: Vec<String> = v.iter().map(|s| s.to_string()).collect();
    if !t.is_empty() && t != "for you" {
        out.push(topic.trim().to_string());
    }
    out.push("Laboratory glassware".into());
    out
}

/// "(a b) OR (c d)" for OpenAlex's boolean search.
fn or_query(queries: &[String]) -> String {
    match queries.len() {
        0 => String::new(),
        1 => queries[0].clone(),
        _ => queries.iter().map(|q| format!("({})", q.replace(['(', ')', '"'], " ").trim())).collect::<Vec<_>>().join(" OR "),
    }
}

/// GDELT query for a news topic tab.
fn news_topic_query(topic: &str) -> String {
    let t = topic.trim().to_lowercase();
    let q = match t.as_str() {
        "" | "for you" | "top" | "all" | "home" | "discover" | "science breakthrough" => "(science OR research OR scientists OR researchers)",
        "tech" | "technology" => "(technology OR \"artificial intelligence\" OR software OR chip)",
        "ai" | "artificial intelligence" => "(\"artificial intelligence\" OR chatbot OR \"machine learning\")",
        "space" => "(NASA OR spacecraft OR astronauts OR \"space station\" OR rocket)",
        "astrophysics" | "astronomy" => "(astronomers OR telescope OR galaxy OR \"black hole\")",
        "computer vision" => "(\"computer vision\" OR \"facial recognition\" OR \"image recognition\")",
        "chemistry" => "(chemists OR chemistry OR molecule)",
        "biology" => "(biologists OR biology OR species OR genome)",
        "physics" => "(physicists OR physics OR quantum)",
        "medicine" | "health" => "(medical OR health OR disease OR vaccine)",
        "climate" | "environment" => "(climate OR emissions OR biodiversity)",
        _ => "",
    };
    if !q.is_empty() {
        return format!("{q} sourcelang:english");
    }
    let words: Vec<&str> = topic.split_whitespace().collect();
    if words.len() > 1 && words.len() <= 3 { format!("\"{}\" sourcelang:english", topic.trim()) } else { gdelt_query(topic, 4) }
}

/// Tokens that mark an RSS item as on-topic (None = accept everything).
fn rss_topic_terms(topic: &str) -> Option<HashSet<String>> {
    let t = topic.trim().to_lowercase();
    if matches!(t.as_str(), "" | "for you" | "top" | "all" | "home" | "discover" | "science breakthrough") {
        return None;
    }
    let mut set: HashSet<String> = topic_queries(topic).iter().flat_map(|q| token_set(q)).collect();
    set.extend(token_set(topic));
    // News words for the common tabs.
    let extra: &[&str] = match t.as_str() {
        "space" => &["nasa", "rocket", "astronaut", "moon", "mars", "orbit", "satellite", "esa", "spacex"],
        "tech" | "technology" => &["ai", "chip", "software", "apple", "google", "microsoft", "robot", "app"],
        "astrophysics" | "astronomy" => &["telescope", "astronomer", "star", "galaxy", "webb", "supernova"],
        "biology" => &["species", "gene", "cell", "dna", "animal"],
        "chemistry" => &["chemist", "molecule", "chemical", "battery"],
        "medicine" | "health" => &["patient", "drug", "vaccine", "disease", "hospital"],
        _ => &[],
    };
    set.extend(extra.iter().map(|s| text::stem(s)));
    Some(set)
}

fn rss_matches(p: &Paper, terms: &HashSet<String>) -> bool {
    let t = token_set(&format!("{} {}", p.title, p.abstract_text.as_deref().unwrap_or("")));
    t.iter().any(|x| terms.contains(x))
}

/// Dedupe news by URL and near-identical titles (keeps the first).
fn dedupe_news(items: Vec<Paper>) -> Vec<Paper> {
    let mut urls = HashSet::new();
    let mut kept: Vec<(HashSet<String>, Paper)> = Vec::new();
    for p in items {
        let Some(u) = p.url.clone() else { continue };
        let norm = u.split('#').next().unwrap_or(&u).trim_end_matches('/').replace("://www.", "://").replace("http://", "https://");
        if !urls.insert(norm) {
            continue;
        }
        let ts = token_set(&p.title);
        if ts.is_empty() || kept.iter().any(|(k, _)| text::jaccard(k, &ts) >= 0.8) {
            continue;
        }
        kept.push((ts, p));
    }
    kept.into_iter().map(|x| x.1).collect()
}

/// Order items so outlets alternate (round-robin by domain), at most `per_outlet` each.
fn diversify(items: Vec<Paper>, per_outlet: usize) -> Vec<Paper> {
    let mut groups: Vec<(String, Vec<Paper>)> = Vec::new();
    for p in items {
        let d = p.url.as_deref().map(domain_of).unwrap_or_default();
        match groups.iter_mut().find(|g| g.0 == d) {
            Some(g) => g.1.push(p),
            None => groups.push((d, vec![p])),
        }
    }
    let mut out = Vec::new();
    for round in 0..per_outlet {
        for g in groups.iter_mut() {
            if round < g.1.len() {
                out.push(std::mem::take(&mut g.1[round]));
            }
        }
    }
    out
}

/// One numbered source and its candidate chunks (pinned chunks are always kept).
struct Entry {
    source: Source,
    chunks: Vec<(RawChunk, bool)>,
    /// Most chunks this source may contribute.
    cap: usize,
}

/// Score all chunks against `query`, keep the best per source plus the best overall (≤ cap),
/// and return the pack's sources and chunks.
fn assemble(query: &str, entries: Vec<Entry>, cap: usize) -> (Vec<Source>, Vec<Chunk>) {
    let mut flat: Vec<(u32, RawChunk, bool, usize)> = Vec::new();
    for e in &entries {
        for (c, pinned) in &e.chunks {
            flat.push((e.source.id, c.clone(), *pinned, e.cap));
        }
    }
    let docs: Vec<String> = flat.iter().map(|(_, c, _, _)| format!("{} {}", c.section.as_deref().unwrap_or(""), c.text)).collect();
    let refs: Vec<&str> = docs.iter().map(String::as_str).collect();
    let scores = bm25_scores(query, &refs);
    let mut order: Vec<usize> = (0..flat.len()).collect();
    order.sort_by(|&a, &b| scores[b].total_cmp(&scores[a]).then(a.cmp(&b)));

    let mut chosen: Vec<usize> = Vec::new();
    let mut per_source: std::collections::HashMap<u32, usize> = Default::default();
    let sets: Vec<HashSet<String>> = flat.iter().map(|(_, c, _, _)| token_set(&c.text)).collect();
    let take = |i: usize, chosen: &mut Vec<usize>, per_source: &mut std::collections::HashMap<u32, usize>| {
        // Skip near-duplicates (e.g. an abstract repeated at the top of the full text).
        if !chosen.contains(&i) && !chosen.iter().any(|&j| flat[j].0 == flat[i].0 && text::jaccard(&sets[i], &sets[j]) >= 0.8) {
            chosen.push(i);
            *per_source.entry(flat[i].0).or_insert(0) += 1;
        }
    };
    // Pinned first, then the best chunk of every source, then the rest by score.
    for &i in &order {
        if flat[i].2 {
            take(i, &mut chosen, &mut per_source);
        }
    }
    for &i in &order {
        if chosen.len() >= cap {
            break;
        }
        if !per_source.contains_key(&flat[i].0) {
            take(i, &mut chosen, &mut per_source);
        }
    }
    for &i in &order {
        if chosen.len() >= cap {
            break;
        }
        let n = per_source.get(&flat[i].0).copied().unwrap_or(0);
        if n < flat[i].3 {
            take(i, &mut chosen, &mut per_source);
        }
    }
    chosen.sort_by(|&a, &b| scores[b].total_cmp(&scores[a]).then(a.cmp(&b)));
    let chunks = chosen
        .into_iter()
        .map(|i| Chunk { source_id: flat[i].0, text: flat[i].1.text.clone(), section: flat[i].1.section.clone(), score: scores[i] })
        .collect();
    (entries.into_iter().map(|e| e.source).collect(), chunks)
}

fn abstract_chunks(p: &Paper) -> Vec<RawChunk> {
    match p.abstract_text.as_deref().filter(|a| !a.trim().is_empty()) {
        Some(a) => chunk_text(Some("Abstract"), a),
        None => vec![],
    }
}

fn paper_doc(p: &Paper) -> String {
    format!("{} {} {}", p.title, p.title, p.abstract_text.as_deref().unwrap_or(""))
}

fn year_ok(p: &Paper, from: Option<i32>, to: Option<i32>) -> bool {
    let y = p.year.or(p.date.map(|d| d.year()));
    match y {
        Some(y) => from.is_none_or(|f| y >= f) && to.is_none_or(|t| y <= t),
        None => from.is_none() && to.is_none(),
    }
}

impl Research {
    pub fn new(db: Db, config: ResearchConfig) -> Self {
        Research { db, config }
    }

    /// Scholar-mode search (journals) or news search, with filters and paging.
    /// Merges OpenAlex, Semantic Scholar, arXiv, Crossref (and CORE when keyed), dedupes by
    /// DOI/arXiv id/normalised title, applies filters and sort. Marks `kind` FreeFullText
    /// when a legal free copy is known, AbstractOnly otherwise. News scope uses GDELT + RSS.
    pub async fn search(&self, q: &SearchQuery) -> Result<Vec<Paper>> {
        let f = &q.filters;
        let wants_news = q.scope == SearchScope::News || (!f.kinds.is_empty() && f.kinds.iter().all(|k| *k == ContentKind::News));
        if wants_news {
            return self.search_news(q).await;
        }
        if !f.kinds.is_empty() && !f.kinds.iter().any(|k| matches!(k, ContentKind::FreeFullText | ContentKind::AbstractOnly)) {
            return Ok(vec![]); // AI articles are not searchable papers
        }
        let text = match f.subject.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            Some(s) => format!("{} {s}", q.text.trim()),
            None => q.text.trim().to_string(),
        };
        if text.is_empty() {
            return Ok(vec![]);
        }
        let per_page = if q.per_page == 0 { 20 } else { q.per_page.min(100) };
        let half = (per_page / 2).max(5);
        let oa_only = f.free_full_text_only || (f.kinds.len() == 1 && f.kinds[0] == ContentKind::FreeFullText);
        let years = (f.from_year, f.to_year);
        let from = f.from_year.and_then(|y| NaiveDate::from_ymd_opt(y, 1, 1));
        let to = f.to_year.and_then(|y| NaiveDate::from_ymd_opt(y, 12, 31));

        let oa_q = OaSearch { query: &text, from, to, oa_only, sort: f.sort, page: q.page, per_page, ..Default::default() };
        let oa = self.openalex_search(&oa_q);
        let s2 = self.s2_search(&text, q.page * half, half, Some(years), oa_only);
        let ax = async {
            // arXiv has no citation counts; skip it for "most cited" and for ranges before it existed.
            if f.sort == SortBy::MostCited || f.to_year.is_some_and(|y| y < 1992) {
                Ok(vec![])
            } else {
                self.arxiv_search(&text, q.page * half, half, f.sort == SortBy::Newest).await
            }
        };
        let cr = async {
            if oa_only { Ok(vec![]) } else { self.crossref_search(&text, q.page * half, half, years, f.sort, false).await }
        };
        let core = self.core_search(&text, q.page * half, half);
        let (oa, s2, ax, cr, core) = tokio::join!(
            deadline_r("OpenAlex search", 25, oa),
            deadline_r("Semantic Scholar search", 25, s2),
            deadline_r("arXiv search", 25, ax),
            deadline_r("Crossref search", 25, cr),
            deadline_r("CORE search", 25, core)
        );
        let results = [("OpenAlex", oa), ("Semantic Scholar", s2), ("arXiv", ax), ("Crossref", cr), ("CORE", core)];
        let mut all = Vec::new();
        let mut errors = Vec::new();
        let mut any_ok = false;
        for (name, r) in results {
            match r {
                Ok(v) => {
                    any_ok = true;
                    all.extend(v);
                }
                Err(e) => {
                    log::warn!("research: {name} search failed: {e:#}");
                    errors.push(format!("{name}: {e}"));
                }
            }
        }
        if !any_ok {
            anyhow::bail!("all research sources failed ({})", errors.join("; "));
        }
        let mut papers = dedupe(all);
        // OpenAlex records already carry Unpaywall's OA data; look up the others.
        let mut need: Vec<Paper> = papers.iter().filter(|p| p.doi.is_some() && p.oa_url.is_none() && !p.found_in.iter().any(|s| s == "openalex")).cloned().collect();
        if !need.is_empty() {
            deadline("open-access enrichment", 15, self.enrich_open_access(&mut need)).await;
            for n in need {
                if let Some(p) = papers.iter_mut().find(|p| p.key == n.key) {
                    *p = n;
                }
            }
        }
        papers.retain(|p| year_ok(p, f.from_year, f.to_year));
        if oa_only {
            papers.retain(|p| p.kind == ContentKind::FreeFullText);
        }
        if !f.kinds.is_empty() {
            papers.retain(|p| f.kinds.contains(&p.kind));
        }
        match f.sort {
            SortBy::Relevance => {
                let docs: Vec<String> = papers.iter().map(paper_doc).collect();
                let refs: Vec<&str> = docs.iter().map(String::as_str).collect();
                let bm = bm25_scores(&text, &refs);
                let mut scored: Vec<(f32, Paper)> = papers
                    .into_iter()
                    .enumerate()
                    .map(|(i, p)| {
                        let s = bm[i] + 0.6 * (p.found_in.len() as f32 - 1.0) + 0.25 * (p.cited_by_count.unwrap_or(0) as f32 + 1.0).log10() + if p.abstract_text.is_some() { 0.5 } else { 0.0 };
                        (s, p)
                    })
                    .collect();
                scored.sort_by(|a, b| b.0.total_cmp(&a.0));
                papers = scored.into_iter().map(|x| x.1).collect();
            }
            SortBy::Newest => papers.sort_by(|a, b| b.date.or(b.year.and_then(|y| NaiveDate::from_ymd_opt(y, 1, 1))).cmp(&a.date.or(a.year.and_then(|y| NaiveDate::from_ymd_opt(y, 1, 1))))),
            SortBy::MostCited => papers.sort_by(|a, b| b.cited_by_count.unwrap_or(0).cmp(&a.cited_by_count.unwrap_or(0))),
        }
        Ok(papers)
    }

    async fn search_news(&self, q: &SearchQuery) -> Result<Vec<Paper>> {
        let per_page = if q.per_page == 0 { 20 } else { q.per_page.min(100) } as usize;
        let text = q.text.trim();
        if text.is_empty() {
            return Ok(vec![]);
        }
        let since = q.filters.from_year.and_then(|y| NaiveDate::from_ymd_opt(y, 1, 1));
        let gq = gdelt_query(text, 5);
        let (g, rss) = tokio::join!(deadline_r("GDELT search", 60, self.gdelt_search(&gq, since, 150)), deadline_r("RSS feeds", 30, async { Ok(self.rss_items().await) }));
        let g_ok = g.is_ok();
        let mut items = or_warn("GDELT search", g);
        let qset = token_set(text);
        let need = if qset.len() >= 3 { 2 } else { 1 };
        items.extend(or_warn::<Vec<Paper>>("RSS", rss).into_iter().filter(|p| {
            let t = token_set(&format!("{} {}", p.title, p.abstract_text.as_deref().unwrap_or("")));
            t.intersection(&qset).count() >= need
        }));
        if items.is_empty() && !g_ok {
            anyhow::bail!("news search failed (GDELT unavailable and no RSS matches)");
        }
        let mut items = dedupe_news(items);
        if let Some(to) = q.filters.to_year {
            items.retain(|p| p.year.is_none_or(|y| y <= to));
        }
        match q.filters.sort {
            SortBy::Newest => items.sort_by(|a, b| b.date.cmp(&a.date)),
            _ => {
                let docs: Vec<String> = items.iter().map(paper_doc).collect();
                let refs: Vec<&str> = docs.iter().map(String::as_str).collect();
                let bm = bm25_scores(text, &refs);
                let mut idx: Vec<(f32, Paper)> = bm.into_iter().zip(items).collect();
                idx.sort_by(|a, b| b.0.total_cmp(&a.0).then(b.1.date.cmp(&a.1.date)));
                items = idx.into_iter().map(|x| x.1).collect();
            }
        }
        Ok(items.into_iter().skip(q.page as usize * per_page).take(per_page).collect())
    }

    /// Recent papers for a Discover topic tab (newest first, with abstracts, deduped).
    pub async fn recent_papers(&self, topic: &str, since: NaiveDate, limit: usize) -> Result<Vec<Paper>> {
        let limit = limit.max(1);
        let queries = topic_queries(topic);
        let per_q = ((limit * 3 / 2).div_ceil(queries.len().max(1))).clamp(8, 50) as u32;
        let mut since = since.min(today());
        let mut out = Vec::new();
        for attempt in 0..2 {
            // One OpenAlex request for all of the tab's queries (OpenAlex bills per search).
            let combined = or_query(&queries);
            let oa_fut = async {
                vec![
                    self.openalex_search(&OaSearch {
                        query: &combined,
                        from: Some(since),
                        to: Some(today()),
                        need_abstract: true,
                        sort: SortBy::Relevance,
                        per_page: (limit as u32 * 2).clamp(10, 50),
                        extra_filter: Some("type:article|preprint|review".into()),
                        ..Default::default()
                    })
                    .await,
                ]
            };
            let ax_futs = queries.iter().take(2).map(|qq| async move { self.arxiv_search(qq, 0, per_q.min(25), true).await });
            let (oa_res, ax_res) = tokio::join!(
                deadline("recent OpenAlex", 40, oa_fut),
                deadline("recent arXiv", 30, futures::future::join_all(ax_futs))
            );
            let ok = oa_res.iter().any(Result::is_ok) || ax_res.iter().any(Result::is_ok);
            let mut all: Vec<Paper> = Vec::new();
            // Interleave the queries so a broad tab mixes subjects.
            let lists: Vec<Vec<Paper>> = oa_res.into_iter().chain(ax_res).map(|r| or_warn("recent papers", r)).collect();
            let longest = lists.iter().map(Vec::len).max().unwrap_or(0);
            for i in 0..longest {
                for l in &lists {
                    if let Some(p) = l.get(i) {
                        all.push(p.clone());
                    }
                }
            }
            if !ok && attempt == 0 {
                anyhow::bail!("recent papers: all sources failed");
            }
            let mut papers = dedupe(all);
            // On-topic only: every word of at least one of the tab's queries must appear in the
            // title or abstract ("space exploration" must not match a "world-space" vision paper).
            let query_sets: Vec<HashSet<String>> = queries.iter().map(|q| token_set(q)).filter(|t| !t.is_empty()).collect();
            if !query_sets.is_empty() {
                papers.retain(|p| {
                    let words = token_set(&format!("{} {}", p.title, p.abstract_text.as_deref().unwrap_or("")));
                    query_sets.iter().any(|q| q.is_subset(&words))
                });
            }
            papers.retain(|p| {
                p.abstract_text.as_deref().is_some_and(|a| a.split_whitespace().count() >= 30) && p.date.is_none_or(|d| d >= since && d <= today())
            });
            papers.sort_by(|a, b| b.date.cmp(&a.date));
            out = papers;
            if out.len() >= limit / 3 || attempt == 1 {
                break;
            }
            since -= chrono::Duration::days(30);
        }
        out.truncate(limit);
        Ok(out)
    }

    /// Recent news on a topic grouped into events (each cluster: several outlets on one story).
    pub async fn news_clusters(&self, topic: &str, since: NaiveDate, max_clusters: usize) -> Result<Vec<NewsCluster>> {
        let gq = news_topic_query(topic);
        let (g, rss) = tokio::join!(deadline_r("GDELT", 70, self.gdelt_search(&gq, Some(since), 200)), deadline_r("RSS", 30, async { Ok(self.rss_items().await) }));
        let g_failed = g.is_err();
        let mut items = or_warn("GDELT", g);
        let terms = rss_topic_terms(topic);
        items.extend(or_warn::<Vec<Paper>>("RSS", rss).into_iter().filter(|p| p.date.is_none_or(|d| d >= since)).filter(|p| terms.as_ref().is_none_or(|t| rss_matches(p, t))));
        if items.is_empty() && g_failed {
            anyhow::bail!("news unavailable: GDELT failed and no RSS items matched");
        }
        let items = dedupe_news(items);
        let clusters = cluster_titles(items);
        let outlets = |c: &Vec<Paper>| c.iter().filter_map(|p| p.venue.clone()).map(|v| v.to_lowercase()).collect::<HashSet<_>>().len();
        let mut multi: Vec<Vec<Paper>> = clusters.iter().filter(|c| outlets(c) >= 2).cloned().collect();
        let singles: Vec<Vec<Paper>> = clusters.into_iter().filter(|c| outlets(c) < 2).collect();
        // Prefer singles that have some summary (RSS) to make a usable card.
        let mut singles = singles;
        singles.sort_by_key(|c| std::cmp::Reverse(c.iter().filter(|p| p.abstract_text.is_some()).count()));
        for s in singles {
            if multi.len() >= max_clusters {
                break;
            }
            multi.push(s);
        }
        multi.truncate(max_clusters);
        let out: Vec<NewsCluster> = futures::stream::iter(multi)
            .map(|c| async move {
                let articles = diversify(c, 2).into_iter().take(15).collect::<Vec<_>>();
                let title = articles.first().map(|p| clean_news_title(&p.title)).unwrap_or_default();
                let link_only = articles.iter().find_map(|p| p.image.clone());
                let open = match self.commons_search(&keywords(&title, 3).join(" "), 6).await {
                    Ok(imgs) => {
                        let wanted = token_set(&title);
                        imgs.into_iter().find(|i| i.embeddable && token_set(&i.title).intersection(&wanted).next().is_some())
                    }
                    Err(e) => {
                        log::warn!("Commons search failed: {e:#}");
                        None
                    }
                };
                NewsCluster { title, articles, image: open.or(link_only) }
            })
            .buffered(4)
            .collect()
            .await;
        Ok(out)
    }

    /// Fill `oa_url`, `oa_status`, `oa_version`, `oa_licence` and `kind` from Unpaywall /
    /// arXiv / Europe PMC for papers that lack them.
    pub async fn enrich_open_access(&self, papers: &mut [Paper]) {
        // arXiv and PubMed Central copies are free without a lookup.
        for p in papers.iter_mut() {
            if p.oa_url.is_some() || p.kind == ContentKind::News {
                continue;
            }
            if let Some(a) = &p.arxiv_id {
                p.oa_url = Some(format!("https://arxiv.org/abs/{a}"));
                p.oa_status.get_or_insert_with(|| "green".into());
                p.oa_version.get_or_insert_with(|| "submittedVersion".into());
            } else if let Some(pmc) = &p.pmcid {
                p.oa_url = Some(format!("https://europepmc.org/article/PMC/{}", pmc.trim_start_matches("PMC")));
                p.oa_status.get_or_insert_with(|| "green".into());
                p.oa_version.get_or_insert_with(|| "publishedVersion".into());
            }
            if p.oa_url.is_some() {
                p.kind = ContentKind::FreeFullText;
            }
        }
        let todo: Vec<(usize, String)> = papers.iter().enumerate().filter(|(_, p)| p.oa_url.is_none() && p.kind != ContentKind::News).filter_map(|(i, p)| p.doi.clone().map(|d| (i, d))).collect();
        let results: Vec<(usize, Option<crossref::OaLocation>)> = futures::stream::iter(todo)
            .map(|(i, doi)| async move {
                let r = self.unpaywall(&doi).await;
                (i, or_warn("Unpaywall", r.map(|x| x)))
            })
            .buffer_unordered(6)
            .collect()
            .await;
        for (i, loc) in results {
            let Some(loc) = loc else { continue };
            let Some(p) = papers.get_mut(i) else { continue };
            p.oa_status = loc.status.clone().or(p.oa_status.take());
            if loc.is_oa {
                if let Some(u) = loc.pdf_url.clone().or(loc.landing_url.clone()) {
                    p.oa_url = Some(u);
                    p.oa_version = loc.version.clone();
                    p.oa_licence = loc.licence.clone();
                    p.kind = ContentKind::FreeFullText;
                }
            } else if p.kind != ContentKind::FreeFullText {
                p.kind = ContentKind::AbstractOnly;
            }
        }
    }

    /// Search several queries across OpenAlex, Semantic Scholar and arXiv (for related work).
    async fn related_search(&self, queries: &[String], per_query: u32) -> Vec<Paper> {
        // At most two OpenAlex searches: the main query, then the rest OR-ed together.
        let mut oa_queries: Vec<String> = queries.iter().take(1).cloned().collect();
        if queries.len() > 1 {
            oa_queries.push(or_query(&queries[1..]));
        }
        let oa = futures::future::join_all(oa_queries.iter().map(|q| async move {
            or_warn("OpenAlex related", self.openalex_search(&OaSearch { query: q, need_abstract: true, per_page: per_query, ..Default::default() }).await)
        }));
        let s2 = futures::future::join_all(queries.iter().take(2).map(|q| async move { or_warn("Semantic Scholar related", self.s2_search(q, 0, per_query, None, false).await) }));
        let ax = async {
            match queries.first() {
                Some(q) => or_warn("arXiv related", self.arxiv_search(q, 0, per_query.min(8), false).await),
                None => vec![],
            }
        };
        let (oa, s2, ax) = tokio::join!(deadline("related OpenAlex", 30, oa), deadline("related S2", 30, s2), deadline("related arXiv", 30, ax));
        let mut all: Vec<Paper> = oa.into_iter().flatten().collect();
        all.extend(s2.into_iter().flatten());
        all.extend(ax);
        dedupe(all)
    }

    /// Gather sources for writing about one paper (interpretation or full-text article):
    /// the target itself (abstract as source 1), its legal full text if any (into
    /// `SourcePack::full_text`, split into original paragraphs), citing papers' sentences that
    /// describe it, related papers' abstracts, ranked chunks, and images.
    pub async fn gather_for_paper(&self, paper: &Paper, extra_queries: &[String], progress: UnboundedSender<Progress>) -> Result<SourcePack> {
        report(&progress, "Finding sources…", 0.03);
        let original_key = if paper.key.is_empty() { paper.make_key() } else { paper.key.clone() };

        // 1. Complete the target's identifiers and abstract.
        let mut target = paper.clone();
        let s2_lookup = async {
            let by_id = match Self::s2_ref(paper) {
                Some(id) => or_warn("S2 paper lookup", self.s2_paper(&id).await),
                None => None,
            };
            match by_id {
                Some(p) => Some(p),
                // S2 does not know every DOI; fall back to matching the title.
                None if !paper.title.is_empty() => or_warn("S2 title match", self.s2_match_title(&paper.title).await),
                None => None,
            }
        };
        let oa_lookup = async {
            match (&paper.doi, &paper.openalex_id) {
                (Some(d), _) if paper.openalex_id.is_none() || paper.abstract_text.is_none() => or_warn("OpenAlex lookup", self.openalex_work(d).await),
                _ => None,
            }
        };
        let ax_lookup = async {
            match &paper.arxiv_id {
                Some(a) if paper.abstract_text.is_none() => or_warn("arXiv lookup", self.arxiv_by_id(a).await),
                _ => None,
            }
        };
        let (s2p, oap, axp) = tokio::join!(deadline("S2 lookup", 20, s2_lookup), deadline("OpenAlex lookup", 20, oa_lookup), deadline("arXiv lookup", 20, ax_lookup));
        for found in [oap, s2p, axp].into_iter().flatten() {
            // Only merge a title match when it really is the same paper.
            if normalize_title(&found.title) == normalize_title(&target.title) || found.doi.is_some() && found.doi == target.doi || found.arxiv_id.is_some() && found.arxiv_id == target.arxiv_id {
                merge_into(&mut target, found);
            }
        }
        target.key = original_key.clone();
        report(&progress, "Searching OpenAlex, Semantic Scholar and arXiv…", 0.12);

        let query = format!("{} {} {}", target.title, target.abstract_text.as_deref().unwrap_or(""), extra_queries.join(" "));
        let mut queries = expand_queries(&target.title, extra_queries, 4);
        // The abstract's key terms find related work the title's words miss.
        if let Some(a) = target.abstract_text.as_deref() {
            let kw = keywords(a, 5).join(" ");
            if !kw.is_empty() && !queries.contains(&kw) {
                queries.insert(queries.len().min(1), kw);
                queries.truncate(4.max(extra_queries.len() + 1));
            }
        }
        let done = AtomicU32::new(0);
        let step = |text: String| {
            let n = done.fetch_add(1, Ordering::SeqCst) + 1;
            report(&progress, text, 0.12 + 0.68 * n as f32 / 4.0);
        };

        // 2. In parallel: full text, citing papers, related work, images.
        let ft_fut = async {
            let r = deadline("full text", 75, self.find_full_text(&target)).await;
            step(match &r {
                Some(f) => format!("Reading the free full text ({} sections)…", f.text.sections.len()),
                None => "No legal free full text found; using the abstract…".into(),
            });
            r
        };
        let cite_fut = async {
            let r = match Self::s2_ref(&target) {
                Some(id) => deadline("citations", 40, async { or_warn("S2 citations", self.s2_citations(&id, 500).await) }).await,
                None => vec![],
            };
            let mut with_ctx: Vec<_> = r.into_iter().filter(|c| !c.contexts.is_empty() && !c.paper.title.is_empty()).collect();
            with_ctx.sort_by_key(|c| std::cmp::Reverse((c.contexts.len().min(3), c.paper.cited_by_count.unwrap_or(0))));
            with_ctx.truncate(12);
            step(format!("Checking {} citing papers…", with_ctx.len()));
            with_ctx
        };
        let rel_fut = async {
            let mut rel = self.related_search(&queries, 10).await;
            let tnorm = normalize_title(&target.title);
            rel.retain(|p| p.key != target.key && normalize_title(&p.title) != tnorm && p.doi.as_ref().is_none_or(|d| Some(d) != target.doi.as_ref()) && p.abstract_text.is_some());
            let docs: Vec<String> = rel.iter().map(paper_doc).collect();
            let refs: Vec<&str> = docs.iter().map(String::as_str).collect();
            let scores = bm25_scores(&query, &refs);
            let mut scored: Vec<(f32, Paper)> = scores.into_iter().zip(rel).filter(|(s, _)| *s > 0.0).collect();
            scored.sort_by(|a, b| b.0.total_cmp(&a.0));
            let rel: Vec<Paper> = scored.into_iter().take(8).map(|x| x.1).collect();
            step(format!("Reading {} related papers…", rel.len()));
            rel
        };
        let img_fut = async {
            let r = deadline("images", 30, async { self.find_images(&target.title, Some(&target), 2, false).await }).await;
            step("Finding images…".into());
            r
        };
        let (full, citing, related, mut images) = tokio::join!(ft_fut, cite_fut, rel_fut, img_fut);

        report(&progress, "Ranking passages…", 0.85);
        let mut entries = Vec::new();
        let mut next_id = 1u32;
        let mut tsrc = Source::from_paper(next_id, SourceKind::Target, &target);
        tsrc.paper_key = Some(original_key.clone());
        let mut tchunks: Vec<(RawChunk, bool)> = abstract_chunks(&target).into_iter().map(|c| (c, true)).collect();
        if let Some(f) = &full {
            tchunks.extend(chunk_sections(&f.text.sections).into_iter().map(|c| (c, false)));
            for fig in &f.figures {
                if fig.embeddable && images.len() < 3 && !images.iter().any(|i| i.url == fig.url) {
                    images.insert(0, fig.clone());
                }
            }
        }
        entries.push(Entry { source: tsrc, chunks: tchunks, cap: 16 });
        next_id += 1;
        for c in citing {
            let text = c.contexts.iter().map(|s| text::strip_bracket_citations(s).trim().to_string()).collect::<Vec<_>>().join(" … ");
            let chunks = chunk_text(Some("How this citing paper describes the target"), &text).into_iter().map(|c| (c, false)).collect();
            entries.push(Entry { source: Source::from_paper(next_id, SourceKind::Citing, &c.paper), chunks, cap: 2 });
            next_id += 1;
        }
        for p in &related {
            entries.push(Entry { source: Source::from_paper(next_id, SourceKind::Paper, p), chunks: abstract_chunks(p).into_iter().map(|c| (c, false)).collect(), cap: 2 });
            next_id += 1;
        }
        let (sources, chunks) = assemble(&query, entries, MAX_CHUNKS);
        images.truncate(3);
        report(&progress, format!("Collected {} sources and {} passages", sources.len(), chunks.len()), 1.0);
        Ok(SourcePack { sources, chunks, full_text: full.map(|f| f.text), images })
    }

    /// Gather sources for an AI-only article on a topic, starting from `basis` papers.
    pub async fn gather_for_topic(&self, topic: &str, basis: &[Paper], extra_queries: &[String], progress: UnboundedSender<Progress>) -> Result<SourcePack> {
        report(&progress, "Finding sources…", 0.03);
        let query = format!("{topic} {} {}", basis.iter().map(|p| p.title.as_str()).collect::<Vec<_>>().join(" "), extra_queries.join(" "));
        let queries = expand_queries(topic, extra_queries, 4);
        let basis = dedupe(basis.to_vec());
        let done = AtomicU32::new(0);
        let step = |text: String| {
            let n = done.fetch_add(1, Ordering::SeqCst) + 1;
            report(&progress, text, 0.1 + 0.7 * n as f32 / 3.0);
        };
        let rel_fut = async {
            let rel = self.related_search(&queries, 10).await;
            step(format!("Found {} candidate papers…", rel.len()));
            rel
        };
        let ft_fut = async {
            let cands: Vec<&Paper> = basis.iter().filter(|p| p.arxiv_id.is_some() || p.pmcid.is_some() || p.oa_url.is_some()).take(3).collect();
            let texts = futures::future::join_all(cands.iter().map(|p| async move { (p.key.clone(), deadline("full text", 60, self.find_full_text(p)).await) })).await;
            step(format!("Reading {} free full texts…", texts.iter().filter(|t| t.1.is_some()).count()));
            texts
        };
        let img_fut = async {
            let r = deadline("images", 30, self.find_images(topic, basis.first(), 2, false)).await;
            step("Finding images…".into());
            r
        };
        let (related, texts, images) = tokio::join!(rel_fut, ft_fut, img_fut);

        report(&progress, "Ranking passages…", 0.85);
        let basis_keys: HashSet<String> = basis.iter().map(|p| p.key.clone()).collect();
        let basis_titles: HashSet<String> = basis.iter().map(|p| normalize_title(&p.title)).collect();
        let mut rel: Vec<Paper> = related.into_iter().filter(|p| !basis_keys.contains(&p.key) && !basis_titles.contains(&normalize_title(&p.title)) && p.abstract_text.is_some()).collect();
        let docs: Vec<String> = rel.iter().map(paper_doc).collect();
        let refs: Vec<&str> = docs.iter().map(String::as_str).collect();
        let scores = bm25_scores(&query, &refs);
        let mut scored: Vec<(f32, Paper)> = scores.into_iter().zip(rel.drain(..)).filter(|x| x.0 > 0.0).collect();
        scored.sort_by(|a, b| b.0.total_cmp(&a.0));
        let room = 14usize.saturating_sub(basis.len()).max(4);
        let related: Vec<Paper> = scored.into_iter().take(room).map(|x| x.1).collect();

        let mut entries = Vec::new();
        let mut id = 1u32;
        for p in &basis {
            let mut chunks: Vec<(RawChunk, bool)> = abstract_chunks(p).into_iter().map(|c| (c, false)).collect();
            if let Some((_, Some(f))) = texts.iter().find(|t| t.0 == p.key) {
                chunks.extend(chunk_sections(&f.text.sections).into_iter().map(|c| (c, false)));
            }
            entries.push(Entry { source: Source::from_paper(id, SourceKind::Paper, p), chunks, cap: 6 });
            id += 1;
        }
        for p in &related {
            entries.push(Entry { source: Source::from_paper(id, SourceKind::Paper, p), chunks: abstract_chunks(p).into_iter().map(|c| (c, false)).collect(), cap: 2 });
            id += 1;
        }
        let (sources, chunks) = assemble(&query, entries, MAX_CHUNKS);
        report(&progress, format!("Collected {} sources and {} passages", sources.len(), chunks.len()), 1.0);
        Ok(SourcePack { sources, chunks, full_text: None, images })
    }

    /// Gather 10–20 news sources across outlets about one event (starting from `basis`),
    /// fetching each article's readable text where the outlet serves it openly.
    pub async fn gather_news(&self, headline: &str, basis: &[Paper], progress: UnboundedSender<Progress>) -> Result<SourcePack> {
        report(&progress, "Finding coverage across outlets…", 0.03);
        let since = today() - chrono::Duration::days(10);
        let basis_text = basis.iter().map(|p| p.title.as_str()).collect::<Vec<_>>().join(" ");
        let query = format!("{headline} {basis_text}");
        let head_set = token_set(&query);

        let gdelt = async {
            let mut r = or_warn("GDELT", self.gdelt_search(&gdelt_query(headline, 5), Some(since), 75).await);
            if r.len() < 8 {
                // Broaden: fewer terms, from the outlets' own wording when we have it.
                let alt = basis.first().map(|b| gdelt_query(&b.title, 3)).unwrap_or_else(|| gdelt_query(headline, 3));
                r.extend(or_warn("GDELT (broader)", self.gdelt_search(&alt, Some(since), 75).await));
            }
            r
        };
        let (g, rss) = tokio::join!(deadline("GDELT", 90, gdelt), deadline("RSS", 30, self.rss_items()));
        report(&progress, format!("Found {} candidate articles…", g.len() + basis.len()), 0.25);
        let basis_sets: Vec<HashSet<String>> = basis.iter().map(|p| token_set(&p.title)).chain(std::iter::once(token_set(headline))).collect();
        let rss: Vec<Paper> = rss.into_iter().filter(|p| p.date.is_none_or(|d| d >= since)).filter(|p| basis_sets.iter().any(|b| same_event(b, &token_set(&p.title)))).collect();

        let mut cands: Vec<Paper> = basis.to_vec();
        let mut rest: Vec<Paper> = g.into_iter().chain(rss).collect();
        // Relevance to the event: shared tokens with the headline + basis titles.
        rest.sort_by_key(|p| std::cmp::Reverse(token_set(&p.title).intersection(&head_set).count()));
        rest.retain(|p| token_set(&p.title).intersection(&head_set).count() >= 2);
        cands.extend(rest);
        let cands = diversify(dedupe_news(cands), 2);
        let total = cands.len().min(32);

        // Fetch pages; keep up to 20 readable ones.
        let read = AtomicU32::new(0);
        let fetched: Vec<(Paper, Vec<String>, Option<String>, Option<String>)> = futures::stream::iter(cands.into_iter().take(32))
            .map(|p| {
                let read = &read;
                let progress = &progress;
                async move {
                    let url = p.url.clone().unwrap_or_default();
                    let (paras, site, og) = match self.get_page(&url).await {
                        Ok(bytes) if !bytes.starts_with(b"%PDF") => {
                            let html = String::from_utf8_lossy(&bytes).into_owned();
                            (html_paragraphs(&html), text::og_site_name(&html), text::og_image(&html))
                        }
                        Ok(_) => (vec![], None, None),
                        Err(e) => {
                            log::debug!("news page {url} skipped: {e:#}");
                            (vec![], None, None)
                        }
                    };
                    let n = read.fetch_add(1, Ordering::SeqCst) + 1;
                    report(progress, format!("Reading {n} of {total} articles…"), 0.25 + 0.55 * n as f32 / total.max(1) as f32);
                    (p, paras, site, og)
                }
            })
            .buffered(6)
            .collect()
            .await;

        report(&progress, "Ranking passages…", 0.85);
        let mut entries = Vec::new();
        let mut og_images = Vec::new();
        let mut id = 1u32;
        for (mut p, paras, site, og) in fetched {
            if entries.len() >= 20 {
                break;
            }
            let words: usize = paras.iter().map(|x| x.split_whitespace().count()).sum();
            let chunks: Vec<RawChunk> = if words >= 80 {
                chunk_sections(&[FullTextSection { heading: String::new(), paragraphs: paras }])
            } else if let Some(a) = p.abstract_text.as_deref().filter(|a| a.split_whitespace().count() >= 25) {
                chunk_text(None, a)
            } else {
                continue; // paywalled / blocked / not an article
            };
            if let Some(s) = site.filter(|s| s.len() < 60) {
                p.venue = Some(s);
            }
            if let (Some(img), Some(url)) = (og.or(p.image.as_ref().map(|i| i.url.clone())), p.url.clone()) {
                if og_images.len() < 2 {
                    og_images.push(link_only_image(&img, &url, p.venue.as_deref().unwrap_or(""), &p.title));
                }
            }
            let mut src = Source::from_paper(id, SourceKind::News, &p);
            src.date = p.date;
            entries.push(Entry { source: src, chunks: chunks.into_iter().map(|c| (c, false)).collect(), cap: 3 });
            id += 1;
        }
        if entries.is_empty() {
            anyhow::bail!("no readable news coverage found for \"{headline}\"");
        }
        let (sources, chunks) = assemble(&query, entries, MAX_CHUNKS);
        let mut images = deadline("images", 25, self.find_images(headline, None, 1, false)).await;
        images.extend(og_images);
        images.truncate(3);
        report(&progress, format!("Collected {} articles from {} outlets", sources.len(), sources.iter().filter_map(|s| s.venue.clone()).collect::<HashSet<_>>().len()), 1.0);
        Ok(SourcePack { sources, chunks, full_text: None, images })
    }

    /// Gather sources to answer an Ask-panel question (search + rank), merged after the
    /// article's existing sources (ids continue after `existing.len()`).
    ///
    /// Returns only the *new* sources (ids `existing.len()+1…`) and their ranked chunks.
    pub async fn gather_for_question(&self, question: &str, existing: &[Source]) -> Result<SourcePack> {
        let q = keywords(question, 6).join(" ");
        let q = if q.is_empty() { question.to_string() } else { q };
        let (oa, s2) = tokio::join!(
            deadline_r("OpenAlex", 25, async { self.openalex_search(&OaSearch { query: &q, need_abstract: true, per_page: 12, ..Default::default() }).await }),
            deadline_r("S2", 25, async { self.s2_search(&q, 0, 10, None, false).await })
        );
        if oa.is_err() && s2.is_err() {
            anyhow::bail!("search failed: {}", oa.err().map(|e| e.to_string()).unwrap_or_default());
        }
        let mut all = or_warn("OpenAlex", oa);
        all.extend(or_warn("S2", s2));
        let dois: HashSet<String> = existing.iter().filter_map(|s| s.doi.as_deref().map(normalize_doi)).collect();
        let urls: HashSet<String> = existing.iter().filter_map(|s| s.url.clone()).collect();
        let titles: HashSet<String> = existing.iter().map(|s| normalize_title(&s.title)).collect();
        let keys: HashSet<String> = existing.iter().filter_map(|s| s.paper_key.clone()).collect();
        let mut papers: Vec<Paper> = dedupe(all)
            .into_iter()
            .filter(|p| p.abstract_text.is_some())
            .filter(|p| {
                !keys.contains(&p.key)
                    && !p.doi.as_deref().is_some_and(|d| dois.contains(&normalize_doi(d)))
                    && !p.url.as_ref().is_some_and(|u| urls.contains(u))
                    && !titles.contains(&normalize_title(&p.title))
            })
            .collect();
        let docs: Vec<String> = papers.iter().map(paper_doc).collect();
        let refs: Vec<&str> = docs.iter().map(String::as_str).collect();
        let scores = bm25_scores(question, &refs);
        let mut scored: Vec<(f32, Paper)> = scores.into_iter().zip(papers.drain(..)).filter(|x| x.0 > 0.0).collect();
        scored.sort_by(|a, b| b.0.total_cmp(&a.0));
        let start = existing.iter().map(|s| s.id).max().unwrap_or(0).max(existing.len() as u32) + 1;
        let entries: Vec<Entry> = scored
            .into_iter()
            .take(6)
            .enumerate()
            .map(|(i, (_, p))| Entry { source: Source::from_paper(start + i as u32, SourceKind::Paper, &p), chunks: abstract_chunks(&p).into_iter().map(|c| (c, false)).collect(), cap: 2 })
            .collect();
        let (sources, chunks) = assemble(question, entries, 12);
        Ok(SourcePack { sources, chunks, full_text: None, images: vec![] })
    }

    /// The closest relevant openly licensed image (Wikimedia Commons; open-access figures when
    /// the paper has them). Copyrighted images are returned link-only (`embeddable = false`).
    pub async fn find_image(&self, query: &str, paper: Option<&Paper>) -> Result<Option<ImageRef>> {
        Ok(self.find_images(query, paper, 1, true).await.into_iter().next())
    }

    /// Like `find_image`, but always tries to return a picture: when nothing matches the item
    /// itself, the lead Wikipedia image for the broader topic (e.g. the Discover tab or the
    /// paper's field). Such images are marked `from_paper = false` ("may not be from the paper").
    pub async fn find_image_or_topic(&self, query: &str, paper: Option<&Paper>, topic: &str) -> Option<ImageRef> {
        if let Some(i) = self.find_image(query, paper).await.ok().flatten().filter(|i| i.embeddable) {
            return Some(i);
        }
        // The tab's topic photo (subject tags gave odd matches, e.g. "Cascade" → a train).
        let mut topics: Vec<String> = topic_photo_queries(topic);
        let mut seen = HashSet::new();
        topics.retain(|t| seen.insert(t.to_lowercase()));
        // Rotate the tab's photo topics per item so a feed isn't one picture repeated.
        let curated = topics.len().saturating_sub(2).max(1);
        let start = query.bytes().fold(0usize, |h, b| h.wrapping_mul(31).wrapping_add(b as usize)) % curated;
        topics[..curated].rotate_left(start);
        for t in topics.into_iter().take(5) {
            if let Ok(imgs) = self.wikipedia_images(&t, 1).await
                && let Some(i) = imgs.into_iter().next()
            {
                return Some(i);
            }
        }
        None
    }

    /// Up to `n` images: the paper's own open figures first (when `paper_figures`), then
    /// Wikimedia Commons.
    async fn find_images(&self, query: &str, paper: Option<&Paper>, n: usize, paper_figures: bool) -> Vec<ImageRef> {
        let mut out: Vec<ImageRef> = Vec::new();
        if let Some(p) = paper.filter(|_| paper_figures) {
            // Europe PMC figures (cheap: one cached XML request).
            let mut pmcid = p.pmcid.clone();
            if pmcid.is_none() && p.arxiv_id.is_none() {
                if let Some(d) = &p.doi {
                    pmcid = or_warn("Europe PMC lookup", self.europepmc_pmcid(d).await);
                }
            }
            if let Some(pmc) = pmcid {
                if let Some(ft) = or_warn("Europe PMC figures", self.europepmc_fulltext(&pmc).await) {
                    out.extend(self.resolve_pmc_figures(&pmc, ft.figures).await.into_iter().take(1));
                }
            }
            // arXiv figures when the preprint carries a free licence.
            if out.is_empty() && p.oa_licence.as_deref().is_some_and(is_free_licence) {
                if let Some(id) = &p.arxiv_id {
                    let url = format!("https://arxiv.org/html/{id}");
                    if let Ok(bytes) = self.get_page(&url).await {
                        let html = String::from_utf8_lossy(&bytes);
                        if let Some((img, cap)) = fulltext::arxiv_html_figures(&html, &format!("{url}/")).into_iter().next() {
                            out.push(ImageRef {
                                url: img,
                                thumb_url: None,
                                page_url: format!("https://arxiv.org/abs/{id}"),
                                title: cap.chars().take(200).collect(),
                                credit: format!("Figure from {} (arXiv:{id})", p.author_line(2)),
                                licence: wikimedia::licence_label(p.oa_licence.as_deref().unwrap_or("")),
                                licence_url: None,
                                from_paper: true,
                                embeddable: true,
                            });
                        }
                    }
                }
            }
        }
        let kw4 = keywords(query, 4);
        let kw2 = keywords(query, 2);
        // Wikipedia: the lead image of the best-matching article, usually the clearest picture
        // of the topic (e.g. "Axion" for an axion dark-matter paper).
        if out.len() < n {
            let mut wtries: Vec<String> = vec![];
            if !kw4.is_empty() {
                wtries.push(kw4.join(" "));
            }
            if kw2.len() == 2 {
                wtries.push(kw2.join(" "));
            }
            let mut seen_w = HashSet::new();
            wtries.retain(|t| seen_w.insert(t.to_lowercase()));
            for t in wtries {
                if out.len() >= n {
                    break;
                }
                match self.wikipedia_images(&t, 3).await {
                    Ok(imgs) => {
                        // The Wikipedia article must be about something we searched for
                        // (search sometimes returns loosely related pages).
                        let searched = token_set(&t);
                        for i in imgs {
                            let article = token_set(i.title.trim_end_matches(" (Wikipedia)"));
                            // Shared words must be meaningful and make up at least half of the
                            // article's name ("AI" alone must not pull in an unrelated article).
                            let shared: Vec<&String> = article.intersection(&searched).filter(|w| !is_generic_word(w)).collect();
                            if shared.is_empty() || shared.len() * 2 <= article.len() && article.len() > 1 {
                                continue;
                            }
                            if out.len() < n && !out.iter().any(|o| o.url == i.url) {
                                out.push(i);
                            }
                        }
                    }
                    Err(e) => log::warn!("Wikipedia image search failed: {e:#}"),
                }
            }
        }
        // Commons: from specific to broad.
        let mut tries: Vec<String> = Vec::new();
        if !kw4.is_empty() {
            tries.push(kw4.join(" "));
        }
        if let Some(p) = paper {
            if let Some(s) = p.subjects.iter().find(|s| !s.contains('.')) {
                tries.push(s.clone());
            }
        }
        if kw2.len() == 2 {
            tries.push(kw2.join(" "));
        }
        if let Some(one) = keywords(query, 1).first() {
            tries.push(one.clone());
        }
        let mut seen = HashSet::new();
        tries.retain(|t| seen.insert(t.to_lowercase()));
        let mut fallback: Option<ImageRef> = None;
        // A Commons hit must share a word with what we searched for (its title is all we know).
        // Significant words only; a single shared common word gave unrelated pictures
        // (a bird for a star paper), and a wrong picture is worse than none.
        let wanted: HashSet<String> = keywords(query, 8).iter().flat_map(|k| token_set(k)).collect();
        let needed = 2.min(wanted.len().max(1));
        for t in tries.into_iter().take(3) {
            if out.len() >= n {
                break;
            }
            match self.commons_search(&t, 8).await {
                Ok(imgs) => {
                    for i in imgs {
                        if out.len() >= n {
                            break;
                        }
                        // Enough shared words, and they must be a real share of the image's own
                        // title (long Commons titles otherwise match by chance).
                        let title_tokens = token_set(&i.title);
                        let shared = title_tokens.intersection(&wanted).filter(|w| !is_generic_word(w)).count();
                        let relevant = shared >= needed && shared as f32 / title_tokens.len().max(1) as f32 >= 0.34;
                        if !relevant {
                            continue;
                        }
                        if i.embeddable && !out.iter().any(|o| o.url == i.url) {
                            out.push(i);
                        } else if fallback.is_none() {
                            fallback = Some(i);
                        }
                    }
                }
                Err(e) => log::warn!("Commons search failed: {e:#}"),
            }
        }
        if out.is_empty() {
            out.extend(fallback);
        }
        out
    }

    /// Download (with disk cache under `db.image_cache_dir()`) image bytes for display.
    pub async fn image_bytes(&self, url: &str) -> Result<Vec<u8>> {
        use sha2::{Digest, Sha256};
        let dir = self.db.image_cache_dir();
        let name = hex::encode(Sha256::digest(url.as_bytes()));
        let path = dir.join(&name);
        if let Ok(b) = tokio::fs::read(&path).await {
            if !b.is_empty() {
                return Ok(b);
            }
        }
        let bytes = http::download_uncached(self, url, 8 * 1024 * 1024).await?;
        if !looks_like_image(&bytes) {
            anyhow::bail!("{url} did not return an image");
        }
        if let Err(e) = tokio::fs::create_dir_all(&dir).await {
            log::warn!("image cache dir: {e}");
        } else {
            let tmp = dir.join(format!("{name}.part"));
            if tokio::fs::write(&tmp, &bytes).await.is_ok() {
                let _ = tokio::fs::rename(&tmp, &path).await;
            }
        }
        Ok(bytes)
    }

    /// New papers for a followed topic or author since a date (for notifications).
    pub async fn new_for_follow(&self, follow: &Follow, since: NaiveDate) -> Result<Vec<Paper>> {
        let mut papers = match follow {
            Follow::Topic(t) => {
                self.openalex_search(&OaSearch { query: t, from: Some(since), to: Some(today()), sort: SortBy::Relevance, per_page: 25, ..Default::default() }).await?
            }
            Follow::Author(name) => {
                let Some(id) = self.openalex_author_id(name).await? else { return Ok(vec![]) };
                self.openalex_search(&OaSearch {
                    query: "",
                    from: Some(since),
                    to: Some(today()),
                    sort: SortBy::Newest,
                    per_page: 25,
                    extra_filter: Some(format!("author.id:{id}")),
                    ..Default::default()
                })
                .await?
            }
        };
        papers = dedupe(papers);
        papers.sort_by(|a, b| b.date.cmp(&a.date));
        Ok(papers)
    }
}

/// Drop trailing " - Outlet" / " | Outlet" suffixes from a headline.
fn clean_news_title(t: &str) -> String {
    let t = t.trim();
    for sep in [" | ", " - ", " — ", " – "] {
        if let Some((head, tail)) = t.rsplit_once(sep) {
            if tail.split_whitespace().count() <= 4 && head.split_whitespace().count() >= 4 {
                return head.trim().to_string();
            }
        }
    }
    t.to_string()
}

fn looks_like_image(b: &[u8]) -> bool {
    b.starts_with(&[0xFF, 0xD8, 0xFF])
        || b.starts_with(b"\x89PNG")
        || b.starts_with(b"GIF8")
        || (b.len() > 12 && &b[0..4] == b"RIFF" && &b[8..12] == b"WEBP")
        || {
            let head = String::from_utf8_lossy(&b[..b.len().min(256)]).to_lowercase();
            head.contains("<svg") || (head.starts_with("<?xml") && head.contains("svg"))
        }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn research_assemble_keeps_one_chunk_per_source() {
        let mk = |id: u32, texts: &[&str]| Entry {
            source: Source { id, title: format!("s{id}"), ..Default::default() },
            chunks: texts.iter().map(|t| (RawChunk { section: None, text: t.to_string() }, false)).collect(),
            cap: 10,
        };
        let entries = vec![
            mk(1, &["black hole black hole radiation", "black hole thermodynamics", "black hole spin", "black hole jets"]),
            mk(2, &["gardening tips"]),
            mk(3, &["black hole mergers"]),
        ];
        let (sources, chunks) = assemble("black hole", entries, 3);
        assert_eq!(sources.len(), 3);
        assert_eq!(chunks.len(), 3);
        let ids: HashSet<u32> = chunks.iter().map(|c| c.source_id).collect();
        assert_eq!(ids, [1, 2, 3].into_iter().collect());
        assert!(chunks[0].score >= chunks[1].score && chunks[1].score >= chunks[2].score);
    }

    #[test]
    fn research_news_helpers() {
        assert_eq!(clean_news_title("NASA delays Artemis II launch again - BBC News"), "NASA delays Artemis II launch again");
        let mk = |t: &str, u: &str| news::news_paper(t.into(), u.into(), domain_of(u), None, None, None, "t");
        let items = vec![
            mk("Comet lights up the night sky over Europe", "https://a.com/1"),
            mk("Comet lights up the night sky over Europe", "https://b.com/1"),
            mk("Other story entirely here", "https://www.a.com/1/"),
            mk("Other story about rockets", "https://a.com/2"),
            mk("Rocket story three", "https://a.com/3"),
            mk("Different outlet story", "https://c.com/1"),
        ];
        let d = dedupe_news(items);
        assert_eq!(d.len(), 4);
        let div = diversify(d, 2);
        let doms: Vec<String> = div.iter().map(|p| domain_of(p.url.as_deref().unwrap_or(""))).collect();
        assert_eq!(doms, vec!["a.com", "c.com", "a.com"]);
        assert!(looks_like_image(b"\x89PNG\r\n"));
        assert!(!looks_like_image(b"<html>"));
    }
}
