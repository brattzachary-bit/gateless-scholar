//! Prompt building. Prompts are explicit and short so small local models cope: numbered
//! sources, excerpts tagged with their source number, an example of the citation format, and
//! no tool use required.

use std::collections::HashSet;

use crate::model::{ArticleKind, Chunk, Paper, Source, SourceKind};

use super::validate::tokens;

pub const CITATION_EXAMPLE: &str = "Example: \"The probe found water ice near the south pole [2]. Later surveys confirmed it [3, 5].\"";

pub fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let t: String = s.chars().take(max).collect();
    format!("{}…", t.trim_end())
}

fn role(kind: SourceKind) -> &'static str {
    match kind {
        SourceKind::Target => " — TARGET PAPER (only its abstract is available)",
        SourceKind::Citing => " — a paper that cites the target",
        SourceKind::News => " — news report",
        SourceKind::Web => " — web source",
        SourceKind::Paper => "",
    }
}

fn authors_short(a: &[String]) -> String {
    match a.len() {
        0 => String::new(),
        1 => a[0].clone(),
        2 => format!("{} and {}", a[0], a[1]),
        _ => format!("{} et al.", a[0]),
    }
}

/// "[3] Title. Smith et al. (2021). Nature — role"
pub fn source_list(sources: &[Source], full_text_target: bool) -> String {
    let mut out = String::new();
    for s in sources {
        let mut line = format!("[{}] {}", s.id, s.title.trim());
        let au = authors_short(&s.authors);
        if !au.is_empty() {
            line.push_str(&format!(". {au}"));
        }
        if let Some(y) = s.year {
            line.push_str(&format!(" ({y})"));
        } else if let Some(d) = s.date {
            line.push_str(&format!(" ({d})"));
        }
        if let Some(v) = s.venue.as_ref().filter(|v| !v.is_empty()) {
            line.push_str(&format!(". {v}"));
        }
        if s.kind == SourceKind::Target && full_text_target {
            line.push_str(" — TARGET PAPER");
        } else {
            line.push_str(role(s.kind));
        }
        out.push_str(&line);
        out.push('\n');
    }
    out
}

/// "[3] (Methods) excerpt…" lines, best first, within a character budget.
pub fn excerpt_list(chunks: &[Chunk], budget_chars: usize, per_chunk: usize) -> String {
    let mut out = String::new();
    for c in chunks {
        let mut line = format!("[{}]", c.source_id);
        if let Some(sec) = c.section.as_ref().filter(|s| !s.is_empty()) {
            line.push_str(&format!(" ({sec})"));
        }
        line.push(' ');
        line.push_str(&truncate_chars(&c.text.split_whitespace().collect::<Vec<_>>().join(" "), per_chunk));
        if out.len() + line.len() > budget_chars && !out.is_empty() {
            break;
        }
        out.push_str(&line);
        out.push_str("\n\n");
    }
    out
}

const STOP: &[&str] = &[
    "the", "a", "an", "and", "or", "of", "in", "on", "to", "for", "with", "is", "are", "what", "how", "it", "its", "this", "that", "by", "as",
    "at", "from", "be", "was", "were", "about", "other", "describe", "context", "paper", "reports", "open", "questions",
];

/// Re-rank chunks for one section by term overlap with the section heading/focus (and topic),
/// keeping the research layer's order as a tie-breaker. Returns the top `k`.
pub fn rerank_for_section(chunks: &[Chunk], query: &str, k: usize) -> Vec<Chunk> {
    let q: HashSet<String> = tokens(query).into_iter().filter(|t| t.len() > 2 && !STOP.contains(&t.as_str())).collect();
    let n = chunks.len().max(1) as f32;
    let mut scored: Vec<(f32, usize)> = chunks
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let ct: HashSet<String> = tokens(&c.text).into_iter().collect();
            let overlap = q.iter().filter(|t| ct.contains(*t)).count() as f32;
            let sec_bonus = c.section.as_ref().map(|s| tokens(s).iter().filter(|t| q.contains(*t)).count() as f32).unwrap_or(0.0);
            (overlap + 2.0 * sec_bonus + 1.5 * (1.0 - i as f32 / n), i)
        })
        .collect();
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal).then(a.1.cmp(&b.1)));
    scored.into_iter().take(k).map(|(_, i)| chunks[i].clone()).collect()
}

pub fn kind_instructions(kind: ArticleKind) -> &'static str {
    match kind {
        ArticleKind::Interpretation => {
            "You are writing Gateless Scholar's own interpretation of the topic of the TARGET PAPER. Only its abstract is freely \
             available, so you have NOT read the full paper: never claim to know details of its methods or results beyond what \
             the abstract and the other sources say, and say so where it matters. This is not the paper itself: refer to its \
             authors in the third person (\"the authors report…\") and never write as if you were them. Use the related papers \
             for background and context, and use the citing papers to explain how other researchers describe the work."
        }
        ArticleKind::AiOnly => {
            "You are writing an explanatory article that synthesises several papers on one topic. Compare and connect the \
             sources, say where they agree or differ, and explain why the findings matter."
        }
        ArticleKind::News => {
            "You are writing ONE neutral news story about a single event, combined from several outlets' reports. Report only \
             facts the outlets give and attribute each fact to the outlets that report it. Where outlets disagree, say so and \
             attribute each version. Do not adopt any outlet's framing, loaded words or opinions; keep a calm, neutral tone \
             whatever the slant of individual sources. Cite every outlet at least once."
        }
        ArticleKind::FullText | ArticleKind::Learn => "",
    }
}

pub fn system_prompt(kind: ArticleKind) -> String {
    format!(
        "You are a careful science and news writer for Gateless Scholar, an app that makes research readable for everyone.\n\
         RULES:\n\
         1. Write ONLY from the numbered sources and excerpts provided. Never invent facts, numbers, quotes, studies or sources.\n\
         2. After every sentence that states a fact, cite its source number(s) in square brackets: [2] or [2, 5]. Use only the \
         numbers in the SOURCES list.\n\
         3. If the sources do not cover something, say briefly that it is not covered rather than guessing.\n\
         4. Plain, clear English for an educated general reader. Explain technical terms.\n\
         5. No reference list, no bibliography, no title line: the app adds those.\n\
         {CITATION_EXAMPLE}\n\n{}",
        kind_instructions(kind)
    )
}

pub const SMALL_SYSTEM: &str = "You write one section of an article using ONLY the numbered sources given. Put the source number in \
square brackets after every fact, like this: \"Rates fell by 12% in 2023 [2].\" Never invent facts, numbers or sources. Write plain \
prose paragraphs only: no headings, no lists, no reference list.";

pub const CHART_INSTRUCTIONS: &str = "OPTIONAL CHART: if (and only if) the excerpts give several comparable numbers, you may add ONE chart \
inside a section as a fenced block like this:\n```chart\n{\"kind\":\"bar\",\"title\":\"…\",\"x_label\":\"…\",\"y_label\":\"…\",\"series\":[{\"name\":\"…\",\"points\":[[\"label\", 12.5], [\"label 2\", 20]]}],\"citations\":[2]}\n```\n\
kinds: bar, line, stock (use \"series\"); venn (\"sets\":[{\"label\":\"A\",\"size\":40,\"overlaps\":[[1,10]]},{\"label\":\"B\",\"size\":30}]); \
flow for a process diagram (\"nodes\":[\"Step 1\",\"Step 2\"],\"edges\":[[0,1,\"leads to\"]]). Every number in a chart must appear \
in the excerpts of the sources it cites. Leave the chart out if unsure.";

pub fn target_block(target: Option<&Paper>) -> String {
    let Some(p) = target else { return String::new() };
    let mut s = format!("TARGET PAPER: \"{}\"", p.title.trim());
    if !p.authors.is_empty() {
        s.push_str(&format!(" by {}", authors_short(&p.authors)));
    }
    if let Some(y) = p.year {
        s.push_str(&format!(" ({y})"));
    }
    if let Some(v) = &p.venue {
        s.push_str(&format!(", {v}"));
    }
    s.push('\n');
    if let Some(a) = p.abstract_text.as_ref().filter(|a| !a.trim().is_empty()) {
        s.push_str(&format!("ABSTRACT: {}\n", truncate_chars(a.trim(), 3000)));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rerank_prefers_matching_chunks() {
        let chunks = vec![
            Chunk { source_id: 1, text: "General introduction to the field.".into(), section: None, score: 3.0 },
            Chunk { source_id: 2, text: "We used a spectrometer to measure methane emissions.".into(), section: Some("Methods".into()), score: 2.0 },
        ];
        let top = rerank_for_section(&chunks, "Methods in context: spectrometer measurements", 1);
        assert_eq!(top[0].source_id, 2);
    }
}
