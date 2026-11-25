//! Background tasks. Each takes clones of what it needs from `App`, runs on the tokio runtime
//! and reports through the `Bus`. Nothing here touches UI state directly.

use std::collections::HashSet;
use std::sync::atomic::Ordering;

use chrono::{Days, Utc};
use tokio::sync::mpsc::unbounded_channel;

use crate::app::{App, AppEvent, ExportKind, ToastKind};
use crate::db::{AskEntry, QueueStatus};
use crate::model::*;
use crate::providers::{self, ProviderConfig, SharedProvider};
use crate::writing::{self, ArticleRequest, Length, WriteEvent};

pub fn detect_providers(app: &App) {
    let bus = app.bus.clone();
    app.rt.spawn(async move {
        let d = providers::detect::detect().await;
        bus.send(AppEvent::Detected(d));
    });
}

pub fn list_ollama(app: &App, base_url: String) {
    let bus = app.bus.clone();
    app.rt.spawn(async move {
        match providers::ollama::list_models(&base_url).await {
            Ok(models) => bus.send(AppEvent::OllamaModels { models }),
            Err(e) => bus.toast(format!("Ollama: {e}"), ToastKind::Error),
        }
    });
}

pub fn test_provider(app: &App, cfg: ProviderConfig) {
    let bus = app.bus.clone();
    app.rt.spawn(async move {
        let id = cfg.id.clone();
        let result = async {
            let p = providers::build(&cfg)?;
            let req = providers::CompletionRequest {
                system: "You are a terse assistant.".into(),
                prompt: "Reply with exactly: Gateless Scholar is connected.".into(),
                max_tokens: 30,
                temperature: 0.0,
                json: false,
                allow_tools: false,
            };
            p.complete(&req, None).await
        }
        .await
        .map(|s| s.trim().chars().take(120).collect())
        .map_err(|e| e.to_string());
        bus.send(AppEvent::ProviderTest { id, result });
    });
}

pub fn load_image(app: &App, url: String) {
    let bus = app.bus.clone();
    let research = app.research.clone();
    app.rt.spawn(async move {
        let result = async {
            let bytes = research.image_bytes(&url).await?;
            let img = tokio::task::spawn_blocking(move || -> anyhow::Result<egui::ColorImage> {
                let img = image::load_from_memory(&bytes)?;
                // Keep textures modest: cards and the viewer never need more than ~1600 px.
                let img = if img.width() > 1600 || img.height() > 1600 { img.thumbnail(1600, 1600) } else { img };
                let rgba = img.to_rgba8();
                Ok(egui::ColorImage::from_rgba_unmultiplied([rgba.width() as usize, rgba.height() as usize], rgba.as_raw()))
            })
            .await??;
            anyhow::Ok(img)
        }
        .await
        .map_err(|e| e.to_string());
        bus.send(AppEvent::Image { url, result });
    });
}

/// Research → write → save one document's article, streaming progress to the reader.
pub fn generate(app: &App, doc: Doc, provider: SharedProvider) {
    let bus = app.bus.clone();
    let research = app.research.clone();
    let db = app.db.clone();
    let allow_tools = app.settings.cli_extra_research;
    app.rt.spawn(async move {
        let key = doc.key();
        let result = async {
            // Research phase: 0–30 %.
            let (ptx, mut prx) = unbounded_channel::<crate::research::Progress>();
            let fwd_bus = bus.clone();
            let fwd_key = key.clone();
            let fwd = tokio::spawn(async move {
                while let Some(p) = prx.recv().await {
                    fwd_bus.send(AppEvent::GenStage { key: fwd_key.clone(), text: p.text, fraction: 0.02 + p.fraction.clamp(0.0, 1.0) * 0.28 });
                }
            });
            let extra = writing::expand_query(provider.clone(), doc.title()).await;
            let (pack, req) = match &doc {
                Doc::Feed(f) if f.kind == ContentKind::News => {
                    let pack = research.gather_news(&f.headline, &f.basis, ptx).await?;
                    (pack, ArticleRequest { kind: ArticleKind::News, target: None, topic: f.headline.clone(), brief: Some(f.summary.clone()), length: Length::OnePage, allow_tools })
                }
                Doc::Feed(f) if f.kind == ContentKind::AiArticle || f.paper.is_none() => {
                    let pack = research.gather_for_topic(&f.headline, &f.basis, &extra, ptx).await?;
                    (pack, ArticleRequest { kind: ArticleKind::AiOnly, target: None, topic: f.headline.clone(), brief: Some(f.summary.clone()), length: Length::Journal, allow_tools })
                }
                _ => {
                    let paper = doc.paper().cloned().unwrap_or_default();
                    let pack = research.gather_for_paper(&paper, &extra, ptx).await?;
                    let kind = if pack.full_text.as_ref().is_some_and(|f| !f.sections.is_empty()) { ArticleKind::FullText } else { ArticleKind::Interpretation };
                    (pack, ArticleRequest { kind, target: Some(paper.clone()), topic: paper.title.clone(), brief: None, length: Length::Journal, allow_tools })
                }
            };
            let _ = fwd.await;
            if pack.sources.is_empty() {
                anyhow::bail!("no sources could be found for this item, so nothing can be written about it honestly");
            }
            bus.send(AppEvent::GenSources { key: key.clone(), sources: pack.sources.clone() });

            // Writing phase: 30–100 %.
            let (wtx, mut wrx) = unbounded_channel::<WriteEvent>();
            let wbus = bus.clone();
            let wkey = key.clone();
            let wfwd = tokio::spawn(async move {
                while let Some(ev) = wrx.recv().await {
                    match ev {
                        WriteEvent::Stage { text, fraction } => wbus.send(AppEvent::GenStage { key: wkey.clone(), text, fraction: 0.3 + fraction.clamp(0.0, 1.0) * 0.7 }),
                        WriteEvent::Delta(text) => wbus.send(AppEvent::GenDelta { key: wkey.clone(), text }),
                        WriteEvent::Section(section) => wbus.send(AppEvent::GenSection { key: wkey.clone(), section }),
                    }
                }
            });
            let article = writing::write_article(provider.clone(), req, pack, wtx).await?;
            let _ = wfwd.await;
            db.put_article(&key, &article)?;
            anyhow::Ok(article)
        }
        .await;
        match result {
            Ok(article) => {
                let _ = db.set_queue_status(&key, QueueStatus::Done, None);
                bus.send(AppEvent::GenDone { key, article: Box::new(article) });
            }
            Err(e) => {
                let msg = format!("{e:#}");
                let _ = db.set_queue_status(&key, QueueStatus::Failed, Some(&msg));
                bus.send(AppEvent::GenFailed { key, error: msg });
            }
        }
    });
}

/// Learn AI: plain-language explanation, streamed like an article and saved under `key`.
pub fn learn(app: &App, key: String, input: crate::writing::learn::LearnInput, provider: SharedProvider) {
    let bus = app.bus.clone();
    let db = app.db.clone();
    app.rt.spawn(async move {
        let (wtx, mut wrx) = unbounded_channel::<WriteEvent>();
        let wbus = bus.clone();
        let wkey = key.clone();
        let fwd = tokio::spawn(async move {
            while let Some(ev) = wrx.recv().await {
                match ev {
                    WriteEvent::Stage { text, fraction } => wbus.send(AppEvent::GenStage { key: wkey.clone(), text, fraction }),
                    WriteEvent::Delta(text) => wbus.send(AppEvent::GenDelta { key: wkey.clone(), text }),
                    WriteEvent::Section(section) => wbus.send(AppEvent::GenSection { key: wkey.clone(), section }),
                }
            }
        });
        let result = crate::writing::learn::explain(provider, input, wtx).await;
        let _ = fwd.await;
        match result {
            Ok(article) => {
                let _ = db.put_article(&key, &article);
                bus.send(AppEvent::GenDone { key, article: Box::new(article) });
            }
            Err(e) => bus.send(AppEvent::GenFailed { key, error: format!("{e:#}") }),
        }
    });
}

/// A photo for a document that has none: the closest openly licensed image (Wikipedia lead
/// images, then Commons, then the topic's own image). Cached in the database.
pub fn find_image(app: &App, doc: Doc, topic: String) {
    let bus = app.bus.clone();
    let research = app.research.clone();
    let db = app.db.clone();
    let sem = app.image_jobs.clone();
    app.rt.spawn(async move {
        let key = doc.key();
        let cache_key = format!("img:{key}");
        if let Some(cached) = db.kv_get(&cache_key) {
            let image: Option<ImageRef> = serde_json::from_str(&cached).ok().flatten();
            bus.send(AppEvent::FoundImage { key, image });
            return;
        }
        let Ok(_permit) = sem.acquire().await else { return };
        let topic = doc.paper().and_then(|p| p.subjects.first().cloned()).unwrap_or(topic);
        let image = research.find_image_or_topic(doc.title(), doc.paper(), &topic).await;
        let _ = db.kv_set(&cache_key, &serde_json::to_string(&image).unwrap_or_default());
        bus.send(AppEvent::FoundImage { key, image });
    });
}

pub fn ask(app: &App, key: String, article: Article, question: String, provider: SharedProvider) {
    let bus = app.bus.clone();
    let research = app.research.clone();
    let db = app.db.clone();
    app.rt.spawn(async move {
        let result = async {
            let pack = research.gather_for_question(&question, &article.sources).await.unwrap_or_default();
            let (dtx, mut drx) = unbounded_channel::<String>();
            let dbus = bus.clone();
            let dkey = key.clone();
            let fwd = tokio::spawn(async move {
                while let Some(t) = drx.recv().await {
                    dbus.send(AppEvent::AskDelta { key: dkey.clone(), text: t });
                }
            });
            let answer = writing::answer_question(provider, &article, &question, pack, Some(dtx)).await?;
            let _ = fwd.await;
            // Remember the papers the answer cites so "open in Gateless" works for them.
            db.add_ask(&key, &question, &answer)?;
            anyhow::Ok(answer)
        }
        .await;
        match result {
            Ok(answer) => bus.send(AppEvent::AskDone { key, entry: AskEntry { id: 0, question, answer, created: Utc::now() } }),
            Err(e) => bus.send(AppEvent::AskFailed { key, error: format!("{e:#}") }),
        }
    });
}

pub fn search(app: &App, id: u64, query: SearchQuery, append: bool) {
    let bus = app.bus.clone();
    let research = app.research.clone();
    app.rt.spawn(async move {
        let result = research.search(&query).await.map_err(|e| format!("{e:#}"));
        bus.send(AppEvent::Search { id, result, append });
    });
}

/// AI mode: the model plans queries and filters, the research layer searches, the model
/// curates the newest and most relevant to the top, and the normal list follows below.
pub fn ai_search(app: &App, id: u64, query: SearchQuery, provider: SharedProvider) {
    let bus = app.bus.clone();
    let research = app.research.clone();
    app.rt.spawn(async move {
        let plan = writing::plan_search(provider.clone(), &query.text).await.unwrap_or_else(|_| writing::SearchPlan {
            queries: vec![query.text.clone()],
            filters: query.filters.clone(),
            explanation: String::new(),
        });
        bus.send(AppEvent::AiPlan { id, plan: plan.clone() });
        let mut all: Vec<Paper> = vec![];
        let mut seen = HashSet::new();
        let mut queries = plan.queries.clone();
        if !queries.iter().any(|q| q.eq_ignore_ascii_case(&query.text)) {
            queries.insert(0, query.text.clone());
        }
        for q in queries.iter().take(4) {
            let sq = SearchQuery { text: q.clone(), scope: query.scope, filters: plan.filters.clone(), page: 0, per_page: 20 };
            match research.search(&sq).await {
                Ok(ps) => {
                    for p in ps {
                        if seen.insert(p.key.clone()) {
                            all.push(p);
                        }
                    }
                }
                Err(e) => log::warn!("ai search query {q}: {e}"),
            }
        }
        bus.send(AppEvent::Search { id, result: Ok(all.clone()), append: false });
        let curated = writing::curate_results(provider, &query.text, &all).await.map_err(|e| format!("{e:#}"));
        bus.send(AppEvent::AiCurated { id, result: curated });
    });
}

/// Older items for a Discover tab once stored days run out (infinite scroll).
pub fn feed_more(app: &App, feed: FeedKind, tab: String, page: u32) {
    let bus = app.bus.clone();
    let research = app.research.clone();
    app.rt.spawn(async move {
        let topic = topic_query(&tab);
        let query = SearchQuery {
            text: topic,
            scope: if feed == FeedKind::News { SearchScope::News } else { SearchScope::Journals },
            filters: SearchFilters { sort: SortBy::Newest, from_year: Some(Utc::now().year_ce().1 as i32 - 3), ..Default::default() },
            page,
            per_page: 20,
        };
        let papers = research.search(&query).await.unwrap_or_default();
        let exhausted = papers.is_empty();
        let items = papers
            .into_iter()
            .filter(|p| feed == FeedKind::News || p.abstract_text.as_ref().is_some_and(|a| !a.is_empty()))
            .map(|p| {
                // Metadata dates can be wrong (even in the future): never later than today.
                let today = Utc::now().date_naive();
                paper_card(&p, feed, &tab, p.date.unwrap_or(today).min(today))
            })
            .collect();
        bus.send(AppEvent::FeedMore { feed, tab, items, exhausted });
    });
}

use chrono::Datelike;

/// The search query used for a tab's topic ("For You" means a broad science mix).
pub fn topic_query(tab: &str) -> String {
    if tab.eq_ignore_ascii_case("for you") { "science breakthrough".to_string() } else { tab.to_string() }
}

fn paper_card(p: &Paper, feed: FeedKind, tab: &str, day: chrono::NaiveDate) -> FeedItem {
    FeedItem {
        id: short_hash(&format!("{}|{}|{}", feed.as_str(), tab, p.key)),
        day,
        feed,
        tab: tab.to_string(),
        kind: if feed == FeedKind::News { ContentKind::News } else if p.has_free_full_text() { ContentKind::FreeFullText } else { ContentKind::AbstractOnly },
        headline: p.title.clone(),
        summary: p.abstract_text.clone().unwrap_or_default(),
        summary_ai: false,
        paper: Some(p.clone()),
        basis: if feed == FeedKind::News { vec![p.clone()] } else { vec![] },
        image: p.image.clone(),
        created: Utc::now(),
    }
}

/// Today's Discover: headlines and short abstracts only, tab by tab (the visible tab first).
/// Cards are stored and sent to the UI as they are made. Full articles are written later,
/// when opened or added to Read Later.
pub fn daily(app: &App, tabs: Vec<String>, force: bool, headline_provider: Option<SharedProvider>, news_provider: Option<SharedProvider>) {
    let bus = app.bus.clone();
    let research = app.research.clone();
    let db = app.db.clone();
    let s = app.settings.clone();
    let cancel = app.daily.cancel.clone();
    app.rt.spawn(async move {
        let today = Utc::now().date_naive();
        let since = today - Days::new(10);
        let feeds = [FeedKind::Research, FeedKind::News];
        let jobs: Vec<(FeedKind, String)> = tabs.iter().flat_map(|t| feeds.iter().map(move |f| (*f, t.clone()))).filter(|(f, t)| force || !db.feed_run_done(today, *f, t)).collect();
        let total = jobs.len();
        if total == 0 {
            bus.send(AppEvent::DailyStatus { running: false, text: String::new(), done: 0, total: 0 });
            return;
        }
        for (i, (feed, tab)) in jobs.into_iter().enumerate() {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            let what = if feed == FeedKind::Research { "research" } else { "news" };
            bus.send(AppEvent::DailyStatus { running: true, text: format!("Writing today's {what} headlines: {tab}"), done: i, total });
            let start_pos = db.feed_count_for_day(feed, &tab, today) as i64;
            let shown: HashSet<String> = db.recent_feed_paper_keys(feed, &tab, today - Days::new(s.feed_days_kept as u64)).into_iter().collect();
            let res = match feed {
                FeedKind::Research => daily_research(&research, &db, &bus, &s, &tab, since, today, start_pos, &shown, headline_provider.clone()).await,
                FeedKind::News => daily_news(&research, &db, &bus, &s, &tab, since, today, start_pos, &shown, news_provider.clone()).await,
            };
            match res {
                Ok(()) => {
                    let _ = db.set_feed_run_done(today, feed, &tab);
                }
                Err(e) => log::warn!("daily {what} for {tab}: {e:#}"),
            }
        }
        bus.send(AppEvent::DailyStatus { running: false, text: String::new(), done: total, total });
    });
}

#[allow(clippy::too_many_arguments)]
async fn daily_research(
    research: &crate::research::Research,
    db: &crate::db::Db,
    bus: &crate::app::Bus,
    s: &crate::settings::Settings,
    tab: &str,
    since: chrono::NaiveDate,
    today: chrono::NaiveDate,
    mut pos: i64,
    shown: &HashSet<String>,
    provider: Option<SharedProvider>,
) -> anyhow::Result<()> {
    let mut papers = research.recent_papers(&topic_query(tab), since, (s.daily_research_per_tab as usize) * 3).await?;
    papers.retain(|p| !shown.contains(&p.key) && p.abstract_text.as_ref().is_some_and(|a| a.len() > 80));
    research.enrich_open_access(&mut papers).await;
    let picked: Vec<Paper> = papers.iter().take(s.daily_research_per_tab as usize).cloned().collect();
    for p in &picked {
        let mut p = p.clone();
        if p.image.is_none() {
            p.image = research.find_image_or_topic(&p.title, Some(&p), tab).await;
        }
        let item = paper_card(&p, FeedKind::Research, tab, today);
        db.put_feed_item(&item, pos)?;
        pos += 1;
        bus.send(AppEvent::FeedItem(item));
    }
    // AI-only articles: syntheses across several of today's papers, clearly labelled.
    if let Some(provider) = provider
        && s.daily_ai_articles_per_tab > 0
        && papers.len() >= 3
    {
        let proposals = writing::propose_ai_articles(provider.clone(), tab, &papers, s.daily_ai_articles_per_tab as usize).await.unwrap_or_default();
        for prop in proposals {
            let basis: Vec<Paper> = papers.iter().filter(|p| prop.basis_keys.contains(&p.key)).cloned().collect();
            if basis.is_empty() {
                continue;
            }
            let image = research.find_image_or_topic(&prop.headline, Some(&basis[0]), tab).await;
            let item = FeedItem {
                id: short_hash(&format!("ai|{tab}|{today}|{}", prop.headline)),
                day: today,
                feed: FeedKind::Research,
                tab: tab.to_string(),
                kind: ContentKind::AiArticle,
                headline: prop.headline,
                summary: prop.summary,
                summary_ai: true,
                paper: None,
                basis,
                image,
                created: Utc::now(),
            };
            // Mix AI articles in after the first few real papers.
            db.put_feed_item(&item, (pos / 2).max(2))?;
            pos += 1;
            bus.send(AppEvent::FeedItem(item));
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn daily_news(
    research: &crate::research::Research,
    db: &crate::db::Db,
    bus: &crate::app::Bus,
    s: &crate::settings::Settings,
    tab: &str,
    since: chrono::NaiveDate,
    today: chrono::NaiveDate,
    mut pos: i64,
    shown: &HashSet<String>,
    provider: Option<SharedProvider>,
) -> anyhow::Result<()> {
    let since = since.max(today - Days::new(3));
    let clusters = research.news_clusters(&topic_query(tab), since, s.daily_news_per_tab as usize + 4).await?;
    let mut made = 0;
    for c in clusters {
        if made >= s.daily_news_per_tab as usize {
            break;
        }
        if c.articles.iter().all(|a| shown.contains(&a.key)) || c.articles.is_empty() {
            continue;
        }
        // AI-written neutral headline and summary when a model is set up; otherwise the
        // outlets' own headline and description, not marked as AI.
        let (headline, summary, ai) = match &provider {
            Some(p) => match writing::news_headline(p.clone(), &c.articles).await {
                Ok((h, s)) => (h, s, true),
                Err(_) => (c.title.clone(), writing::outlet_paragraph(&c.articles), false),
            },
            None => (c.title.clone(), writing::outlet_paragraph(&c.articles), false),
        };
        // Our own relevance-checked search first; the cluster's picture (often an outlet's
        // copyrighted photo, link-only) only as a last resort.
        let image = research.find_image_or_topic(&headline, None, tab).await.or(c.image.clone().filter(|i| !i.embeddable));
        let item = FeedItem {
            id: short_hash(&format!("news|{tab}|{today}|{}", c.title)),
            day: today,
            feed: FeedKind::News,
            tab: tab.to_string(),
            kind: ContentKind::News,
            headline,
            summary,
            summary_ai: ai,
            paper: None,
            basis: c.articles,
            image,
            created: Utc::now(),
        };
        db.put_feed_item(&item, pos)?;
        pos += 1;
        made += 1;
        bus.send(AppEvent::FeedItem(item));
    }
    Ok(())
}

pub fn check_follows(app: &App) {
    let bus = app.bus.clone();
    let research = app.research.clone();
    let db = app.db.clone();
    let follows = app.settings.follows.clone();
    app.rt.spawn(async move {
        let since = Utc::now().date_naive() - Days::new(7);
        let mut added = 0;
        for f in follows {
            let (label, papers) = match &f {
                Follow::Topic(t) => (format!("New in {t}"), research.new_for_follow(&f, since).await.unwrap_or_default()),
                Follow::Author(a) => (format!("New from {a}"), research.new_for_follow(&f, since).await.unwrap_or_default()),
            };
            for p in papers.into_iter().take(10) {
                let dedupe = format!("{:?}|{}", f, p.key);
                if db.add_notification(&label, &p.title, Some(&p), &dedupe).unwrap_or(false) {
                    added += 1;
                }
            }
        }
        bus.send(AppEvent::Notified(added));
    });
}

pub fn export(app: &App, kind: ExportKind, article: Article) {
    let bus = app.bus.clone();
    let style = app.settings.citation_style;
    app.rt.spawn(async move {
        let stem: String = article.title.chars().filter(|c| c.is_alphanumeric() || *c == ' ' || *c == '-').take(60).collect::<String>().trim().replace(' ', "-");
        let stem = if stem.is_empty() { "article".to_string() } else { stem };
        let result: anyhow::Result<String> = async {
            match kind {
                ExportKind::Print => Ok(crate::export::print(&article, style)?),
                ExportKind::Email => {
                    crate::export::email(&article, style)?;
                    Ok("Opened your mail client".to_string())
                }
                ExportKind::Txt | ExportKind::Docx | ExportKind::Pdf => {
                    let (ext, name) = match kind {
                        ExportKind::Txt => ("txt", "Text"),
                        ExportKind::Docx => ("docx", "Word document"),
                        _ => ("pdf", "PDF"),
                    };
                    // TODO(windows): rfd's xdg-portal backend is Linux-only; Windows needs its default backend.
                    let Some(file) = rfd::AsyncFileDialog::new().set_file_name(format!("{stem}.{ext}")).add_filter(name, &[ext]).save_file().await else {
                        return Ok(String::new());
                    };
                    let path = file.path().to_path_buf();
                    match kind {
                        ExportKind::Txt => std::fs::write(&path, crate::export::to_txt(&article, style))?,
                        ExportKind::Docx => crate::export::to_docx(&article, style, &path)?,
                        _ => crate::export::to_pdf(&article, style, &path)?,
                    }
                    Ok(format!("Saved {}", path.display()))
                }
            }
        }
        .await;
        match result {
            Ok(msg) if msg.is_empty() => {}
            Ok(msg) => bus.toast(msg, ToastKind::Success),
            Err(e) => bus.toast(format!("Export failed: {e:#}"), ToastKind::Error),
        }
    });
}

/// Save a bibliography (text, BibTeX or RIS) chosen in the Bibliography view.
pub fn save_text_file(app: &App, default_name: String, filter: (&'static str, &'static str), contents: String) {
    let bus = app.bus.clone();
    app.rt.spawn(async move {
        let Some(file) = rfd::AsyncFileDialog::new().set_file_name(&default_name).add_filter(filter.0, &[filter.1]).save_file().await else {
            return;
        };
        match std::fs::write(file.path(), contents) {
            Ok(()) => bus.toast(format!("Saved {}", file.path().display()), ToastKind::Success),
            Err(e) => bus.toast(format!("Could not save: {e}"), ToastKind::Error),
        }
    });
}

#[cfg(test)]
mod tests {
    use crate::model::*;

    /// Seed a data directory with a sample written article, for looking at the reader:
    /// `GATELESS_SEED_DIR=/path cargo test seed_demo -- --ignored`, then
    /// `XDG_DATA_HOME=/path gateless --open doi:10.1234/demo`.
    #[test]
    #[ignore]
    fn seed_demo() {
        let Ok(dir) = std::env::var("GATELESS_SEED_DIR") else { return };
        let db = crate::db::Db::open(&std::path::Path::new(&dir).join("gateless")).unwrap();
        let article = crate::export::tests::sample_article();
        let paper = Paper {
            key: "doi:10.1234/demo".into(),
            doi: Some("10.1234/demo".into()),
            title: article.original_title.clone().unwrap_or_else(|| "Demo paper".into()),
            abstract_text: Some("We report a demonstration abstract used to check how the reader shows original text, AI-reworded originals and AI-written passages, each with its own marking.".into()),
            authors: article.original_authors.clone(),
            year: Some(2025),
            venue: Some("Journal of Demonstrations".into()),
            url: article.original_url.clone(),
            kind: ContentKind::AbstractOnly,
            ..Default::default()
        };
        let doc = Doc::Paper(paper);
        db.put_article(&doc.key(), &article).unwrap();
        db.record_history(&doc).unwrap();
        let today = chrono::Utc::now().date_naive();
        let mk = |i: usize, kind: ContentKind, title: &str| {
            let p = Paper {
                key: format!("doi:10.1234/demo{i}"),
                doi: Some(format!("10.1234/demo{i}")),
                title: title.into(),
                abstract_text: Some("Long abstract text that goes on for a while to check that the card shows only two lines of it with an ellipsis and a Show more link, which expands the full abstract in place without leaving the feed. It continues further still so it must wrap.".into()),
                authors: vec!["Ada Lovelace".into(), "Alan Turing".into(), "Grace Hopper".into(), "Katherine Johnson".into(), "Claude Shannon".into()],
                year: Some(2026),
                venue: Some("Nature Astronomy".into()),
                cited_by_count: Some(12),
                oa_url: (kind == ContentKind::FreeFullText).then(|| "https://arxiv.org/abs/0000.0000".into()),
                kind,
                ..Default::default()
            };
            FeedItem { id: format!("demo{i}"), day: today, feed: FeedKind::Research, tab: "For You".into(), kind, headline: p.title.clone(), summary: p.abstract_text.clone().unwrap_or_default(), summary_ai: false, paper: Some(p), basis: vec![], image: None, created: chrono::Utc::now() }
        };
        db.put_feed_item(&mk(1, ContentKind::FreeFullText, "JWST spots water vapour in a temperate sub-Neptune atmosphere"), 0).unwrap();
        db.put_feed_item(&mk(2, ContentKind::AbstractOnly, "Self-healing concrete using bacterial spores survives freeze–thaw cycles"), 1).unwrap();
        let mut ai = mk(3, ContentKind::AiArticle, "Why three new studies suggest dark energy may be weakening");
        ai.paper = None;
        ai.summary_ai = true;
        ai.basis = vec![mk(4, ContentKind::AbstractOnly, "DESI DR2 baryon acoustic oscillations").paper.unwrap()];
        db.put_feed_item(&ai, 2).unwrap();
        let mut s = db.load_settings();
        s.welcomed = true;
        s.auto_daily = false;
        db.save_settings(&s).unwrap();
    }

    /// End to end: real search → research → write with local Ollama → save.
    /// `GATELESS_SEED_DIR=/path cargo test e2e_generate -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn e2e_generate() {
        let Ok(dir) = std::env::var("GATELESS_SEED_DIR") else { return };
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let db = crate::db::Db::open(&std::path::Path::new(&dir).join("gateless")).unwrap();
            let research = crate::research::Research::new(db.clone(), crate::research::ResearchConfig { news_feeds: crate::settings::default_news_feeds(), ..Default::default() });
            let t = std::time::Instant::now();
            let q = SearchQuery { text: std::env::var("GATELESS_Q").unwrap_or("CRISPR base editing cholesterol".into()), per_page: 20, ..Default::default() };
            let papers = research.search(&q).await.unwrap();
            println!("search: {} papers in {:?}", papers.len(), t.elapsed());
            let paper = papers.iter().find(|p| p.kind == ContentKind::AbstractOnly && p.abstract_text.as_ref().is_some_and(|a| a.len() > 300)).or(papers.first()).unwrap().clone();
            println!("target: {} [{}] {:?}", paper.title, paper.key, paper.kind);
            let (ptx, mut prx) = tokio::sync::mpsc::unbounded_channel();
            tokio::spawn(async move { while let Some(p) = prx.recv().await { let p: crate::research::Progress = p; println!("  research {:.0}% {}", p.fraction * 100.0, p.text); } });
            let pack = research.gather_for_paper(&paper, &[], ptx).await.unwrap();
            println!("pack: {} sources, {} chunks, full text: {}, images: {} ({:?})", pack.sources.len(), pack.chunks.len(), pack.full_text.is_some(), pack.images.len(), t.elapsed());
            let mut cfg = crate::providers::ProviderConfig::new(crate::providers::ProviderKind::Ollama);
            cfg.model = std::env::var("GATELESS_MODEL").unwrap_or("mistral-nemo:latest".into());
            let provider = crate::providers::build(&cfg).unwrap();
            let kind = if pack.full_text.is_some() { ArticleKind::FullText } else { ArticleKind::Interpretation };
            let req = crate::writing::ArticleRequest { kind, target: Some(paper.clone()), topic: paper.title.clone(), brief: None, length: crate::writing::Length::OnePage, allow_tools: false };
            let (wtx, mut wrx) = tokio::sync::mpsc::unbounded_channel();
            tokio::spawn(async move { while let Some(e) = wrx.recv().await { if let crate::writing::WriteEvent::Stage { text, .. } = e { println!("  write: {text}"); } } });
            let article = crate::writing::write_article(provider, req, pack, wtx).await.unwrap();
            println!("article: {:?} '{}' — {} sections, {} passages, validation {:?} ({:?})", article.kind, article.title, article.sections.len(), article.passages().count(), article.validation, t.elapsed());
            let doc = Doc::Paper(paper);
            db.put_article(&doc.key(), &article).unwrap();
            db.record_history(&doc).unwrap();
            let mut s = db.load_settings();
            s.welcomed = true;
            s.auto_daily = false;
            s.providers = vec![cfg];
            db.save_settings(&s).unwrap();
            println!("OPEN KEY: {}", doc.key());
        });
    }

    /// Learn AI on a stored article (abstract and whole article) with local Ollama.
    /// `GATELESS_SEED_DIR=/path GATELESS_KEY=<doc key> cargo test e2e_learn -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn e2e_learn() {
        let (Ok(dir), Ok(key)) = (std::env::var("GATELESS_SEED_DIR"), std::env::var("GATELESS_KEY")) else { return };
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let db = crate::db::Db::open(&std::path::Path::new(&dir).join("gateless")).unwrap();
            let doc = db.find_doc(&key).unwrap();
            let article = db.get_article(&key).unwrap();
            let mut cfg = crate::providers::ProviderConfig::new(crate::providers::ProviderKind::Ollama);
            cfg.model = std::env::var("GATELESS_MODEL").unwrap_or("mistral-nemo:latest".into());
            let provider = crate::providers::build(&cfg).unwrap();
            for (lk, input) in [
                (crate::app::learn_key(&key, true), crate::writing::learn::LearnInput::from_abstract(&doc)),
                (crate::app::learn_key(&key, false), crate::writing::learn::LearnInput::from_article(&article)),
            ] {
                let t = std::time::Instant::now();
                let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
                let a = crate::writing::learn::explain(provider.clone(), input, tx).await.unwrap();
                println!("{lk}: {} sections in {:?}, validation {:?}", a.sections.len(), t.elapsed(), a.validation);
                for s in a.sections.iter().take(2) {
                    println!("  ## {}", s.heading);
                    for b in s.blocks.iter().take(1) {
                        if let Block::Paragraph(ps) = b {
                            println!("  {}", ps.iter().map(|p| format!("{}{:?}", p.text.trim(), p.citations)).collect::<Vec<_>>().join(" "));
                        }
                    }
                }
                db.put_article(&lk, &a).unwrap();
            }
        });
    }
}
