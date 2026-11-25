//! The writing pipeline. The model writes only from the numbered sources it is given, citing
//! inline; the code then validates: every citation must map to a real source, invented
//! references are removed, uncited factual sentences are flagged "unverified", rewording of
//! originals is limited to flow (too-different rewordings are reverted to the original), and
//! chart numbers must appear in the cited sources.
//!
//! Every passage produced carries its `Origin`.
//!
//! INTERFACE FILE: the public functions here are what the app calls.
//!
//! Strategy:
//! - Interpretation / AiOnly / News: one short JSON call plans the outline (title, standfirst,
//!   sections). Strong models then write the whole article in ONE streamed call with `## `
//!   section headings; each section is validated and emitted as soon as the next heading starts.
//!   Small models (`provider.is_small()`) write section by section (one call per section), each
//!   given only the excerpts most relevant to that section.
//! - FullText: the original paragraphs, reworded lightly in batches (6 paragraphs per call for
//!   strong models, 2 for small ones, with a paragraph budget); every reworded sentence is checked
//!   against the original and reverted when it drifts.

pub mod charts;
pub mod json;
pub mod learn;
pub mod parse;
pub mod prompts;
pub mod sentences;
pub mod validate;

use std::collections::HashSet;

use anyhow::Result;
use chrono::{Datelike, Utc};
use serde_json::Value;
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};

use crate::model::*;
use crate::providers::{CompletionRequest, SharedProvider};

use json::{extract_json, int_list, str_field, str_list};
use parse::{SectionStreamer, ValidationContext, body_to_blocks, is_reference_heading, strip_leading_heading};
use prompts::*;
use sentences::extract_citations;
use validate::{ai_passages, align_rewording, strip_markdown};

/// Streamed while an article is written; the reader shows sections as they arrive.
#[derive(Clone, Debug)]
pub enum WriteEvent {
    /// Progress text and 0.0..=1.0 fraction of the writing phase ("Writing: Methods…").
    Stage { text: String, fraction: f32 },
    /// Raw model text as it streams (for a live preview of the section being written).
    Delta(String),
    /// A finished, validated section (append to the reader).
    Section(Section),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Length {
    /// Full journal-article length (research articles): ~3,000–6,000 words.
    Journal,
    /// About one page (news stories): ~500–800 words.
    OnePage,
}

#[derive(Clone, Debug)]
pub struct ArticleRequest {
    pub kind: ArticleKind,
    /// The paper (Interpretation / FullText).
    pub target: Option<Paper>,
    /// Topic or headline (AiOnly / News), or the paper title.
    pub topic: String,
    /// For AiOnly/News: the AI-written summary from the feed card, as a brief.
    pub brief: Option<String>,
    pub length: Length,
    /// Let a CLI provider do extra research itself (sources are still always supplied).
    pub allow_tools: bool,
}

// ---------------------------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------------------------

fn stage(events: &UnboundedSender<WriteEvent>, text: impl Into<String>, fraction: f32) {
    let _ = events.send(WriteEvent::Stage { text: text.into(), fraction: fraction.clamp(0.0, 1.0) });
}

/// One completion whose deltas are forwarded as `WriteEvent::Delta`.
async fn call_forwarding(provider: &SharedProvider, req: &CompletionRequest, events: &UnboundedSender<WriteEvent>) -> Result<String> {
    let (tx, mut rx) = unbounded_channel::<String>();
    let fut = provider.complete(req, Some(tx));
    let forward = async {
        while let Some(d) = rx.recv().await {
            let _ = events.send(WriteEvent::Delta(d));
        }
    };
    let (res, ()) = tokio::join!(fut, forward);
    res
}

/// A JSON completion: provider errors propagate; an unreadable reply is retried once, then
/// gives `Ok(None)` so callers can fall back.
async fn complete_json(provider: &SharedProvider, system: &str, prompt: &str, max_tokens: u32) -> Result<Option<Value>> {
    let mut req = CompletionRequest { system: system.to_string(), prompt: prompt.to_string(), max_tokens, temperature: 0.2, json: true, allow_tools: false };
    let reply = provider.complete(&req, None).await?;
    if let Some(v) = extract_json(&reply) {
        return Ok(Some(v));
    }
    log::warn!("unreadable JSON reply, retrying: {}", prompts::truncate_chars(&reply, 200));
    req.prompt.push_str("\n\nYour previous reply was not valid JSON. Reply with ONE valid JSON object only.");
    let reply = provider.complete(&req, None).await?;
    Ok(extract_json(&reply))
}

fn plain(s: &str) -> String {
    strip_markdown(&extract_citations(s).0).trim_matches(['"', '“', '”']).trim().to_string()
}

fn valid_ids(sources: &[Source]) -> HashSet<u32> {
    sources.iter().map(|s| s.id).collect()
}

/// Hero image (first embeddable) and the rest, in order.
fn split_images(images: &[ImageRef]) -> (Option<ImageRef>, Vec<ImageRef>) {
    let hero_idx = images.iter().position(|i| i.embeddable);
    let hero = hero_idx.map(|i| images[i].clone());
    let rest = images.iter().enumerate().filter(|(i, _)| Some(*i) != hero_idx).map(|(_, im)| im.clone()).collect();
    (hero, rest)
}

/// Put an image after the section's first paragraph.
fn place_image(section: &mut Section, image: ImageRef) {
    let pos = section.blocks.iter().position(|b| matches!(b, Block::Paragraph(_))).map(|i| i + 1).unwrap_or(section.blocks.len());
    section.blocks.insert(pos, Block::Image(image));
}

/// Shared state while an article is assembled.
struct Assembler<'a> {
    article: Article,
    ctx: ValidationContext,
    images: Vec<ImageRef>,
    events: &'a UnboundedSender<WriteEvent>,
    planned: Vec<String>,
}

impl Assembler<'_> {
    /// Validate a model-written section body and emit it.
    fn finish_generated(&mut self, heading: &str, body: &str) {
        if is_reference_heading(heading) {
            self.article.validation.notes.push("A model-written reference list was discarded (references come from the research sources only).".into());
            return;
        }
        let blocks = body_to_blocks(body, &self.ctx, &mut self.article.validation);
        let words: usize = blocks
            .iter()
            .map(|b| if let Block::Paragraph(ps) = b { ps.iter().map(|p| p.text.split_whitespace().filter(|w| w.chars().any(char::is_alphabetic)).count()).sum() } else { 0 })
            .sum();
        if words < 3 {
            self.article.validation.notes.push(format!("Section \"{heading}\" came back empty and was left out."));
            return;
        }
        self.push_section(Section { heading: strip_markdown(heading), blocks });
    }

    fn push_section(&mut self, mut section: Section) {
        let idx = self.article.sections.len();
        if idx >= 1 && idx - 1 < self.images.len() {
            place_image(&mut section, self.images[idx - 1].clone());
        }
        let _ = self.events.send(WriteEvent::Section(section.clone()));
        self.article.sections.push(section);
    }
}

// ---------------------------------------------------------------------------------------------
// Articles
// ---------------------------------------------------------------------------------------------

struct Outline {
    title: Option<String>,
    standfirst: Option<String>,
    sections: Vec<(String, String)>,
}

fn default_sections(kind: ArticleKind, length: Length) -> Vec<(&'static str, &'static str)> {
    use ArticleKind::*;
    match (kind, length) {
        (Interpretation, Length::Journal) => vec![
            ("Overview", "what the topic is and why it matters, in plain language"),
            ("Background", "the wider field and earlier work the paper builds on"),
            ("What the paper reports", "what the abstract says the authors did and found, attributed to them"),
            ("How other researchers describe it", "how citing papers describe and use the work"),
            ("Methods in context", "the kinds of methods involved and how they compare with related work"),
            ("Implications", "why the findings could matter, for science and for people"),
            ("Limitations and open questions", "what is uncertain, what the abstract cannot tell us, what remains open"),
        ],
        (Interpretation, Length::OnePage) => vec![
            ("Overview", "what the topic is and why it matters"),
            ("What the paper reports", "what the abstract says the authors did and found"),
            ("How other researchers describe it", "how citing and related papers see the work"),
            ("Open questions", "what is uncertain or still open"),
        ],
        (News, Length::OnePage) => vec![
            ("What happened", "the core facts of the event"),
            ("Key details", "numbers, places, people and timeline, attributed to outlets"),
            ("Reactions and context", "responses and background, attributed to outlets"),
            ("What comes next", "announced next steps and open questions"),
        ],
        (News, Length::Journal) => vec![
            ("What happened", "the core facts of the event"),
            ("Timeline", "how events unfolded"),
            ("Key details", "numbers, places and people, attributed to outlets"),
            ("Reactions", "responses from those involved, attributed to outlets"),
            ("Background", "context that explains the event"),
            ("Where reports differ", "points on which outlets disagree"),
            ("What comes next", "announced next steps and open questions"),
        ],
        (_, Length::OnePage) => vec![
            ("What's new", "the main development the sources describe"),
            ("Key findings", "the most important results, compared across sources"),
            ("Why it matters", "implications"),
            ("Open questions", "what remains uncertain"),
        ],
        (_, Length::Journal) => vec![
            ("Introduction", "what the topic is and why it matters"),
            ("Background", "earlier work and key concepts"),
            ("Key findings", "the main results across the sources"),
            ("Methods and approaches", "how the researchers studied it"),
            ("Agreements and differences", "where the sources agree or disagree"),
            ("Implications", "why it matters"),
            ("Open questions", "limitations and what remains open"),
        ],
    }
}

fn length_words(length: Length) -> (usize, &'static str) {
    match length {
        Length::Journal => (4500, "full journal-article length"),
        Length::OnePage => (800, "about one page"),
    }
}

async fn plan_outline(provider: &SharedProvider, req: &ArticleRequest, pack: &SourcePack) -> Result<Outline> {
    let defaults = default_sections(req.kind, req.length);
    let titles: String = pack
        .sources
        .iter()
        .take(25)
        .map(|s| format!("[{}] {}{}\n", s.id, truncate_chars(&s.title, 150), s.venue.as_ref().map(|v| format!(" ({v})")).unwrap_or_default()))
        .collect();
    let brief = req.brief.as_ref().map(|b| format!("BRIEF: {b}\n")).unwrap_or_default();
    let (prompt, fixed) = if req.kind == ArticleKind::Interpretation {
        let list: String = defaults.iter().enumerate().map(|(i, (h, f))| format!("{}. {h} ({f})\n", i + 1)).collect();
        (
            format!(
                "{}\nRELATED SOURCES:\n{titles}\nWe are writing an explainer about the topic of this paper (we only have its abstract). \
                 It has these sections:\n{list}\nReply with a JSON object:\n{{\"title\": \"a short plain-language title about the TOPIC (not the paper's own title)\", \
                 \"standfirst\": \"one sentence saying what the explainer covers\", \"focus\": [\"one line on what section 1 should cover, based on the sources\", \"…one per section\"]}}",
                target_block(req.target.as_ref())
            ),
            true,
        )
    } else {
        let n = if req.length == Length::Journal { "6 to 8" } else { "3 or 4" };
        let headline_rule = if req.kind == ArticleKind::News { "a neutral, factual news headline (no opinion words)" } else { "a clear, engaging plain-language title" };
        (
            format!(
                "TOPIC: {}\n{brief}SOURCES:\n{titles}\nPlan an article ({}) on this topic written only from these sources. Reply with a JSON object:\n\
                 {{\"title\": \"{headline_rule}\", \"standfirst\": \"one sentence summary\", \"sections\": [{{\"heading\": \"short section heading\", \"focus\": \"what it covers\"}}]}}\n\
                 Use {n} sections. Do not include a references section.",
                req.topic,
                length_words(req.length).1
            ),
            false,
        )
    };
    let v = complete_json(provider, "You plan articles. Reply with JSON only.", &prompt, 900).await?;
    let v = v.unwrap_or(Value::Null);
    let title = str_field(&v, "title").map(|t| plain(&t)).filter(|t| !t.is_empty());
    let standfirst = str_field(&v, "standfirst").map(|t| plain(&t)).filter(|t| !t.is_empty());
    let sections: Vec<(String, String)> = if fixed {
        let focus = str_list(&v, "focus");
        defaults.iter().enumerate().map(|(i, (h, f))| (h.to_string(), focus.get(i).cloned().unwrap_or_else(|| f.to_string()))).collect()
    } else {
        let mut s: Vec<(String, String)> = v
            .get("sections")
            .and_then(|s| s.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| match x {
                        Value::String(h) => Some((plain(h), String::new())),
                        _ => str_field(x, "heading").or_else(|| str_field(x, "title")).map(|h| (plain(&h), str_field(x, "focus").unwrap_or_default())),
                    })
                    .filter(|(h, _)| !h.is_empty() && !is_reference_heading(h))
                    .collect()
            })
            .unwrap_or_default();
        s.truncate(9);
        if s.len() < 2 { defaults.iter().map(|(h, f)| (h.to_string(), f.to_string())).collect() } else { s }
    };
    Ok(Outline { title, standfirst, sections })
}

fn base_article(provider: &SharedProvider, req: &ArticleRequest, pack: &SourcePack) -> Article {
    let target = req.target.as_ref();
    let mut sources = pack.sources.clone();
    sources.sort_by_key(|s| s.id);
    sources.dedup_by_key(|s| s.id);
    Article {
        id: uuid::Uuid::new_v4().to_string(),
        kind: req.kind,
        paper_key: target.map(|p| p.key.clone()),
        title: String::new(),
        standfirst: None,
        original_title: target.map(|p| p.title.clone()),
        original_authors: target.map(|p| p.authors.clone()).unwrap_or_default(),
        original_url: target.and_then(|p| p.url.clone().or_else(|| p.doi.as_ref().map(|d| format!("https://doi.org/{d}"))).or_else(|| p.oa_url.clone())),
        byline: format!("Gateless Scholar + {}", provider.brand()),
        generated_by: provider.describe(),
        created: Utc::now(),
        sections: vec![],
        sources,
        hero_image: None,
        validation: ValidationReport::default(),
    }
}

fn interpretation_title(model_title: Option<&str>, req: &ArticleRequest) -> String {
    let original = req.target.as_ref().map(|p| p.title.trim().to_lowercase()).unwrap_or_default();
    let strip = |t: &str| {
        let t = t.trim();
        let lower = t.to_lowercase();
        for p in ["ai interpretation:", "ai interpretation –", "ai interpretation -", "ai interpretation"] {
            if lower.starts_with(p) {
                return t[p.len()..].trim().to_string();
            }
        }
        t.to_string()
    };
    // Never the original title (or a close variant of it) presented as ours.
    let too_close = |t: &str| {
        let l = t.to_lowercase();
        !original.is_empty() && (l == original || l.contains(&original) || validate::token_similarity(&l, &original) >= 0.6)
    };
    let candidate = model_title.map(strip).filter(|t| !t.is_empty() && !too_close(t));
    let topic = candidate.unwrap_or_else(|| {
        let topic = req.topic.trim();
        if !topic.is_empty() && topic.to_lowercase() != original {
            topic.to_string()
        } else {
            format!("the topic of “{}”", req.target.as_ref().map(|p| p.title.trim()).unwrap_or(topic))
        }
    });
    format!("AI interpretation: {topic}")
}

/// Write a whole article from the gathered sources, streaming sections via `events`.
/// Returns the final validated article (also containing every section already streamed).
pub async fn write_article(provider: SharedProvider, req: ArticleRequest, pack: SourcePack, events: UnboundedSender<WriteEvent>) -> Result<Article> {
    let mut req = req;
    let mut early_notes = Vec::new();
    if req.kind == ArticleKind::FullText && pack.full_text.as_ref().is_none_or(|f| f.sections.iter().all(|s| s.paragraphs.is_empty())) {
        req.kind = ArticleKind::Interpretation;
        early_notes.push("No readable free full text was found, so an AI interpretation was written instead.".to_string());
    }
    let mut article = base_article(&provider, &req, &pack);
    article.validation.notes.extend(early_notes);
    let (hero, rest) = split_images(&pack.images);
    article.hero_image = hero;
    let ctx = ValidationContext { valid: valid_ids(&article.sources), chunks: pack.chunks.clone() };
    let mut asm = Assembler { article, ctx, images: rest, events: &events, planned: vec![] };

    if req.kind == ArticleKind::FullText {
        write_full_text(&provider, &req, &pack, &mut asm).await?;
    } else {
        if pack.sources.is_empty() {
            anyhow::bail!("no sources were found to write from, so nothing was generated");
        }
        write_generated(&provider, &req, &pack, &mut asm).await?;
    }

    stage(&events, "Checking citations…", 0.97);
    let mut article = asm.article;
    if article.sections.is_empty() {
        anyhow::bail!("the model returned no usable text; try again or choose a stronger model");
    }
    let cited: HashSet<u32> = article.passages().flat_map(|p| p.citations.iter().copied()).collect();
    if req.kind != ArticleKind::FullText {
        let uncited: Vec<String> = article.sources.iter().filter(|s| !cited.contains(&s.id)).map(|s| format!("[{}]", s.id)).collect();
        if !uncited.is_empty() {
            let what = if req.kind == ArticleKind::News { "Outlets" } else { "Sources" };
            article.validation.notes.push(format!("{what} not cited in the text: {}.", uncited.join(", ")));
        }
        let v = &article.validation;
        if v.invalid_citations_removed > 0 {
            article.validation.notes.push(format!("{} citation(s) to sources that do not exist were removed.", v.invalid_citations_removed));
        }
        let v = &article.validation;
        if v.unverified_sentences > 0 {
            article.validation.notes.push(format!("{} sentence(s) make claims without a valid source and are marked unverified.", v.unverified_sentences));
        }
    }
    stage(&events, "Done", 1.0);
    Ok(article)
}

async fn write_generated(provider: &SharedProvider, req: &ArticleRequest, pack: &SourcePack, asm: &mut Assembler<'_>) -> Result<()> {
    let events = asm.events;
    stage(events, "Planning sections…", 0.02);
    let outline = plan_outline(provider, req, pack).await?;
    asm.article.title = match req.kind {
        ArticleKind::Interpretation => interpretation_title(outline.title.as_deref(), req),
        _ => outline.title.clone().unwrap_or_else(|| req.topic.trim().to_string()),
    };
    asm.article.standfirst = outline.standfirst.clone().or_else(|| req.brief.as_ref().map(|b| plain(b)));
    asm.planned = outline.sections.iter().map(|s| s.0.clone()).collect();
    let n = outline.sections.len().max(1);
    let (words, _) = length_words(req.length);
    let brief = req.brief.as_ref().map(|b| format!("BRIEF: {b}\n")).unwrap_or_default();
    let topic_line = format!("TOPIC: {}\n{}{brief}", req.topic, target_block(req.target.as_ref()));
    let has_full = pack.full_text.is_some();

    if provider.is_small() {
        let per_section = (words / n).max(120);
        let compact_sources = source_list(&pack.sources, has_full);
        let outline_list: String = outline.sections.iter().enumerate().map(|(i, (h, _))| format!("{}. {h}\n", i + 1)).collect();
        for (i, (heading, focus)) in outline.sections.iter().enumerate() {
            stage(events, format!("Writing: {heading} ({}/{n})…", i + 1), 0.05 + 0.9 * i as f32 / n as f32);
            let mut chunks = rerank_for_section(&pack.chunks, &format!("{heading} {focus} {}", req.topic), 6);
            if req.kind == ArticleKind::Interpretation {
                let target_ids: HashSet<u32> = pack.sources.iter().filter(|s| s.kind == SourceKind::Target).map(|s| s.id).collect();
                if !chunks.iter().any(|c| target_ids.contains(&c.source_id)) {
                    if let Some(c) = pack.chunks.iter().find(|c| target_ids.contains(&c.source_id)) {
                        chunks.insert(0, c.clone());
                    }
                }
            }
            let prompt = format!(
                "{topic_line}\nARTICLE: \"{}\" ({})\nSECTIONS:\n{outline_list}\n{}\n\nSOURCES:\n{compact_sources}\nEXCERPTS (each starts with its source number):\n{}\n\
                 TASK: Write section {} \"{heading}\" ({focus}). About {per_section} words in {} paragraphs. Cite a source number in square brackets after every fact. \
                 Only cover this section's topic. Start directly with the text.",
                asm.article.title,
                req.kind.label(),
                kind_instructions(req.kind),
                excerpt_list(&chunks, 7000, 900),
                i + 1,
                if per_section > 250 { "3 or 4" } else { "2 or 3" },
            );
            let creq = CompletionRequest {
                system: SMALL_SYSTEM.to_string(),
                prompt,
                max_tokens: (per_section as u32 * 2 + 300).min(2400),
                temperature: 0.3,
                json: false,
                allow_tools: false,
            };
            let text = match call_forwarding(provider, &creq, events).await {
                Ok(t) => t,
                Err(e) => {
                    log::warn!("section {heading} failed, retrying: {e}");
                    call_forwarding(provider, &creq, events).await?
                }
            };
            let body = strip_leading_heading(&text, heading);
            asm.finish_generated(heading, &body);
        }
        return Ok(());
    }

    // Strong model: one streamed call for the whole article.
    let (budget, per_chunk, max_tokens) = match req.length {
        Length::Journal => (60_000, 1500, 16_000),
        Length::OnePage => (24_000, 1000, 3_000),
    };
    let outline_list: String = outline.sections.iter().enumerate().map(|(i, (h, f))| format!("{}. {h}: {f}\n", i + 1)).collect();
    let prompt = format!(
        "{topic_line}\nSOURCES:\n{}\nEXCERPTS (each starts with its source number):\n{}\n\
         TASK: Write the article \"{}\" ({}), about {words} words in total, in these {n} sections, in this order:\n{outline_list}\n\
         FORMAT: Start each section with a line \"## <section heading>\" (the heading only), then prose paragraphs separated by blank lines. \
         You may use \"### \" sub-headings inside long sections. No bullet lists, no title line, no reference list. Cite source numbers in \
         square brackets after every fact.\n{}\n\nStart directly with the first \"## \" heading.",
        source_list(&pack.sources, has_full),
        excerpt_list(&pack.chunks, budget, per_chunk),
        asm.article.title,
        req.kind.label(),
        CHART_INSTRUCTIONS,
    );
    let creq = CompletionRequest { system: system_prompt(req.kind), prompt, max_tokens, temperature: 0.3, json: false, allow_tools: req.allow_tools };
    stage(events, format!("Writing: {} (1/{n})…", outline.sections[0].0), 0.05);

    let (tx, mut rx) = unbounded_channel::<String>();
    let fut = provider.complete(&creq, Some(tx));
    let mut streamer = SectionStreamer::default();
    let expected_chars = (words * 6) as f32;
    let consume = async {
        let mut chars = 0usize;
        while let Some(d) = rx.recv().await {
            chars += d.len();
            let _ = events.send(WriteEvent::Delta(d.clone()));
            for (h, body) in streamer.push(&d) {
                asm.finish_generated(&h, &body);
                let k = asm.article.sections.len();
                let next = asm.planned.get(k).cloned().unwrap_or_else(|| "next section".into());
                let frac = 0.05 + 0.9 * (k as f32 / n as f32).max(chars as f32 / expected_chars).min(0.95);
                stage(events, format!("Writing: {next} ({}/{n})…", (k + 1).min(n)), frac);
            }
        }
    };
    let (res, ()) = tokio::join!(fut, consume);
    let full = res?;
    let fallback = outline.sections.first().map(|s| s.0.clone()).unwrap_or_else(|| "Overview".into());
    for (h, body) in streamer.finish(&full, &fallback) {
        asm.finish_generated(&h, &body);
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Full text: original paragraphs, lightly reworded for flow
// ---------------------------------------------------------------------------------------------

const REWORD_SYSTEM: &str = "You are a copy editor. Lightly improve the flow and readability of each paragraph while keeping its meaning, \
facts, numbers, technical terms and sentence order exactly. Change as little as possible: keep each sentence as one sentence, do not add, \
remove, merge or split sentences, and do not add information. Keep reference markers such as [12] or (Smith, 2020) unchanged. If a \
paragraph already reads well, return it unchanged.";

fn parse_marked_paragraphs(reply: &str) -> Vec<(usize, String)> {
    let re = regex::Regex::new(r"@@\s*(\d+)\s*@@").expect("regex");
    let marks: Vec<(usize, usize, usize)> = re
        .captures_iter(reply)
        .filter_map(|c| {
            let m = c.get(0)?;
            Some((m.start(), m.end(), c[1].parse().ok()?))
        })
        .collect();
    let mut out = Vec::new();
    for (i, &(_, end, n)) in marks.iter().enumerate() {
        let stop = marks.get(i + 1).map(|m| m.0).unwrap_or(reply.len());
        let text = reply[end..stop].trim();
        if !text.is_empty() {
            out.push((n, text.split_whitespace().collect::<Vec<_>>().join(" ")));
        }
    }
    out
}

async fn write_full_text(provider: &SharedProvider, req: &ArticleRequest, pack: &SourcePack, asm: &mut Assembler<'_>) -> Result<()> {
    let events = asm.events;
    let Some(ft) = pack.full_text.clone() else { anyhow::bail!("no full text") };
    asm.article.title = req.target.as_ref().map(|p| p.title.clone()).filter(|t| !t.trim().is_empty()).unwrap_or_else(|| req.topic.clone());
    let small = provider.is_small();
    let (mut budget, batch, max_words) = if small { (40usize, 2usize, 150usize) } else { (150, 6, 400) };
    let total_paras: usize = ft.sections.iter().map(|s| s.paragraphs.len()).sum();
    let n = ft.sections.len().max(1);
    let mut enabled = true;
    let mut failures = 0;
    let mut reworded_count = 0usize;
    stage(events, "Preparing the original text…", 0.02);

    for (si, sec) in ft.sections.iter().enumerate() {
        let heading = if sec.heading.trim().is_empty() { if si == 0 { "Text".to_string() } else { format!("Part {}", si + 1) } } else { sec.heading.trim().to_string() };
        stage(events, format!("Rewording for flow: {heading} ({}/{n})…", si + 1), 0.03 + 0.92 * si as f32 / n as f32);
        let refs = is_reference_heading(&heading);
        let mut reworded: Vec<Option<String>> = vec![None; sec.paragraphs.len()];
        let eligible: Vec<usize> = sec
            .paragraphs
            .iter()
            .enumerate()
            .filter(|(_, p)| {
                let w = p.split_whitespace().count();
                (12..=max_words).contains(&w)
            })
            .map(|(i, _)| i)
            .collect();
        if enabled && !refs {
            for group in eligible.chunks(batch) {
                if budget == 0 || !enabled {
                    break;
                }
                let group: Vec<usize> = group.iter().copied().take(budget).collect();
                budget -= group.len();
                let mut body = String::new();
                for (k, &pi) in group.iter().enumerate() {
                    body.push_str(&format!("@@{}@@\n{}\n\n", k + 1, sec.paragraphs[pi].trim()));
                }
                let chars: usize = group.iter().map(|&pi| sec.paragraphs[pi].len()).sum();
                let creq = CompletionRequest {
                    system: REWORD_SYSTEM.to_string(),
                    prompt: format!(
                        "Return every paragraph after its marker, in exactly this format:\n@@1@@\n<paragraph 1>\n\n@@2@@\n<paragraph 2>\n\nPARAGRAPHS:\n{body}"
                    ),
                    max_tokens: ((chars as f32 / 3.0) * 1.4) as u32 + 200,
                    temperature: 0.2,
                    json: false,
                    allow_tools: false,
                };
                match call_forwarding(provider, &creq, events).await {
                    Ok(reply) => {
                        failures = 0;
                        for (k, text) in parse_marked_paragraphs(&reply) {
                            if let Some(&pi) = k.checked_sub(1).and_then(|k| group.get(k)) {
                                reworded[pi] = Some(text);
                            }
                        }
                    }
                    Err(e) => {
                        failures += 1;
                        log::warn!("rewording failed: {e}");
                        if failures >= 2 {
                            enabled = false;
                            asm.article.validation.notes.push(format!("Rewording stopped after errors ({e}); the rest is shown as the original text."));
                        }
                    }
                }
            }
        }
        let mut blocks = Vec::new();
        for (pi, para) in sec.paragraphs.iter().enumerate() {
            let passages = match &reworded[pi] {
                Some(r) => {
                    let ps = align_rewording(para, r, &mut asm.article.validation);
                    reworded_count += ps.iter().filter(|p| p.origin == Origin::Reworded).count();
                    ps
                }
                None => vec![Passage::original(para.trim().to_string())],
            };
            if !passages.is_empty() {
                blocks.push(Block::Paragraph(passages));
            }
        }
        if blocks.is_empty() {
            continue;
        }
        asm.push_section(Section { heading, blocks });
    }
    if budget == 0 && total_paras > 0 {
        asm.article.validation.notes.push("Long text: only part of it was reworded for flow; the rest is shown exactly as the original.".into());
    }
    asm.article.validation.notes.push(format!("{reworded_count} sentence(s) lightly reworded for flow; everything else is the original wording."));
    if asm.article.validation.rewordings_reverted > 0 {
        asm.article.validation.notes.push(format!("{} rewording(s) changed too much and were reverted to the original.", asm.article.validation.rewordings_reverted));
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Discover, search and ask
// ---------------------------------------------------------------------------------------------

/// Input for one AI-only Discover article idea.
#[derive(Clone, Debug)]
pub struct Proposal {
    pub headline: String,
    pub summary: String,
    /// Keys of the papers (from the candidates) the article will be written from.
    pub basis_keys: Vec<String>,
}

fn paper_line(i: usize, p: &Paper, abstract_chars: usize) -> String {
    let mut s = format!("[{i}] {}", p.title.trim());
    if let Some(y) = p.year {
        s.push_str(&format!(" ({y})"));
    }
    if let Some(v) = p.venue.as_ref().filter(|v| !v.is_empty()) {
        s.push_str(&format!(" — {v}"));
    }
    if let Some(c) = p.cited_by_count {
        s.push_str(&format!(" — cited {c} times"));
    }
    if let Some(a) = p.abstract_text.as_ref().filter(|a| !a.trim().is_empty()) {
        s.push_str(&format!("\n    {}", truncate_chars(a.trim(), abstract_chars)));
    }
    s.push('\n');
    s
}

/// Propose `n` AI-only article ideas for a Discover tab, each synthesising several of the
/// candidate papers (headline + 2–3 sentence summary written only from those papers' abstracts).
pub async fn propose_ai_articles(provider: SharedProvider, tab: &str, candidates: &[Paper], n: usize) -> Result<Vec<Proposal>> {
    if n == 0 || candidates.is_empty() {
        return Ok(vec![]);
    }
    let cands: Vec<&Paper> = candidates.iter().take(40).collect();
    let list: String = cands.iter().enumerate().map(|(i, p)| paper_line(i + 1, p, 350)).collect();
    let prompt = format!(
        "Discover tab: {tab}\nRecent papers:\n{list}\nPropose {n} article ideas. Each article must combine 2 to 5 of the papers above that are about the \
         same theme. The headline and summary must use ONLY facts in those papers' abstracts. Summary: 2 or 3 plain-language sentences.\n\
         Reply with a JSON object: {{\"articles\": [{{\"headline\": \"…\", \"summary\": \"…\", \"papers\": [1, 4]}}]}}"
    );
    let v = complete_json(&provider, "You are a science editor choosing stories for a research feed. Reply with JSON only.", &prompt, 400 + 250 * n as u32)
        .await?
        .unwrap_or(Value::Null);
    let items = v.get("articles").or(v.get("ideas")).and_then(|a| a.as_array()).cloned().or_else(|| v.as_array().cloned()).unwrap_or_default();
    let min_basis = if cands.len() >= 2 { 2 } else { 1 };
    let mut out = Vec::new();
    for it in items {
        let Some(headline) = str_field(&it, "headline").or_else(|| str_field(&it, "title")).map(|h| plain(&h)).filter(|h| !h.is_empty()) else { continue };
        let summary = str_field(&it, "summary").map(|s| plain(&s)).unwrap_or_default();
        let mut keys: Vec<String> = Vec::new();
        for i in int_list(&it, "papers").into_iter().chain(int_list(&it, "sources")) {
            if let Some(p) = usize::try_from(i).ok().and_then(|i| i.checked_sub(1)).and_then(|i| cands.get(i)) {
                if !keys.contains(&p.key) {
                    keys.push(p.key.clone());
                }
            }
        }
        if keys.len() < min_basis || summary.is_empty() {
            continue;
        }
        out.push(Proposal { headline, summary, basis_keys: keys });
        if out.len() >= n {
            break;
        }
    }
    Ok(out)
}

/// A neutral headline and a one-paragraph summary (4–6 sentences) for a news event from its outlets' titles and
/// descriptions (only facts the outlets agree on; no slant).
pub async fn news_headline(provider: SharedProvider, articles: &[Paper]) -> Result<(String, String)> {
    let Some(first) = articles.first() else { anyhow::bail!("no news articles given") };
    let list: String = articles
        .iter()
        .take(20)
        .enumerate()
        .map(|(i, a)| {
            let outlet = a.venue.clone().or_else(|| a.publisher.clone()).unwrap_or_else(|| "unknown outlet".into());
            format!("[{}] {outlet}: {}{}\n", i + 1, a.title.trim(), a.abstract_text.as_ref().map(|d| format!(" — {}", truncate_chars(d.trim(), 300))).unwrap_or_default())
        })
        .collect();
    let prompt = format!(
        "These reports from different outlets are about the same news event:\n{list}\nWrite a neutral, factual headline (no opinion or loaded words, \
         no outlet's framing) and a one-paragraph summary of 4 to 6 sentences (about 100 words) covering what happened, who is involved, \
         where and when, and why it matters — using only facts that the reports agree on. \
         Never state a date, number or name that the reports above do not give.\n\
         Reply with a JSON object: {{\"headline\": \"…\", \"summary\": \"…\"}}"
    );
    let v = complete_json(&provider, "You are a neutral news editor. Reply with JSON only.", &prompt, 800).await?.unwrap_or(Value::Null);
    let headline = str_field(&v, "headline").or_else(|| str_field(&v, "title")).map(|h| plain(&h)).filter(|h| !h.is_empty());
    let summary = str_field(&v, "summary").map(|s| plain(&s)).filter(|s| !s.is_empty());
    Ok((
        headline.unwrap_or_else(|| first.title.trim().to_string()),
        summary.unwrap_or_else(|| outlet_paragraph(articles)),
    ))
}

/// Without a model: a paragraph from the outlets' own descriptions (the longest few, deduped).
pub fn outlet_paragraph(articles: &[Paper]) -> String {
    let mut descs: Vec<String> = articles.iter().filter_map(|a| a.abstract_text.clone()).map(|d| d.trim().to_string()).filter(|d| d.len() > 30).collect();
    descs.sort_by_key(|d| std::cmp::Reverse(d.len()));
    let mut out: Vec<String> = vec![];
    for d in descs {
        if out.iter().all(|o| validate::token_similarity(o, &d) < 0.5) {
            out.push(d);
        }
        if out.iter().map(String::len).sum::<usize>() > 500 {
            break;
        }
    }
    out.join(" ")
}

/// AI search mode, step 1: expand the user's query into 2–4 API queries and choose filters
/// (date range, free-full-text, subject, sort) automatically.
#[derive(Clone, Debug, Default)]
pub struct SearchPlan {
    pub queries: Vec<String>,
    pub filters: SearchFilters,
    /// One-line explanation shown to the user ("Looking for 2024–2026 papers on …").
    pub explanation: String,
}

pub async fn plan_search(provider: SharedProvider, query: &str) -> Result<SearchPlan> {
    let year = Utc::now().year();
    let prompt = format!(
        "The current year is {year}. A user searching research papers typed: \"{query}\"\n\
         Plan the search. Reply with a JSON object:\n\
         {{\"queries\": [\"2 to 4 short keyword queries for scholarly search APIs\"], \"from_year\": null or a year, \"to_year\": null or a year, \
         \"free_full_text_only\": false, \"subject\": null or a short subject name, \"sort\": \"relevance\" or \"newest\" or \"most_cited\", \
         \"explanation\": \"one short sentence telling the user what you are looking for\"}}\n\
         Only set years if the query implies a time range (\"recent\" means the last 3 years). Prefer \"newest\" for recent developments."
    );
    let v = complete_json(&provider, "You plan literature searches. Reply with JSON only.", &prompt, 500).await?.unwrap_or(Value::Null);
    Ok(search_plan_from_json(&v, query, year))
}

fn search_plan_from_json(v: &Value, query: &str, year: i32) -> SearchPlan {
    let mut queries: Vec<String> = str_list(v, "queries").into_iter().map(|q| q.trim_matches('"').to_string()).filter(|q| !q.is_empty() && q.len() < 200).collect();
    queries.dedup();
    queries.truncate(4);
    if queries.is_empty() {
        queries.push(query.trim().to_string());
    }
    let valid_year = |k: &str| int_list(v, k).first().copied().map(|y| y as i32).filter(|y| (1800..=year + 1).contains(y));
    let mut from_year = valid_year("from_year");
    let to_year = valid_year("to_year");
    if let (Some(f), Some(t)) = (from_year, to_year) {
        if f > t {
            from_year = None;
        }
    }
    let sort = match str_field(v, "sort").unwrap_or_default().to_lowercase().replace([' ', '-'], "_").as_str() {
        "newest" | "date" | "recent" => SortBy::Newest,
        "most_cited" | "cited" | "citations" => SortBy::MostCited,
        _ => SortBy::Relevance,
    };
    let filters = SearchFilters {
        from_year,
        to_year,
        free_full_text_only: v.get("free_full_text_only").and_then(|b| b.as_bool()).unwrap_or(false),
        kinds: vec![],
        subject: str_field(v, "subject").filter(|s| !s.eq_ignore_ascii_case("null") && !s.eq_ignore_ascii_case("none")),
        sort,
    };
    let explanation = str_field(v, "explanation").map(|e| plain(&e)).unwrap_or_else(|| {
        let range = match (filters.from_year, filters.to_year) {
            (Some(f), Some(t)) => format!("{f}–{t} "),
            (Some(f), None) => format!("{f}–{year} "),
            _ => String::new(),
        };
        format!("Looking for {range}papers on “{}”.", query.trim())
    });
    SearchPlan { queries, filters, explanation }
}

/// AI search mode, step 2: from the found papers, pick the newest and most relevant (keys, best
/// first) and write a short cited overview (sources = the picked papers).
#[derive(Clone, Debug, Default)]
pub struct Curated {
    pub top_keys: Vec<String>,
    pub overview: Vec<Passage>,
    pub sources: Vec<Source>,
}

pub async fn curate_results(provider: SharedProvider, query: &str, papers: &[Paper]) -> Result<Curated> {
    if papers.is_empty() {
        return Ok(Curated::default());
    }
    let cands: Vec<&Paper> = papers.iter().take(30).collect();
    let list: String = cands.iter().enumerate().map(|(i, p)| paper_line(i + 1, p, 400)).collect();
    let year = Utc::now().year();
    let prompt = format!(
        "The current year is {year}. Search: \"{query}\"\nPapers found:\n{list}\n\
         1. Pick the 3 to 6 papers that best answer the search, preferring newer and more relevant ones, best first.\n\
         2. Write an overview of 3 to 5 sentences of what these papers say about the search, using ONLY their titles and abstracts above. \
         Cite paper numbers in square brackets after every fact, e.g. \"A new catalyst doubled the yield [3].\"\n\
         Reply with a JSON object: {{\"top\": [3, 1, 7], \"overview\": \"…\"}}"
    );
    let v = complete_json(&provider, "You are a research librarian. Reply with JSON only.", &prompt, 900).await?.unwrap_or(Value::Null);
    Ok(curated_from_json(&v, &cands))
}

fn curated_from_json(v: &Value, cands: &[&Paper]) -> Curated {
    let n = cands.len() as i64;
    let mut top: Vec<usize> = Vec::new();
    for i in int_list(v, "top").into_iter().chain(int_list(v, "papers")) {
        if (1..=n).contains(&i) && !top.contains(&(i as usize)) {
            top.push(i as usize);
        }
    }
    let overview_text = str_field(v, "overview").unwrap_or_default();
    let all_valid: HashSet<u32> = (1..=n as u32).collect();
    let mut report = ValidationReport::default();
    let mut overview = ai_passages(&overview_text, &all_valid, &mut report);
    // Renumber: picked papers first (in order), then any other cited ones, as 1..k.
    let mut order: Vec<usize> = top.clone();
    for p in &overview {
        for &c in &p.citations {
            if !order.contains(&(c as usize)) {
                order.push(c as usize);
            }
        }
    }
    let new_id = |old: u32| order.iter().position(|&o| o == old as usize).map(|i| i as u32 + 1);
    for p in &mut overview {
        p.citations = p.citations.iter().filter_map(|&c| new_id(c)).collect();
    }
    let sources: Vec<Source> = order.iter().enumerate().map(|(i, &o)| Source::from_paper(i as u32 + 1, SourceKind::Paper, cands[o - 1])).collect();
    Curated { top_keys: top.iter().map(|&i| cands[i - 1].key.clone()).collect(), overview, sources }
}

/// Answer an Ask-panel question about an article, from the article's sources plus newly
/// gathered ones (`pack`). Cites inline; validated like articles.
pub async fn answer_question(provider: SharedProvider, article: &Article, question: &str, pack: SourcePack, deltas: Option<UnboundedSender<String>>) -> Result<Answer> {
    let mut sources = article.sources.clone();
    for s in pack.sources {
        if !sources.iter().any(|x| x.id == s.id) {
            sources.push(s);
        }
    }
    sources.sort_by_key(|s| s.id);
    let valid = valid_ids(&sources);
    // The article text with its citation markers, so the model sees which source backs what.
    let mut text = String::new();
    for sec in &article.sections {
        text.push_str(&format!("## {}\n", sec.heading));
        for b in &sec.blocks {
            if let Block::Paragraph(ps) = b {
                for p in ps {
                    text.push_str(p.text.trim());
                    if !p.citations.is_empty() {
                        text.push_str(&format!(" [{}]", p.citations.iter().map(|c| c.to_string()).collect::<Vec<_>>().join(", ")));
                    }
                    text.push(' ');
                }
                text.push_str("\n\n");
            }
        }
    }
    let prompt = format!(
        "ARTICLE: \"{}\" ({})\n{}\n\nSOURCES:\n{}\nEXTRA EXCERPTS (each starts with its source number):\n{}\n\
         QUESTION: {question}\n\nAnswer in 1 to 3 short paragraphs (at most about 200 words), using ONLY the article and the sources above. \
         Cite source numbers in square brackets after every fact, e.g. \"The effect was strongest in children [4].\" \
         If the sources do not answer the question, say so plainly.",
        article.title,
        article.kind.label(),
        truncate_chars(&text, 9000),
        source_list(&sources, article.kind == ArticleKind::FullText),
        excerpt_list(&pack.chunks, 12_000, 800),
    );
    let req = CompletionRequest {
        system: "You answer questions about a research or news article, Perplexity-style: concise, direct, and cited. Never invent facts or sources.".into(),
        prompt,
        max_tokens: 700,
        temperature: 0.2,
        json: false,
        allow_tools: false,
    };
    let reply = provider.complete(&req, deltas).await?;
    let mut report = ValidationReport::default();
    let mut passages = Vec::new();
    let paragraphs: Vec<&str> = reply.split("\n\n").map(str::trim).filter(|p| !p.is_empty() && !is_reference_heading(p)).collect();
    let np = paragraphs.len();
    for (i, para) in paragraphs.into_iter().enumerate() {
        let joined = para.lines().map(|l| l.trim().trim_start_matches(['-', '*', '•']).trim()).collect::<Vec<_>>().join(" ");
        let mut ps = ai_passages(&joined, &valid, &mut report);
        // Paragraph break: the last passage of each paragraph but the final one ends in "\n\n".
        if i + 1 < np {
            if let Some(last) = ps.last_mut() {
                last.text = format!("{}\n\n", last.text.trim_end());
            }
        }
        passages.extend(ps);
    }
    Ok(Answer { passages, sources, generated_by: provider.describe() })
}

/// Expand a query into extra search queries (synonyms, key terms) for the research layer.
/// Falls back to no extra queries on any error.
pub async fn expand_query(provider: SharedProvider, query: &str) -> Vec<String> {
    let prompt = format!(
        "Search query: \"{query}\"\nGive 2 to 4 alternative short keyword queries (synonyms, technical terms, related phrasings) that would find \
         relevant research papers. Reply with a JSON object: {{\"queries\": [\"…\", \"…\"]}}"
    );
    match complete_json(&provider, "You help search scholarly databases. Reply with JSON only.", &prompt, 300).await {
        Ok(Some(v)) => {
            let orig = query.trim().to_lowercase();
            let mut out: Vec<String> = Vec::new();
            let list = if v.is_array() { str_list(&serde_json::json!({ "q": v }), "q") } else { str_list(&v, "queries") };
            for q in list {
                let q = q.trim().trim_matches('"').to_string();
                if !q.is_empty() && q.len() < 200 && q.to_lowercase() != orig && !out.iter().any(|o| o.eq_ignore_ascii_case(&q)) {
                    out.push(q);
                }
            }
            out.truncate(4);
            out
        }
        _ => vec![],
    }
}

#[cfg(test)]
mod tests;
