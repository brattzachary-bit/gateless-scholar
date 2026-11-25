//! Turning model-written markdown into validated article blocks.
//!
//! The model writes sections as `## Heading` followed by prose paragraphs, optional `###`
//! sub-headings and optional ```` ```chart ```` blocks. Model-written reference lists are
//! always discarded: the reference list comes from the research layer's sources only.

use std::collections::HashSet;

use crate::model::{Block, Chunk, ValidationReport};

use super::charts::{parse_chart, validate_chart};
use super::validate::{ai_passages, strip_markdown};

/// What a section body is validated against.
pub struct ValidationContext {
    pub valid: HashSet<u32>,
    pub chunks: Vec<Chunk>,
}

pub fn is_reference_heading(h: &str) -> bool {
    let h = h.trim().trim_matches(['*', '#', ':', ' ']).to_lowercase();
    matches!(h.as_str(), "references" | "reference list" | "sources" | "bibliography" | "works cited" | "citations" | "sources cited" | "further reading" | "notes and references")
}

/// A line that looks like an entry of a model-written reference list ("[3] Smith, J. (2020) …").
fn looks_like_reference_entry(line: &str) -> bool {
    let t = line.trim().trim_start_matches(['-', '*', ' ']);
    let starts_numbered = t.starts_with('[') && t.find(']').is_some_and(|e| e < 8 && t[1..e].trim().chars().all(|c| c.is_ascii_digit()) && e > 1)
        || t.split_once(['.', ')']).is_some_and(|(n, _)| !n.is_empty() && n.len() <= 3 && n.chars().all(|c| c.is_ascii_digit()));
    if !starts_numbered {
        return false;
    }
    let has_year = regex::Regex::new(r"\(?(19|20)\d{2}\)?").map(|re| re.is_match(t)).unwrap_or(false);
    has_year && (t.contains("et al") || t.contains("http") || t.contains("doi") || t.contains(" (") || t.contains("Journal") || t.matches(',').count() >= 2)
}

/// Heading level of a markdown heading line (1–6), and its text.
fn heading(line: &str) -> Option<(usize, String)> {
    let t = line.trim_start();
    let level = t.chars().take_while(|&c| c == '#').count();
    if level == 0 || level > 6 {
        return None;
    }
    let rest = &t[level..];
    if !rest.starts_with(' ') {
        return None;
    }
    let text = rest.trim().trim_matches('#').trim().trim_matches('*').trim().to_string();
    if text.is_empty() { None } else { Some((level, text)) }
}

/// Split markdown into (`## ` sections). Text before the first section heading and `# ` title
/// lines are ignored. Headings inside fenced blocks are not headings.
pub fn split_sections(text: &str) -> (String, Vec<(String, String)>) {
    let mut preamble = String::new();
    let mut sections: Vec<(String, String)> = Vec::new();
    let mut in_fence = false;
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
        }
        if !in_fence {
            if let Some((level, h)) = heading(line) {
                if level == 2 {
                    sections.push((h, String::new()));
                    continue;
                }
                if level == 1 {
                    continue;
                }
            }
        }
        match sections.last_mut() {
            Some((_, body)) => {
                body.push_str(line);
                body.push('\n');
            }
            None => {
                preamble.push_str(line);
                preamble.push('\n');
            }
        }
    }
    (preamble, sections)
}

/// Parse a section body into validated blocks (paragraphs of passages, sub-headings, charts).
pub fn body_to_blocks(body: &str, ctx: &ValidationContext, report: &mut ValidationReport) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut para = String::new();
    let mut fence: Option<(String, String)> = None; // (info, content)

    let flush = |para: &mut String, blocks: &mut Vec<Block>, report: &mut ValidationReport| {
        let text = para.trim().to_string();
        para.clear();
        if text.is_empty() {
            return;
        }
        let passages = ai_passages(&text, &ctx.valid, report);
        if !passages.is_empty() {
            blocks.push(Block::Paragraph(passages));
        }
    };

    let mut skipping_refs = false;
    for line in body.lines() {
        let t = line.trim();
        if let Some((info, content)) = fence.as_mut() {
            if t.starts_with("```") {
                let info = info.clone();
                let content = std::mem::take(content);
                fence = None;
                if info.starts_with("chart") || content.contains("\"kind\"") {
                    match parse_chart(&content) {
                        Some(mut spec) => match validate_chart(&mut spec, &ctx.valid, &ctx.chunks) {
                            Ok(()) => blocks.push(Block::Chart(spec)),
                            Err(why) => {
                                report.charts_rejected += 1;
                                report.notes.push(format!("Chart dropped: {why}."));
                            }
                        },
                        None => {
                            report.charts_rejected += 1;
                            report.notes.push("Chart dropped: its specification could not be read.".into());
                        }
                    }
                }
                // Other fenced blocks (code, tables) are not part of an article: dropped.
            } else {
                content.push_str(line);
                content.push('\n');
            }
            continue;
        }
        if t.starts_with("```") {
            flush(&mut para, &mut blocks, report);
            fence = Some((t.trim_start_matches('`').trim().to_lowercase(), String::new()));
            continue;
        }
        if t.is_empty() {
            flush(&mut para, &mut blocks, report);
            continue;
        }
        if let Some((_, h)) = heading(t) {
            flush(&mut para, &mut blocks, report);
            skipping_refs = is_reference_heading(&h);
            if !skipping_refs {
                blocks.push(Block::Heading(strip_markdown(&h)));
            }
            continue;
        }
        if skipping_refs {
            continue;
        }
        // A bold-only line acts as a sub-heading.
        if t.starts_with("**") && t.ends_with("**") && t.len() > 4 && t.len() < 90 && !t[2..t.len() - 2].contains("**") {
            flush(&mut para, &mut blocks, report);
            let h = t.trim_matches('*').trim().trim_end_matches(':').to_string();
            if is_reference_heading(&h) {
                skipping_refs = true;
            } else {
                blocks.push(Block::Heading(h));
            }
            continue;
        }
        if is_reference_heading(t) {
            flush(&mut para, &mut blocks, report);
            skipping_refs = true;
            continue;
        }
        if looks_like_reference_entry(t) {
            continue;
        }
        if matches!(t, "---" | "***" | "___") {
            flush(&mut para, &mut blocks, report);
            continue;
        }
        // Bullets become their own paragraphs (articles are prose).
        let bullet = t.strip_prefix("- ").or_else(|| t.strip_prefix("* ")).or_else(|| t.strip_prefix("• ")).or_else(|| {
            t.split_once(". ").filter(|(n, _)| !n.is_empty() && n.len() <= 2 && n.chars().all(|c| c.is_ascii_digit())).map(|(_, r)| r)
        });
        if let Some(b) = bullet {
            flush(&mut para, &mut blocks, report);
            para.push_str(b);
            flush(&mut para, &mut blocks, report);
            continue;
        }
        if !para.is_empty() {
            para.push(' ');
        }
        para.push_str(t);
    }
    flush(&mut para, &mut blocks, report);
    blocks
}

/// Remove a heading the model repeated at the top of a section it was asked to write.
pub fn strip_leading_heading(text: &str, heading_text: &str) -> String {
    let mut lines: Vec<&str> = text.trim().lines().collect();
    while let Some(first) = lines.first() {
        let t = first.trim();
        let plain = t.trim_start_matches('#').trim().trim_matches('*').trim().trim_end_matches(':');
        if t.is_empty() || t.starts_with('#') || plain.eq_ignore_ascii_case(heading_text.trim()) {
            lines.remove(0);
        } else {
            break;
        }
    }
    lines.join("\n")
}

/// Incrementally finds finished `## ` sections in streamed text. A section is finished when
/// the next section heading has started; the last one is finished by [`SectionStreamer::finish`].
#[derive(Default)]
pub struct SectionStreamer {
    buf: String,
    emitted: usize,
}

impl SectionStreamer {
    /// Add streamed text; returns sections that just became complete.
    pub fn push(&mut self, delta: &str) -> Vec<(String, String)> {
        self.buf.push_str(delta);
        if !delta.contains('\n') {
            return vec![];
        }
        // Only consider complete lines.
        let complete = match self.buf.rfind('\n') {
            Some(p) => &self.buf[..=p],
            None => return vec![],
        };
        let (_, sections) = split_sections(complete);
        if sections.len() <= self.emitted + 1 {
            return vec![];
        }
        let done: Vec<(String, String)> = sections[self.emitted..sections.len() - 1].to_vec();
        self.emitted = sections.len() - 1;
        done
    }

    /// The reply is complete: return the sections not yet emitted. `full` is the provider's
    /// final text, used when nothing (or less) was streamed. With no `## ` headings at all,
    /// the whole text becomes one section titled `fallback_heading`.
    pub fn finish(&mut self, full: &str, fallback_heading: &str) -> Vec<(String, String)> {
        let text = if self.buf.trim().is_empty() || (full.len() > self.buf.len() && full.starts_with(self.buf.as_str())) {
            full.to_string()
        } else {
            self.buf.clone()
        };
        let (preamble, sections) = split_sections(&text);
        if sections.is_empty() {
            if self.emitted == 0 && !preamble.trim().is_empty() {
                self.emitted = 1;
                return vec![(fallback_heading.to_string(), preamble)];
            }
            return vec![];
        }
        let rest = sections.get(self.emitted..).map(|s| s.to_vec()).unwrap_or_default();
        self.emitted = sections.len();
        rest
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Origin;

    fn ctx() -> ValidationContext {
        ValidationContext {
            valid: [1, 2].into(),
            chunks: vec![Chunk { source_id: 1, text: "In 2023 the yield was 42 tonnes and in 2024 it was 57 tonnes.".into(), section: None, score: 1.0 }],
        }
    }

    #[test]
    fn splits_sections_and_ignores_title() {
        let (pre, s) = split_sections("# Title\nIntro text\n## One\nA.\n### Sub\nB.\n## Two\nC.\n");
        assert!(pre.contains("Intro"));
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].0, "One");
        assert!(s[0].1.contains("### Sub"));
    }

    #[test]
    fn body_blocks_with_chart_and_reference_list_dropped() {
        let body = "First paragraph with a fact [1].\nStill first.\n\n### Detail\n\nThe yield grew [1].\n\n```chart\n{\"kind\":\"bar\",\"title\":\"Yield\",\"series\":[{\"name\":\"t\",\"points\":[[\"2023\",42],[\"2024\",57]]}],\"citations\":[1]}\n```\n\n```chart\n{\"kind\":\"bar\",\"title\":\"Bad\",\"series\":[{\"name\":\"t\",\"points\":[[\"2023\",99]]}],\"citations\":[1]}\n```\n\n**References**\n[1] Smith, J. et al. (2020) A paper. Journal of Things.\n[7] Invented, A. (2021) Not real, Fake Journal.\n";
        let mut r = ValidationReport::default();
        let blocks = body_to_blocks(body, &ctx(), &mut r);
        assert_eq!(blocks.len(), 4, "{blocks:#?}");
        assert!(matches!(&blocks[0], Block::Paragraph(p) if p.len() == 2 && p[0].citations == vec![1]));
        assert!(matches!(&blocks[1], Block::Heading(h) if h == "Detail"));
        assert!(matches!(&blocks[3], Block::Chart(_)));
        assert_eq!(r.charts_rejected, 1);
        for b in &blocks {
            if let Block::Paragraph(ps) = b {
                assert!(ps.iter().all(|p| p.origin == Origin::AiWritten && !p.text.contains("Invented")));
            }
        }
    }

    #[test]
    fn streamer_emits_sections_as_they_finish() {
        let mut s = SectionStreamer::default();
        assert!(s.push("Sure, here it is.\n## Over").is_empty());
        assert!(s.push("view\nText one [1].\n").is_empty());
        let done = s.push("\n## Background\nText two.");
        assert_eq!(done.len(), 1);
        assert_eq!(done[0].0, "Overview");
        let rest = s.finish("ignored because deltas were streamed", "X");
        assert_eq!(rest.len(), 1);
        assert_eq!(rest[0].0, "Background");
        assert!(rest[0].1.contains("Text two."));
    }

    #[test]
    fn streamer_without_deltas_uses_full_text() {
        let mut s = SectionStreamer::default();
        let rest = s.finish("## A\nx\n## B\ny\n", "X");
        assert_eq!(rest.len(), 2);
        let mut s2 = SectionStreamer::default();
        let rest = s2.finish("Just prose without headings.", "Overview");
        assert_eq!(rest, vec![("Overview".to_string(), "Just prose without headings.\n".to_string())]);
    }

    #[test]
    fn strips_repeated_heading() {
        assert_eq!(strip_leading_heading("## Background\n\nText.", "Background"), "Text.");
        assert_eq!(strip_leading_heading("**Background:**\nText.", "Background"), "Text.");
        assert_eq!(strip_leading_heading("Text.", "Background"), "Text.");
    }
}
