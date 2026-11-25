//! Sentence splitting and inline-citation marker parsing.
//!
//! Splitting handles abbreviations ("et al.", "e.g.", "Fig.", initials), decimals ("3.5"),
//! and keeps citation markers that follow the full stop ("… finding. [3]") with the sentence
//! they belong to.

/// Abbreviations after which a full stop does not end a sentence (lowercase, without the dot).
const ABBREVIATIONS: &[&str] = &[
    "al", "e.g", "i.e", "eg", "ie", "fig", "figs", "eq", "eqs", "ref", "refs", "vs", "cf", "approx", "ca", "c", "no", "nos",
    "vol", "vols", "pp", "p", "dr", "mr", "mrs", "ms", "prof", "st", "jr", "sr", "inc", "ltd", "co", "corp", "dept", "univ",
    "sec", "secs", "ch", "tab", "suppl", "est", "u.s", "u.k", "e.u", "u.n", "ph.d", "resp", "viz", "sp", "spp", "gen", "jan", "feb",
    "mar", "apr", "jun", "jul", "aug", "sep", "sept", "oct", "nov", "dec", "mt", "ft", "approx", "min", "max", "avg", "no",
];

/// Parse a citation marker starting at `chars[start] == '['`: `[3]`, `[3, 5]`, `[2–4]`,
/// `[Source 3]`, `[S3; S4]`. Returns (index just past `]`, source numbers).
pub fn citation_marker_at(chars: &[char], start: usize) -> Option<(usize, Vec<u32>)> {
    if chars.get(start) != Some(&'[') {
        return None;
    }
    let mut end = None;
    for (k, c) in chars.iter().enumerate().skip(start + 1).take(48) {
        match c {
            ']' => {
                end = Some(k);
                break;
            }
            '[' | '\n' => return None,
            _ => {}
        }
    }
    let end = end?;
    let inner: String = chars[start + 1..end].iter().collect();
    parse_marker_inner(&inner).map(|ids| (end + 1, ids))
}

fn parse_marker_inner(inner: &str) -> Option<Vec<u32>> {
    let lower = inner.to_lowercase();
    let mut cleaned = lower.replace("sources", " ").replace("source", " ").replace("refs", " ").replace("ref", " ");
    cleaned = cleaned.replace(" and ", ",").replace('^', " ").replace('#', " ");
    // "s3" style prefixes.
    let cleaned: String = {
        let cs: Vec<char> = cleaned.chars().collect();
        cs.iter()
            .enumerate()
            .map(|(i, &c)| if c == 's' && cs.get(i + 1).is_some_and(|n| n.is_ascii_digit()) { ' ' } else { c })
            .collect()
    };
    let mut ids = Vec::new();
    let mut any = false;
    for token in cleaned.split([',', ';']) {
        let t = token.trim();
        if t.is_empty() {
            continue;
        }
        let range: Vec<&str> = t.split(['-', '–', '—']).map(str::trim).collect();
        match range.as_slice() {
            [n] => ids.push(n.parse::<u32>().ok()?),
            [a, b] => {
                let a = a.parse::<u32>().ok()?;
                let b = b.parse::<u32>().ok()?;
                if b >= a && b - a <= 30 {
                    ids.extend(a..=b);
                } else {
                    ids.push(a);
                    ids.push(b);
                }
            }
            _ => return None,
        }
        any = true;
    }
    if !any { None } else { Some(ids) }
}

/// Remove all citation markers from a text, returning (clean text, cited numbers in order).
pub fn extract_citations(text: &str) -> (String, Vec<u32>) {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut ids = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '[' {
            if let Some((end, found)) = citation_marker_at(&chars, i) {
                ids.extend(found);
                i = end;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    (tidy_spaces(&out), ids)
}

/// Collapse runs of whitespace and remove spaces left before punctuation by removed markers.
pub fn tidy_spaces(s: &str) -> String {
    let collapsed = s.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut out = String::with_capacity(collapsed.len());
    let chars: Vec<char> = collapsed.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        if c == ' ' && chars.get(i + 1).is_some_and(|n| matches!(n, '.' | ',' | ';' | ':' | '!' | '?' | ')')) {
            continue;
        }
        out.push(c);
    }
    out.replace("()", "").replace("( )", "").trim().to_string()
}

fn is_closer(c: char) -> bool {
    matches!(c, '"' | '\'' | '”' | '’' | ')' | ']' | '»')
}

/// The word (letters and inner dots) right before position `dot`.
fn word_before(chars: &[char], dot: usize) -> String {
    let mut k = dot;
    while k > 0 {
        let c = chars[k - 1];
        if c.is_alphanumeric() || c == '.' {
            k -= 1;
        } else {
            break;
        }
    }
    chars[k..dot].iter().collect()
}

/// Split a paragraph into sentences.
pub fn split_sentences(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.split_whitespace().collect::<Vec<_>>().join(" ").chars().collect();
    let mut out = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if matches!(c, '.' | '!' | '?' | '…') {
            // Absorb repeated punctuation and closing quotes/brackets ("?!", ".)", '."').
            let mut j = i + 1;
            while j < chars.len() && (matches!(chars[j], '.' | '!' | '?') || (is_closer(chars[j]) && chars[j] != ']')) {
                j += 1;
            }
            // Must be followed by whitespace (or a citation marker, or the end).
            let at_end = j >= chars.len();
            let followed_ok = at_end || chars[j] == ' ' || chars[j] == '[';
            if !followed_ok {
                i += 1;
                continue;
            }
            if c == '.' && j == i + 1 {
                let w = word_before(&chars, i);
                let wl = w.to_lowercase();
                let is_abbrev = ABBREVIATIONS.contains(&wl.as_str())
                    || (w.chars().count() == 1 && w.chars().all(|ch| ch.is_uppercase()))
                    || (wl.contains('.') && wl.len() <= 6);
                if is_abbrev && !at_end {
                    // "et al. [3] showed" / "e.g. the" — only a boundary if a capital follows
                    // and the abbreviation is not one that usually precedes a name or number.
                    let next = chars[j..].iter().find(|c| !c.is_whitespace());
                    let never_ends = matches!(wl.as_str(), "e.g" | "i.e" | "eg" | "ie" | "fig" | "figs" | "eq" | "eqs" | "ref" | "refs" | "vs" | "cf" | "dr" | "mr" | "mrs" | "ms" | "prof" | "st" | "approx" | "ca" | "c" | "sec" | "ch" | "tab" | "mt");
                    let number_abbrev = matches!(wl.as_str(), "no" | "nos" | "p" | "pp" | "vol") && next.is_some_and(|c| c.is_ascii_digit());
                    if never_ends || number_abbrev || w.chars().count() == 1 || !next.is_some_and(|c| c.is_uppercase()) {
                        i = j;
                        continue;
                    }
                }
            }
            // Absorb citation markers that directly follow: "... found. [3] [4]"
            let mut k = j;
            loop {
                let mut m = k;
                while m < chars.len() && chars[m] == ' ' {
                    m += 1;
                }
                match citation_marker_at(&chars, m) {
                    Some((end, _)) => k = end,
                    None => break,
                }
            }
            if k > j {
                // Followed by punctuation again ("[3]." )? absorb it.
                while k < chars.len() && matches!(chars[k], '.' | ',' | ';') {
                    k += 1;
                }
            }
            // Next sentence should start with something sentence-like.
            let next = chars[k..].iter().find(|c| !c.is_whitespace()).copied();
            let boundary = match next {
                None => true,
                Some(n) => n.is_uppercase() || n.is_ascii_digit() || matches!(n, '"' | '“' | '‘' | '\'' | '(' | '[' | '*' | '-' | '–') || !n.is_alphabetic(),
            };
            if boundary {
                let s: String = chars[start..k].iter().collect();
                let s = s.trim();
                if !s.is_empty() {
                    out.push(s.to_string());
                }
                start = k;
                i = k;
                continue;
            }
            i = j;
            continue;
        }
        i += 1;
    }
    let rest: String = chars[start.min(chars.len())..].iter().collect();
    let rest = rest.trim();
    if !rest.is_empty() {
        out.push(rest.to_string());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_simple() {
        let s = split_sentences("The cat sat. The dog ran! Did it? Yes.");
        assert_eq!(s, vec!["The cat sat.", "The dog ran!", "Did it?", "Yes."]);
    }

    #[test]
    fn handles_abbreviations_and_decimals() {
        let s = split_sentences("Smith et al. showed a 3.5% rise (Fig. 2). Other work, e.g. Jones, disagrees. J. R. Tolkien wrote it.");
        assert_eq!(s.len(), 3, "{s:?}");
        assert!(s[0].contains("3.5%") && s[0].contains("Fig. 2"));
        assert!(s[1].starts_with("Other work, e.g. Jones"));
        assert_eq!(s[2], "J. R. Tolkien wrote it.");
    }

    #[test]
    fn keeps_trailing_citations_with_sentence() {
        let s = split_sentences("Ice was found near the pole. [2] Later surveys agreed [3, 4]. A third result followed. [5].");
        assert_eq!(s, vec!["Ice was found near the pole. [2]", "Later surveys agreed [3, 4].", "A third result followed. [5]."]);
    }

    #[test]
    fn lowercase_after_period_is_not_a_boundary() {
        let s = split_sentences("Values were approx. ten times higher. Next.");
        assert_eq!(s.len(), 2, "{s:?}");
    }

    #[test]
    fn parses_citation_markers() {
        assert_eq!(extract_citations("A fact [1].").1, vec![1]);
        assert_eq!(extract_citations("A fact [1, 3; 5].").1, vec![1, 3, 5]);
        assert_eq!(extract_citations("A fact [2–4].").1, vec![2, 3, 4]);
        assert_eq!(extract_citations("A fact [2-3][7].").1, vec![2, 3, 7]);
        assert_eq!(extract_citations("A fact [Source 6].").1, vec![6]);
        assert_eq!(extract_citations("A fact [S2, S3].").1, vec![2, 3]);
        let (t, ids) = extract_citations("Arrays [a] stay [1] .");
        assert_eq!(t, "Arrays [a] stay.");
        assert_eq!(ids, vec![1]);
        assert_eq!(extract_citations("Found it [3].").0, "Found it.");
    }
}
