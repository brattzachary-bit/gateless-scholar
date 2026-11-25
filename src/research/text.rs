//! Text utilities: tokenising, stopwords, keyword/phrase extraction (query expansion), title
//! normalisation, HTML → paragraphs, sentence splitting and chunking.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use regex::Regex;

use crate::model::FullTextSection;

const STOPWORDS: &[&str] = &[
    "a", "about", "above", "across", "after", "again", "against", "all", "almost", "also", "although", "am", "among", "an", "and", "another", "any",
    "are", "as", "at", "be", "because", "been", "before", "being", "below", "between", "both", "but", "by", "can", "could", "did", "do", "does",
    "doing", "down", "during", "each", "either", "et", "al", "etc", "few", "for", "from", "further", "had", "has", "have", "having", "he", "her",
    "here", "hers", "him", "his", "how", "however", "i", "if", "in", "into", "is", "it", "its", "itself", "just", "may", "me", "might", "more",
    "most", "much", "must", "my", "no", "nor", "not", "now", "of", "off", "on", "once", "one", "only", "or", "other", "our", "out", "over", "own",
    "per", "same", "she", "should", "since", "so", "some", "such", "than", "that", "the", "their", "them", "then", "there", "these", "they",
    "this", "those", "through", "thus", "to", "too", "two", "under", "until", "up", "upon", "us", "use", "used", "using", "very", "via", "was",
    "we", "were", "what", "when", "where", "whether", "which", "while", "who", "whom", "why", "will", "with", "within", "without", "would", "yet",
    "you", "your", "new", "study", "paper", "results", "show", "shows", "based", "approach", "novel", "towards", "toward", "analysis", "says",
    "said", "after", "amid", "over", "first", "s", "vs", "versus", "make", "makes", "made", "could", "get", "gets", "here", "what", "know",
];

static STOP: LazyLock<HashSet<&'static str>> = LazyLock::new(|| STOPWORDS.iter().copied().collect());

pub fn is_stopword(w: &str) -> bool {
    STOP.contains(w)
}

/// Very light stemmer: lowercase, strip common plural/possessive endings.
pub fn stem(w: &str) -> String {
    let w = w.trim_matches('\'');
    let w = w.strip_suffix("'s").unwrap_or(w);
    if w.len() > 4 {
        if let Some(s) = w.strip_suffix("ies") {
            return format!("{s}y");
        }
        if w.ends_with("sses") {
            return w[..w.len() - 2].to_string();
        }
        if w.ends_with('s') && !w.ends_with("ss") && !w.ends_with("us") && !w.ends_with("is") {
            return w[..w.len() - 1].to_string();
        }
    }
    w.to_string()
}

/// Raw lowercase word tokens (letters/digits, inner hyphens split).
pub fn words(text: &str) -> Vec<String> {
    text.split(|c: char| !(c.is_alphanumeric() || c == '\''))
        .map(|w| w.trim_matches('\'').to_lowercase())
        .filter(|w| !w.is_empty())
        .collect()
}

/// Index tokens: stemmed, stopwords removed, length ≥ 2.
pub fn tokens(text: &str) -> Vec<String> {
    words(text).into_iter().filter(|w| w.chars().count() >= 2 && !is_stopword(w)).map(|w| stem(&w)).collect()
}

pub fn token_set(text: &str) -> HashSet<String> {
    tokens(text).into_iter().collect()
}

/// Normalised title for dedupe: lowercase alphanumerics, single spaces.
pub fn normalize_title(t: &str) -> String {
    let stripped = strip_tags(t);
    let mut out = String::with_capacity(stripped.len());
    let mut space = false;
    for c in stripped.chars() {
        if c.is_alphanumeric() {
            for l in c.to_lowercase() {
                out.push(l);
            }
            space = false;
        } else if !space && !out.is_empty() {
            out.push(' ');
            space = true;
        }
    }
    out.trim_end().to_string()
}

/// Keywords of a text in order of first appearance, most salient first (longer, capitalised,
/// containing digits weigh more). Stopwords removed; not stemmed.
pub fn keywords(text: &str, max: usize) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut scored: Vec<(f32, usize, String)> = Vec::new();
    for (i, raw) in text.split(|c: char| !(c.is_alphanumeric() || c == '-')).enumerate() {
        let raw = raw.trim_matches('-');
        if raw.chars().count() < 3 && !raw.chars().any(|c| c.is_ascii_digit()) && !raw.chars().all(|c| c.is_uppercase()) {
            continue;
        }
        let lw = raw.to_lowercase();
        if lw.len() < 2 || is_stopword(&lw) || !seen.insert(stem(&lw)) {
            continue;
        }
        let mut score = (lw.chars().count() as f32).min(12.0) / 4.0;
        if raw.chars().next().is_some_and(char::is_uppercase) && i > 0 {
            score += 0.7;
        }
        if raw.chars().filter(|c| c.is_uppercase()).count() >= 2 {
            score += 1.0; // acronyms: CRISPR, JWST
        }
        if raw.chars().any(|c| c.is_ascii_digit()) {
            score += 0.5;
        }
        scored.push((score, i, raw.to_string()));
    }
    scored.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
    scored.truncate(max);
    // Back to text order so the phrase still reads naturally.
    scored.sort_by_key(|s| s.1);
    scored.into_iter().map(|s| s.2).collect()
}

/// Key noun-phrase-ish runs: consecutive non-stopwords (2–3 words) from a title.
pub fn phrases(text: &str, max: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut run: Vec<&str> = Vec::new();
    let flush = |run: &mut Vec<&str>, out: &mut Vec<String>| {
        if run.len() >= 2 {
            let take = run.len().min(3);
            out.push(run[..take].join(" "));
            if run.len() > 3 {
                out.push(run[run.len() - 2..].join(" "));
            }
        }
        run.clear();
    };
    for w in text.split(|c: char| c.is_whitespace() || matches!(c, ',' | ':' | ';' | '(' | ')' | '.' | '?' | '!' | '"')) {
        let lw = w.to_lowercase();
        if w.is_empty() || is_stopword(&lw) || w.chars().count() < 2 {
            flush(&mut run, &mut out);
        } else {
            run.push(w);
        }
    }
    flush(&mut run, &mut out);
    let mut seen = HashSet::new();
    out.retain(|p| seen.insert(p.to_lowercase()));
    out.truncate(max);
    out
}

/// Code-based query expansion: a keyword query from the text, the key phrases, and extras.
pub fn expand_queries(title: &str, extra: &[String], max: usize) -> Vec<String> {
    let mut qs = Vec::new();
    let kw = keywords(title, 6);
    if !kw.is_empty() {
        qs.push(kw.join(" "));
    }
    for p in phrases(title, 3) {
        if p.split_whitespace().count() >= 2 {
            qs.push(p);
        }
    }
    for e in extra {
        let e = e.trim();
        if !e.is_empty() {
            qs.push(e.to_string());
        }
    }
    let mut seen = HashSet::new();
    qs.retain(|q| seen.insert(normalize_title(q)));
    // Extra queries are deliberate; keep them even when the cap is tight.
    if qs.len() > max {
        let extras: Vec<String> = qs.iter().filter(|q| extra.iter().any(|e| e.trim() == q.as_str())).cloned().collect();
        let mut base: Vec<String> = qs.iter().filter(|q| !extras.contains(q)).cloned().collect();
        base.truncate(max.saturating_sub(extras.len()).max(1));
        base.extend(extras);
        base.truncate(max.max(1));
        qs = base;
    }
    qs
}

pub fn jaccard(a: &HashSet<String>, b: &HashSet<String>) -> f32 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let inter = a.intersection(b).count() as f32;
    inter / (a.len() as f32 + b.len() as f32 - inter)
}

pub fn overlap(a: &HashSet<String>, b: &HashSet<String>) -> (usize, f32) {
    if a.is_empty() || b.is_empty() {
        return (0, 0.0);
    }
    let inter = a.intersection(b).count();
    (inter, inter as f32 / a.len().min(b.len()) as f32)
}

// ------------------------------------------------------------------------------------- HTML

static RE_DROP_BLOCKS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?is)<(script|style|noscript|svg|nav|footer|header|aside|form|button|select|template|iframe)\b[^>]*>.*?</(script|style|noscript|svg|nav|footer|header|aside|form|button|select|template|iframe)\s*>").unwrap()
});
static RE_COMMENT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<!--.*?-->").unwrap());
static RE_MATH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?is)<math\b[^>]*?alttext="([^"]*)"[^>]*>.*?</math>"#).unwrap());
static RE_MATH_ANY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?is)<math\b.*?</math>").unwrap());
static RE_TAG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<[^>]*>").unwrap());
static RE_WS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").unwrap());
static RE_BLOCK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?is)<(h[1-4]|p)\b[^>]*>(.*?)</(?:h[1-4]|p)\s*>").unwrap());
static RE_ARTICLE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?is)<article\b[^>]*>(.*)</article>").unwrap());
static RE_MAIN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?is)<main\b[^>]*>(.*)</main>").unwrap());
static RE_CITE_BRACKET: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s*\[(?:\d+(?:[,–-]\s*\d+)*)\]").unwrap());

/// Strip tags and decode entities, collapsing whitespace.
pub fn strip_tags(s: &str) -> String {
    let no_math = RE_MATH.replace_all(s, " $1 ");
    let no_math = RE_MATH_ANY.replace_all(&no_math, " ");
    let t = RE_TAG.replace_all(&no_math, " ");
    let decoded = html_escape::decode_html_entities(&t);
    let collapsed = RE_WS.replace_all(&decoded, " ");
    tidy_punct(collapsed.trim())
}

fn tidy_punct(s: &str) -> String {
    s.replace(" ,", ",").replace(" .", ".").replace(" ;", ";").replace(" :", ":").replace("( ", "(").replace(" )", ")")
}

fn is_boilerplate(p: &str) -> bool {
    let l = p.to_lowercase();
    const BAD: &[&str] = &[
        "cookie", "subscribe", "newsletter", "sign up", "sign in", "log in", "all rights reserved", "javascript", "advertisement",
        "click here", "terms of use", "privacy policy", "copyright ©", "follow us", "share this", "read more:", "related:",
        "your browser", "enable js", "accept all",
    ];
    BAD.iter().any(|b| l.contains(b)) && p.split_whitespace().count() < 60
}

/// Extract readable sections from an HTML page: headings (h1–h4) start sections, `<p>`
/// elements become paragraphs. Boilerplate is dropped. Prefers the `<article>`/`<main>` region.
pub fn html_sections(html: &str) -> Vec<FullTextSection> {
    let cleaned = RE_COMMENT.replace_all(html, " ");
    let cleaned = RE_DROP_BLOCKS.replace_all(&cleaned, " ");
    let region: &str = if let Some(c) = RE_ARTICLE.captures(&cleaned) {
        c.get(1).map(|m| m.as_str()).unwrap_or(&cleaned)
    } else if let Some(c) = RE_MAIN.captures(&cleaned) {
        c.get(1).map(|m| m.as_str()).unwrap_or(&cleaned)
    } else {
        &cleaned
    };
    let mut sections: Vec<FullTextSection> = vec![FullTextSection { heading: String::new(), paragraphs: vec![] }];
    for cap in RE_BLOCK.captures_iter(region) {
        let tag = cap.get(1).map(|m| m.as_str().to_lowercase()).unwrap_or_default();
        let text = strip_tags(cap.get(2).map(|m| m.as_str()).unwrap_or(""));
        if text.is_empty() {
            continue;
        }
        if tag.starts_with('h') {
            let heading = text.chars().take(160).collect::<String>();
            if sections.last().is_some_and(|s| s.paragraphs.is_empty()) {
                if let Some(last) = sections.last_mut() {
                    last.heading = heading;
                }
            } else {
                sections.push(FullTextSection { heading, paragraphs: vec![] });
            }
        } else {
            let wc = text.split_whitespace().count();
            if wc < 6 || is_boilerplate(&text) {
                continue;
            }
            if let Some(last) = sections.last_mut() {
                last.paragraphs.push(text);
            }
        }
    }
    sections.retain(|s| !s.paragraphs.is_empty());
    // Stop at the reference list.
    if let Some(pos) = sections.iter().position(|s| is_reference_heading(&s.heading)) {
        if pos > 0 {
            sections.truncate(pos);
        }
    }
    sections
}

pub fn is_reference_heading(h: &str) -> bool {
    let l = h.trim().trim_start_matches(|c: char| c.is_ascii_digit() || c == '.' || c == ' ').to_lowercase();
    matches!(l.as_str(), "references" | "bibliography" | "references and notes" | "literature cited" | "works cited" | "acknowledgements" | "acknowledgments")
}

/// Plain paragraphs of a page (e.g. a news article), in order.
pub fn html_paragraphs(html: &str) -> Vec<String> {
    html_sections(html).into_iter().flat_map(|s| s.paragraphs).collect()
}

pub fn word_count(sections: &[FullTextSection]) -> usize {
    sections.iter().flat_map(|s| &s.paragraphs).map(|p| p.split_whitespace().count()).sum()
}

/// `<meta property="og:image" content="...">` (or twitter:image).
pub fn og_image(html: &str) -> Option<String> {
    static RE1: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"(?is)<meta[^>]+(?:property|name)=["'](?:og:image|twitter:image)["'][^>]*content=["']([^"']+)["']"#).unwrap());
    static RE2: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"(?is)<meta[^>]+content=["']([^"']+)["'][^>]*(?:property|name)=["'](?:og:image|twitter:image)["']"#).unwrap());
    RE1.captures(html).or_else(|| RE2.captures(html)).and_then(|c| c.get(1)).map(|m| html_escape::decode_html_entities(m.as_str()).to_string())
}

/// `<meta property="og:site_name">`.
pub fn og_site_name(html: &str) -> Option<String> {
    static RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"(?is)<meta[^>]+property=["']og:site_name["'][^>]*content=["']([^"']+)["']"#).unwrap());
    RE.captures(html).and_then(|c| c.get(1)).map(|m| html_escape::decode_html_entities(m.as_str()).trim().to_string())
}

/// Remove numeric bracket citations like "[12]" or "[3, 4]" (from full texts / citation contexts).
pub fn strip_bracket_citations(s: &str) -> String {
    RE_CITE_BRACKET.replace_all(s, "").to_string()
}

// ------------------------------------------------------------------------------- sentences

/// Split into sentences (simple rules; keeps abbreviations like "e.g." and "et al." intact).
pub fn sentences(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let chars: Vec<char> = text.chars().collect();
    for i in 0..chars.len() {
        let c = chars[i];
        cur.push(c);
        if matches!(c, '.' | '?' | '!') {
            let next = chars.get(i + 1).copied();
            let next2 = chars.get(i + 2).copied();
            let boundary = match next {
                None => true,
                Some(n) if n.is_whitespace() => next2.is_none_or(|n2| n2.is_uppercase() || n2 == '"' || n2 == '(' || n2 == '[' || n2.is_ascii_digit()),
                _ => false,
            };
            if boundary {
                let lw = cur.trim_end_matches(['.', '?', '!']).rsplit(' ').next().unwrap_or("").to_lowercase();
                let abbrev = matches!(lw.as_str(), "e.g" | "i.e" | "al" | "fig" | "figs" | "eq" | "ref" | "refs" | "vs" | "dr" | "mr" | "ms" | "no" | "approx" | "ca" | "cf" | "st")
                    || (lw.len() == 1 && lw.chars().all(|c| c.is_alphabetic()));
                if !abbrev {
                    let s = cur.trim().to_string();
                    if !s.is_empty() {
                        out.push(s);
                    }
                    cur.clear();
                }
            }
        }
    }
    let s = cur.trim().to_string();
    if !s.is_empty() {
        out.push(s);
    }
    out
}

// --------------------------------------------------------------------------------- chunking

/// A chunk before scoring: (section heading, text).
#[derive(Clone, Debug, PartialEq)]
pub struct RawChunk {
    pub section: Option<String>,
    pub text: String,
}

pub const CHUNK_MIN_WORDS: usize = 150;
pub const CHUNK_MAX_WORDS: usize = 250;

/// Chunk sections into ~150–250-word passages, never crossing a section boundary.
pub fn chunk_sections(sections: &[FullTextSection]) -> Vec<RawChunk> {
    let mut out = Vec::new();
    for sec in sections {
        let heading = (!sec.heading.trim().is_empty()).then(|| sec.heading.trim().to_string());
        let mut buf: Vec<String> = Vec::new();
        let mut buf_words = 0usize;
        let flush = |buf: &mut Vec<String>, buf_words: &mut usize, out: &mut Vec<RawChunk>| {
            if !buf.is_empty() {
                out.push(RawChunk { section: heading.clone(), text: buf.join(" ") });
                buf.clear();
                *buf_words = 0;
            }
        };
        // Break long paragraphs into sentence groups first.
        let mut pieces: Vec<String> = Vec::new();
        for p in &sec.paragraphs {
            let wc = p.split_whitespace().count();
            if wc <= CHUNK_MAX_WORDS {
                pieces.push(p.clone());
            } else {
                let mut cur = String::new();
                let mut cw = 0;
                // Sentences, with over-long "sentences" (lists, tables) cut into word windows.
                let parts: Vec<String> = sentences(p)
                    .into_iter()
                    .flat_map(|s| {
                        let w: Vec<&str> = s.split_whitespace().collect();
                        if w.len() <= CHUNK_MAX_WORDS { vec![s] } else { w.chunks(CHUNK_MIN_WORDS).map(|c| c.join(" ")).collect() }
                    })
                    .collect();
                for s in parts {
                    let sw = s.split_whitespace().count();
                    if cw > 0 && cw + sw > CHUNK_MAX_WORDS {
                        pieces.push(std::mem::take(&mut cur));
                        cw = 0;
                    }
                    if !cur.is_empty() {
                        cur.push(' ');
                    }
                    cur.push_str(&s);
                    cw += sw;
                }
                if !cur.is_empty() {
                    pieces.push(cur);
                }
            }
        }
        for piece in pieces {
            let wc = piece.split_whitespace().count();
            if buf_words > 0 && buf_words + wc > CHUNK_MAX_WORDS {
                flush(&mut buf, &mut buf_words, &mut out);
            }
            buf.push(piece);
            buf_words += wc;
            if buf_words >= CHUNK_MIN_WORDS {
                flush(&mut buf, &mut buf_words, &mut out);
            }
        }
        // A short tail joins the previous chunk of the same section when that stays in bounds.
        if !buf.is_empty() {
            let tail = buf.join(" ");
            let tail_words = buf_words;
            if let Some(prev) = out.last_mut() {
                if prev.section == heading && tail_words < 60 && prev.text.split_whitespace().count() + tail_words <= CHUNK_MAX_WORDS + 60 {
                    prev.text.push(' ');
                    prev.text.push_str(&tail);
                    buf.clear();
                }
            }
            flush(&mut buf, &mut buf_words, &mut out);
        }
    }
    out
}

/// Chunk plain text (e.g. an abstract) under one heading.
pub fn chunk_text(heading: Option<&str>, text: &str) -> Vec<RawChunk> {
    let paragraphs: Vec<String> = text.split("\n\n").map(|p| p.trim().to_string()).filter(|p| !p.is_empty()).collect();
    chunk_sections(&[FullTextSection { heading: heading.unwrap_or("").to_string(), paragraphs }])
}

/// Split PDF-extracted text into sections using common heading lines. Best-effort.
pub fn pdf_text_sections(text: &str) -> Vec<FullTextSection> {
    static RE_HEAD: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)^\s*(?:(?:\d{1,2}|[IVX]{1,4})\.?\s+)?(abstract|introduction|background|related work|methods?|materials and methods|methodology|results|results and discussion|discussion|conclusions?|summary|experiments?|evaluation|limitations|references|bibliography|acknowledge?ments)\s*$").unwrap()
    });
    let mut sections = vec![FullTextSection { heading: String::new(), paragraphs: vec![] }];
    let mut para = String::new();
    let push_para = |para: &mut String, sections: &mut Vec<FullTextSection>| {
        let p = RE_WS.replace_all(para.trim(), " ").to_string();
        if p.split_whitespace().count() >= 8 {
            if let Some(s) = sections.last_mut() {
                s.paragraphs.push(p);
            }
        }
        para.clear();
    };
    for line in text.lines() {
        let t = line.trim();
        if t.is_empty() {
            push_para(&mut para, &mut sections);
            continue;
        }
        if t.len() < 60 && RE_HEAD.is_match(t) {
            push_para(&mut para, &mut sections);
            let heading = t.to_string();
            if is_reference_heading(&heading) {
                break;
            }
            sections.push(FullTextSection { heading, paragraphs: vec![] });
            continue;
        }
        // Join hyphenated line breaks.
        if para.ends_with('-') {
            para.pop();
        } else if !para.is_empty() {
            para.push(' ');
        }
        para.push_str(t);
        if para.split_whitespace().count() > 220 && (t.ends_with('.') || t.ends_with(".)")) {
            push_para(&mut para, &mut sections);
        }
    }
    push_para(&mut para, &mut sections);
    sections.retain(|s| !s.paragraphs.is_empty());
    sections
}

/// Count how often each term occurs; tiny helper for BM25 and clustering.
pub fn term_freq(tokens: &[String]) -> HashMap<&str, u32> {
    let mut m = HashMap::new();
    for t in tokens {
        *m.entry(t.as_str()).or_insert(0) += 1;
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn research_html_paragraph_extraction() {
        let html = r#"<html><head><style>p{}</style><script>var x="<p>no</p>";</script></head>
        <body><nav><p>Home | About | Contact us today please</p></nav>
        <article><h1>Big discovery</h1><p>Scientists have found a <b>new</b> kind of &amp; particle in the detector.</p>
        <p>Subscribe to our newsletter</p>
        <h2>Method</h2><p>The team used <a href="x">a large</a> collider over many years of running.</p>
        <p>We measured <math alttext="E=mc^2"><mi>E</mi></math> carefully in all the runs.</p>
        <h2>References</h2><p>[1] Smith J. A paper title that is long enough.</p></article></body></html>"#;
        let secs = html_sections(html);
        assert_eq!(secs.len(), 2, "{secs:?}");
        assert_eq!(secs[0].heading, "Big discovery");
        assert_eq!(secs[0].paragraphs, vec!["Scientists have found a new kind of & particle in the detector."]);
        assert_eq!(secs[1].heading, "Method");
        assert_eq!(secs[1].paragraphs[0], "The team used a large collider over many years of running.");
        assert!(secs[1].paragraphs[1].contains("E=mc^2"));
        assert!(og_image(r#"<meta property="og:image" content="https://x/y.jpg?a=1&amp;b=2">"#).is_some_and(|u| u == "https://x/y.jpg?a=1&b=2"));
    }

    #[test]
    fn research_chunking() {
        let para = |n: usize| (0..n).map(|i| format!("word{i}")).collect::<Vec<_>>().join(" ") + ".";
        let secs = vec![
            FullTextSection { heading: "Intro".into(), paragraphs: vec![para(100), para(100), para(30)] },
            FullTextSection { heading: "Methods".into(), paragraphs: vec![para(600)] },
        ];
        let chunks = chunk_sections(&secs);
        for c in &chunks {
            let wc = c.text.split_whitespace().count();
            assert!(wc <= CHUNK_MAX_WORDS + 60, "chunk too long: {wc}");
        }
        assert!(chunks.iter().any(|c| c.section.as_deref() == Some("Methods")));
        assert_eq!(chunks.iter().filter(|c| c.section.as_deref() == Some("Intro")).count(), 1); // 200 words + short 30-word tail merged
        let total: usize = chunks.iter().map(|c| c.text.split_whitespace().count()).sum();
        assert_eq!(total, 830);
    }

    #[test]
    fn research_sentences_and_keywords() {
        let s = sentences("Smith et al. found it. This is e.g. fine! Is it? Yes. 3.5 percent rose. J. Smith agreed.");
        assert_eq!(s, vec!["Smith et al. found it.", "This is e.g. fine!", "Is it?", "Yes.", "3.5 percent rose.", "J. Smith agreed."]);
        let kw = keywords("CRISPR base editing of the human genome in vivo", 4);
        assert!(kw.contains(&"CRISPR".to_string()));
        assert!(!kw.iter().any(|k| k == "the" || k == "of"));
        let ph = phrases("Deep learning for galaxy morphology classification", 3);
        assert_eq!(ph[0], "Deep learning");
        assert_eq!(normalize_title("The <i>Foo</i>-Bar: a  study!"), "the foo bar a study");
    }

    #[test]
    fn research_pdf_sections() {
        let t = "Some Title\n\nAbstract\nThis is the abstract of the paper with enough words in it.\n\n1 Introduction\nWe intro-\nduce a thing that is really quite interesting to many people.\n\nReferences\n[1] Foo bar baz qux quux corge grault garply.";
        let s = pdf_text_sections(t);
        assert_eq!(s.len(), 2);
        assert_eq!(s[1].heading, "1 Introduction");
        assert!(s[1].paragraphs[0].contains("introduce a thing"));
    }
}
