//! Code-enforced honesty checks on model output.
//!
//! - Citations: `[n]` markers are parsed out of every sentence into `Passage::citations`;
//!   numbers that do not map to a supplied source are removed and counted.
//! - Unverified claims: a sentence with no valid citation that looks like a factual claim is
//!   kept but marked `unverified = true` (shown with an "unverified" mark) rather than dropped,
//!   so the reader sees exactly what the model wrote and what is unsupported. Heuristic: it
//!   contains a number, date or percentage, or claim words ("found", "showed", "reported", …),
//!   or it is longer than 12 words and is not a transitional, hedged or question sentence.
//!   A sentence whose only citations pointed to non-existent sources is also marked unverified.
//! - Rewording of originals: a reworded sentence is accepted only if it stays very close to the
//!   original (token-set similarity ≥ 0.6, length ratio 0.7–1.4, identical numbers); otherwise
//!   the original sentence is kept.

use std::collections::HashSet;

use crate::model::{Origin, Passage, ValidationReport};

use super::sentences::{extract_citations, split_sentences, tidy_spaces};

pub const MIN_SIMILARITY: f32 = 0.6;
pub const MIN_LEN_RATIO: f32 = 0.7;
pub const MAX_LEN_RATIO: f32 = 1.4;

/// Turn one paragraph of model text into validated AI-written passages (one per sentence).
pub fn ai_passages(paragraph: &str, valid: &HashSet<u32>, report: &mut ValidationReport) -> Vec<Passage> {
    let mut out: Vec<Passage> = Vec::new();
    for sentence in split_sentences(paragraph) {
        let (text, ids) = extract_citations(&sentence);
        let mut cites: Vec<u32> = Vec::new();
        let mut invalid: HashSet<u32> = HashSet::new();
        for id in ids {
            if valid.contains(&id) {
                if !cites.contains(&id) {
                    cites.push(id);
                }
            } else {
                invalid.insert(id);
            }
        }
        report.invalid_citations_removed += invalid.len() as u32;
        let text = strip_markdown(&text);
        if text.chars().all(|c| !c.is_alphanumeric()) {
            // A sentence that was only citation markers: they belong to the previous sentence.
            if let Some(prev) = out.last_mut() {
                for id in cites {
                    if !prev.citations.contains(&id) {
                        prev.citations.push(id);
                    }
                }
                if !prev.citations.is_empty() && prev.unverified {
                    prev.unverified = false;
                    report.unverified_sentences = report.unverified_sentences.saturating_sub(1);
                }
            }
            continue;
        }
        let mut p = Passage::ai(text, cites);
        // Uncited factual claims, and sentences whose only citations were invented (the model
        // itself thought they needed a source), are marked unverified.
        let only_invalid = p.citations.is_empty() && !invalid.is_empty();
        if p.citations.is_empty() && (only_invalid || is_factual_claim(&p.text)) {
            p.unverified = true;
            report.unverified_sentences += 1;
        }
        out.push(p);
    }
    // Space between sentences so passages can simply be concatenated for display.
    let n = out.len();
    for (i, p) in out.iter_mut().enumerate() {
        if i + 1 < n {
            p.text.push(' ');
        }
    }
    out
}

/// Remove light markdown emphasis the model may use.
pub fn strip_markdown(s: &str) -> String {
    let t = s.replace("**", "").replace("__", "");
    let t = t.trim_start_matches(['>', ' ']).to_string();
    // Single *emphasis* / _emphasis_ around words.
    let t = regex::Regex::new(r"(^|[\s(])[*_]([^*_\n]+)[*_]([\s).,;:!?]|$)")
        .map(|re| re.replace_all(&t, "$1$2$3").to_string())
        .unwrap_or(t);
    tidy_spaces(&t)
}

const CLAIM_WORDS: &[&str] = &[
    "found", "find", "finds", "showed", "shows", "show", "shown", "demonstrated", "demonstrates", "reported", "reports",
    "measured", "observed", "discovered", "detected", "identified", "revealed", "reveals", "increased", "decreased",
    "reduced", "increases", "decreases", "caused", "causes", "according", "percent", "million", "billion", "thousand",
    "estimated", "announced", "said", "says", "confirmed", "killed", "died", "won", "approved", "published", "launched",
    "outperformed", "outperforms", "achieved", "achieves", "improved", "improves", "doubled", "tripled", "record",
];

const HEDGES: &[&str] = &[
    "may", "might", "could", "perhaps", "possibly", "unclear", "unknown", "open question", "remains to be", "future work",
    "further research", "it is worth", "do not cover", "does not cover", "not covered", "do not address", "does not address",
    "do not discuss", "does not discuss", "the sources", "the abstract does not", "in this section", "this article", "this section", "we now", "in summary", "overall,",
    "taken together", "to understand", "in short", "in other words", "as discussed", "as noted", "below", "above",
];

/// Heuristic: does this uncited sentence assert a fact that needs a source?
pub fn is_factual_claim(sentence: &str) -> bool {
    let s = sentence.trim();
    if s.ends_with('?') {
        return false;
    }
    let lower = s.to_lowercase();
    let words: Vec<&str> = lower.split(|c: char| !c.is_alphanumeric() && c != '%').filter(|w| !w.is_empty()).collect();
    if words.len() < 4 {
        return false;
    }
    if s.chars().any(|c| c.is_ascii_digit()) || s.contains('%') {
        return true;
    }
    if words.iter().any(|w| CLAIM_WORDS.contains(w)) {
        return true;
    }
    if HEDGES.iter().any(|h| lower.contains(h)) {
        return false;
    }
    // Summarising/linking sentences ("This makes…", "These areas…") restate the cited
    // sentences before them rather than adding a new fact.
    if ["this ", "these ", "such ", "together", "thus", "therefore", "in turn"].iter().any(|p| lower.starts_with(p)) {
        return false;
    }
    words.len() > 12
}

// ---------------------------------------------------------------------------------------------
// Rewording checks
// ---------------------------------------------------------------------------------------------

pub fn tokens(s: &str) -> Vec<String> {
    s.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(String::from).collect()
}

/// Jaccard similarity of the two sentences' word sets.
pub fn token_similarity(a: &str, b: &str) -> f32 {
    let a: HashSet<String> = tokens(a).into_iter().collect();
    let b: HashSet<String> = tokens(b).into_iter().collect();
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    let inter = a.intersection(&b).count() as f32;
    let union = a.union(&b).count() as f32;
    inter / union
}

/// Numbers in a sentence, normalised ("1,200" → "1200"), sorted.
pub fn numbers_in(s: &str) -> Vec<String> {
    let re = regex::Regex::new(r"\d[\d,]*(?:\.\d+)?").expect("regex");
    let mut v: Vec<String> = re.find_iter(s).map(|m| m.as_str().replace(',', "").trim_end_matches('.').to_string()).collect();
    v.sort();
    v
}

fn normalise_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Is `reworded` an acceptable light rewording of `original`?
pub fn accept_rewording(original: &str, reworded: &str) -> bool {
    let (o, r) = (original.trim(), reworded.trim());
    if r.is_empty() || o.is_empty() {
        return false;
    }
    let ratio = r.chars().count() as f32 / o.chars().count() as f32;
    if !(MIN_LEN_RATIO..=MAX_LEN_RATIO).contains(&ratio) {
        return false;
    }
    if numbers_in(o) != numbers_in(r) {
        return false;
    }
    token_similarity(o, r) >= MIN_SIMILARITY
}

/// Align an original paragraph with the model's rewording sentence by sentence. Identical
/// sentences stay `Original`; accepted changes become `Reworded` (keeping the original
/// wording); anything else reverts to the original sentence (counted in the report).
pub fn align_rewording(original: &str, reworded: &str, report: &mut ValidationReport) -> Vec<Passage> {
    let orig = split_sentences(original);
    let rew = split_sentences(reworded);
    let mut out = Vec::with_capacity(orig.len());
    let mut j = 0;
    for o in &orig {
        let mut chosen: Option<Passage> = None;
        for (idx, cand) in rew.iter().enumerate().skip(j).take(3) {
            if normalise_ws(cand) == normalise_ws(o) {
                chosen = Some(Passage::original(o.clone()));
                j = idx + 1;
                break;
            }
            if accept_rewording(o, cand) {
                chosen = Some(Passage {
                    text: cand.trim().to_string(),
                    origin: Origin::Reworded,
                    citations: vec![],
                    original: Some(o.clone()),
                    unverified: false,
                });
                j = idx + 1;
                break;
            }
        }
        match chosen {
            Some(p) => out.push(p),
            None => {
                // Keep `j` where it is: the model may have merged or dropped a sentence, and
                // the next original sentence may still match the next reworded one.
                if j < rew.len() {
                    report.rewordings_reverted += 1;
                }
                out.push(Passage::original(o.clone()));
            }
        }
    }
    let n = out.len();
    for (i, p) in out.iter_mut().enumerate() {
        if i + 1 < n {
            p.text.push(' ');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid(ids: &[u32]) -> HashSet<u32> {
        ids.iter().copied().collect()
    }

    #[test]
    fn citations_become_passage_fields() {
        let mut r = ValidationReport::default();
        let ps = ai_passages("Ice was detected at the south pole [1]. It was confirmed later [2, 3].", &valid(&[1, 2, 3]), &mut r);
        assert_eq!(ps.len(), 2);
        assert_eq!(ps[0].text.trim(), "Ice was detected at the south pole.");
        assert_eq!(ps[0].citations, vec![1]);
        assert_eq!(ps[1].citations, vec![2, 3]);
        assert!(ps.iter().all(|p| p.origin == Origin::AiWritten && !p.unverified));
        assert_eq!(r.invalid_citations_removed, 0);
    }

    #[test]
    fn invalid_citations_removed_and_counted() {
        let mut r = ValidationReport::default();
        let ps = ai_passages("The rate rose by 12% in 2023 [2, 9]. Another claim was reported by Smith [14].", &valid(&[1, 2]), &mut r);
        assert_eq!(ps[0].citations, vec![2]);
        assert!(!ps[0].unverified);
        assert!(ps[1].citations.is_empty());
        assert!(ps[1].unverified, "claim left without a valid source must be unverified");
        assert_eq!(r.invalid_citations_removed, 2);
        assert_eq!(r.unverified_sentences, 1);
        assert!(!ps[1].text.contains('['));
    }

    #[test]
    fn unverified_flagging_heuristic() {
        assert!(is_factual_claim("The trial enrolled 340 patients across Europe."));
        assert!(is_factual_claim("Researchers found that the protein folds faster in cold water."));
        assert!(!is_factual_claim("Why does this matter?"));
        assert!(!is_factual_claim("This section looks at the wider context."));
        assert!(!is_factual_claim("It is short."));
        assert!(!is_factual_claim("It remains unclear whether the effect generalises to other materials and species."));
        assert!(is_factual_claim("The new telescope gives astronomers a much sharper view of the early universe than before."));
    }

    #[test]
    fn marker_only_sentence_attaches_to_previous() {
        let mut r = ValidationReport::default();
        let ps = ai_passages("Researchers found a link between sleep and memory. [4]", &valid(&[4]), &mut r);
        assert_eq!(ps.len(), 1);
        assert_eq!(ps[0].citations, vec![4]);
        assert!(!ps[0].unverified);
    }

    #[test]
    fn rewording_accept_and_revert() {
        let mut r = ValidationReport::default();
        let orig = "The samples were then heated to 400 K for two hours. Results are shown in Fig. 3. The method is robust.";
        let rew = "The samples were then heated to 400 K over two hours. Results are shown in Fig. 3. Cats are wonderful pets that everyone should own.";
        let ps = align_rewording(orig, rew, &mut r);
        assert_eq!(ps.len(), 3);
        assert_eq!(ps[0].origin, Origin::Reworded);
        assert_eq!(ps[0].original.as_deref(), Some("The samples were then heated to 400 K for two hours."));
        assert_eq!(ps[1].origin, Origin::Original);
        assert_eq!(ps[2].origin, Origin::Original);
        assert_eq!(ps[2].text, "The method is robust.");
        assert_eq!(r.rewordings_reverted, 1);
    }

    #[test]
    fn rewording_that_changes_numbers_is_rejected() {
        assert!(!accept_rewording("The yield was 45% at 300 K.", "The yield was 54% at 300 K."));
        assert!(accept_rewording("The yield was 45% at 300 K.", "The yield reached 45% at 300 K."));
        assert!(!accept_rewording("Short.", "A much much longer sentence that says far more than the original did."));
    }

    #[test]
    fn rewording_with_merged_sentences_falls_back() {
        let mut r = ValidationReport::default();
        let orig = "We measured the flux. It was low. The sky was clear.";
        let rew = "We measured the flux, which was low. The sky was clear.";
        let ps = align_rewording(orig, rew, &mut r);
        assert_eq!(ps.len(), 3);
        assert_eq!(ps.iter().map(|p| p.text.trim().to_string()).collect::<Vec<_>>(), vec!["We measured the flux.", "It was low.", "The sky was clear."]);
        assert!(ps.iter().all(|p| p.origin == Origin::Original));
    }
}
