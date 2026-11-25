//! Learn AI: a plain-language rewrite of an abstract or a whole article for readers without
//! the background, so they can learn the subject from the same sources.
//!
//! The rewrite is AI-written text, so it is marked as such and validated like any article:
//! citations must map to the real sources, uncited factual claims are marked "unverified".
//! A closing "Key terms" glossary explains the jargon in general terms.

use std::collections::HashSet;

use anyhow::Result;
use chrono::Utc;
use serde_json::Value;
use tokio::sync::mpsc::UnboundedSender;

use super::json::{extract_json, str_field};
use super::parse::{ValidationContext, body_to_blocks};
use super::prompts::{source_list, truncate_chars};
use super::validate::strip_markdown;
use super::WriteEvent;
use crate::model::*;
use crate::providers::{CompletionRequest, SharedProvider};

/// What to explain.
#[derive(Clone, Debug)]
pub struct LearnInput {
    /// What the text is about (the article or paper title).
    pub title: String,
    /// (heading, text with inline [n] citations). One entry for an abstract.
    pub sections: Vec<(String, String)>,
    /// The real sources the [n] numbers refer to.
    pub sources: Vec<Source>,
    /// Link back to the thing being explained (paper key or doc key).
    pub paper_key: Option<String>,
    pub original_title: Option<String>,
    pub original_authors: Vec<String>,
    pub original_url: Option<String>,
}

impl LearnInput {
    /// Explain an abstract (or news summary): the paper itself is source [1].
    pub fn from_abstract(doc: &Doc) -> Self {
        let text = doc.abstract_text();
        let (sources, cited) = match doc.paper() {
            Some(p) => (vec![Source::from_paper(1, SourceKind::Target, p)], format!("{text} [1]")),
            None => match doc {
                Doc::Feed(f) => {
                    let srcs: Vec<Source> = f.basis.iter().take(12).enumerate().map(|(i, p)| Source::from_paper(i as u32 + 1, if f.kind == ContentKind::News { SourceKind::News } else { SourceKind::Paper }, p)).collect();
                    let marks: String = (1..=srcs.len().min(3)).map(|i| format!("[{i}]")).collect();
                    (srcs, format!("{text} {marks}"))
                }
                _ => (vec![], text.clone()),
            },
        };
        LearnInput {
            title: doc.title().to_string(),
            sections: vec![("Summary".into(), cited)],
            sources,
            paper_key: Some(doc.key()),
            original_title: doc.paper().map(|p| p.title.clone()),
            original_authors: doc.paper().map(|p| p.authors.clone()).unwrap_or_default(),
            original_url: doc.paper().and_then(|p| p.url.clone()),
        }
    }

    /// Explain a whole written article, section by section, keeping its sources.
    pub fn from_article(a: &Article) -> Self {
        let sections = a
            .sections
            .iter()
            .map(|s| {
                let text = s
                    .blocks
                    .iter()
                    .filter_map(|b| match b {
                        Block::Paragraph(ps) => Some(
                            ps.iter()
                                .map(|p| {
                                    let marks: String = p.citations.iter().map(|c| format!("[{c}]")).collect();
                                    format!("{}{}", p.text.trim(), if marks.is_empty() { String::new() } else { format!(" {marks}") })
                                })
                                .collect::<Vec<_>>()
                                .join(" "),
                        ),
                        Block::Heading(h) => Some(format!("{h}:")),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n\n");
                (s.heading.clone(), text)
            })
            .filter(|(_, t)| !t.trim().is_empty())
            .collect();
        LearnInput {
            title: a.title.clone(),
            sections,
            sources: a.sources.clone(),
            paper_key: a.paper_key.clone(),
            original_title: a.original_title.clone(),
            original_authors: a.original_authors.clone(),
            original_url: a.original_url.clone(),
        }
    }
}

const SYSTEM: &str = "You explain research to curious, intelligent adults with no background in the subject, like a great science teacher \
(clear and friendly, never childish). Use short sentences and everyday words. The first time a technical term appears, explain it in plain words in the same sentence \
(for example: \"PCSK9, a protein that controls how much cholesterol stays in the blood\"). Use a simple comparison when it helps. \
Never add facts that are not in the text you are given. Every sentence that states a fact from the text must end with the same \
citation number(s) as the sentence it came from, like [1] or [3] — use only numbers that appear in the text, and never drop them. Do not write a reference list. Write plain paragraphs: no headings, no bullet points, no markdown.";

/// Rewrite the input for a non-specialist. Streams one `WriteEvent::Section` per section.
pub async fn explain(provider: SharedProvider, input: LearnInput, events: UnboundedSender<WriteEvent>) -> Result<Article> {
    let valid: HashSet<u32> = input.sources.iter().map(|s| s.id).collect();
    let ctx = ValidationContext { valid: valid.clone(), chunks: vec![] };
    let mut report = ValidationReport::default();
    let mut sections = vec![];
    let total = input.sections.len().max(1) as f32 + 1.0;
    let sources_block = source_list(&input.sources, false);
    let mut all_text = String::new();

    for (i, (heading, text)) in input.sections.iter().enumerate() {
        let _ = events.send(WriteEvent::Stage { text: format!("Explaining in plain words: {heading} ({}/{})…", i + 1, input.sections.len()), fraction: i as f32 / total });
        let prompt = format!(
            "Topic: {}\n\nSources the citation numbers refer to:\n{}\n\nRewrite this {} for someone with no background in the subject. \
             Keep all the information, explain every technical term in plain words, keep the citations.\n\n---\n{}\n---",
            input.title,
            sources_block,
            if input.sections.len() == 1 { "text" } else { "section of an article" },
            truncate_chars(text, 9000)
        );
        let req = CompletionRequest { system: SYSTEM.into(), prompt, max_tokens: 2200, temperature: 0.3, json: false, allow_tools: false };
        let (dtx, mut drx) = tokio::sync::mpsc::unbounded_channel::<String>();
        let ev = events.clone();
        let fwd = tokio::spawn(async move {
            while let Some(d) = drx.recv().await {
                let _ = ev.send(WriteEvent::Delta(d));
            }
        });
        let reply = provider.complete(&req, Some(dtx)).await?;
        let _ = fwd.await;
        let body = strip_markdown(&paragraphs_from_lists(&strip_think(&reply)));
        all_text.push_str(&body);
        all_text.push('\n');
        let blocks: Vec<Block> = body_to_blocks(&body, &ctx, &mut report).into_iter().filter(|b| matches!(b, Block::Paragraph(_))).collect();
        if blocks.is_empty() {
            continue;
        }
        let section = Section { heading: if input.sections.len() == 1 { "In plain words".into() } else { heading.clone() }, blocks };
        let _ = events.send(WriteEvent::Section(section.clone()));
        sections.push(section);
    }
    if sections.is_empty() {
        anyhow::bail!("the model returned no usable explanation");
    }

    // Glossary of the jargon, in general terms (clearly labelled as such).
    let _ = events.send(WriteEvent::Stage { text: "Explaining key terms…".into(), fraction: (total - 1.0) / total });
    if let Ok(glossary) = key_terms(&provider, &input).await
        && !glossary.is_empty()
    {
        let mut blocks = vec![Block::Paragraph(vec![Passage {
            text: "General definitions written by the AI to help you read the sources — they are not taken from the sources themselves. ".into(),
            origin: Origin::AiWritten,
            citations: vec![],
            original: None,
            unverified: false,
        }])];
        for (term, meaning) in glossary {
            blocks.push(Block::Paragraph(vec![Passage { text: format!("{term}: {meaning} "), origin: Origin::AiWritten, citations: vec![], original: None, unverified: false }]));
        }
        let section = Section { heading: "Key terms".into(), blocks };
        let _ = events.send(WriteEvent::Section(section.clone()));
        sections.push(section);
    }
    let _ = events.send(WriteEvent::Stage { text: "Done".into(), fraction: 1.0 });

    Ok(Article {
        id: uuid::Uuid::new_v4().to_string(),
        kind: ArticleKind::Learn,
        paper_key: input.paper_key,
        title: format!("Learn: {}", input.title.trim_start_matches("AI interpretation: ")),
        standfirst: None,
        original_title: input.original_title,
        original_authors: input.original_authors,
        original_url: input.original_url,
        byline: format!("Gateless Scholar + {}", provider.brand()),
        generated_by: provider.describe(),
        created: Utc::now(),
        sections,
        sources: input.sources,
        hero_image: None,
        validation: report,
    })
}

async fn key_terms(provider: &SharedProvider, input: &LearnInput) -> Result<Vec<(String, String)>> {
    let text: String = input.sections.iter().map(|(_, t)| t.as_str()).collect::<Vec<_>>().join("\n");
    let prompt = format!(
        "List the 4 to 10 technical terms in this text that a newcomer would not know, each with a one-sentence plain-language explanation.\n\
         Reply with a JSON object: {{\"terms\": [{{\"term\": \"…\", \"meaning\": \"…\"}}]}}\n\n---\n{}\n---",
        truncate_chars(&text, 8000)
    );
    let req = CompletionRequest { system: "You are a patient science teacher. Reply with JSON only.".into(), prompt, max_tokens: 900, temperature: 0.2, json: true, allow_tools: false };
    let reply = provider.complete(&req, None).await?;
    let v = extract_json(&strip_think(&reply)).unwrap_or(Value::Null);
    let list = v.get("terms").and_then(Value::as_array).cloned().unwrap_or_default();
    Ok(list
        .iter()
        .filter_map(|t| Some((str_field(t, "term")?, str_field(t, "meaning").or_else(|| str_field(t, "explanation"))?)))
        .map(|(t, m)| (t.trim().trim_matches('*').to_string(), m.trim().to_string()))
        .filter(|(t, m)| !t.is_empty() && !m.is_empty() && t.len() < 80)
        .take(12)
        .collect())
}

/// Models sometimes answer in bullet lists despite being asked for paragraphs: make each item
/// its own paragraph and drop list markers and "Label:" lead-ins on their own line.
fn paragraphs_from_lists(s: &str) -> String {
    s.lines()
        .map(|l| {
            let t = l.trim_start();
            let t = t.strip_prefix("- ").or_else(|| t.strip_prefix("* ")).or_else(|| t.strip_prefix("• ")).map(|r| r.to_string());
            match t {
                Some(item) => format!("\n{item}\n"),
                None => l.to_string(),
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn strip_think(s: &str) -> String {
    let mut out = s.to_string();
    while let (Some(a), Some(b)) = (out.find("<think>"), out.find("</think>")) {
        if b < a {
            break;
        }
        out.replace_range(a..b + "</think>".len(), "");
    }
    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::future::BoxFuture;

    struct Fake;
    impl crate::providers::ModelProvider for Fake {
        fn brand(&self) -> String {
            "Fake".into()
        }
        fn describe(&self) -> String {
            "Fake model".into()
        }
        fn is_small(&self) -> bool {
            true
        }
        fn complete<'a>(&'a self, req: &'a CompletionRequest, _d: Option<UnboundedSender<String>>) -> BoxFuture<'a, anyhow::Result<String>> {
            let json = req.json;
            Box::pin(async move {
                Ok(if json {
                    r#"{"terms":[{"term":"PCSK9","meaning":"A protein that controls how much cholesterol stays in the blood."}]}"#.into()
                } else {
                    "Scientists changed one letter of DNA in monkeys' livers, which lowered their cholesterol for over a year [1]. This is like fixing a typo in a recipe [9].".into()
                })
            })
        }
    }

    #[test]
    fn explains_abstract_with_valid_citations_and_glossary() {
        let p = Paper { key: "doi:10.1/x".into(), title: "Base editing of PCSK9".into(), abstract_text: Some("We edited PCSK9.".into()), ..Default::default() };
        let input = LearnInput::from_abstract(&Doc::Paper(p));
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let a = rt.block_on(explain(std::sync::Arc::new(Fake), input, tx)).unwrap();
        assert_eq!(a.kind, ArticleKind::Learn);
        assert!(a.passages().all(|p| p.origin == Origin::AiWritten));
        // [9] doesn't exist: removed.
        assert!(a.passages().all(|p| p.citations.iter().all(|c| *c == 1)));
        assert_eq!(a.validation.invalid_citations_removed, 1);
        assert_eq!(a.sections.last().unwrap().heading, "Key terms");
    }
}
