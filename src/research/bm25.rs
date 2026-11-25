//! BM25 ranking over passages (k1 = 1.2, b = 0.75).

use std::collections::HashMap;

use super::text::tokens;

const K1: f32 = 1.2;
const B: f32 = 0.75;

/// Score every document against the query. Returns one score per document (same order).
pub fn bm25_scores(query: &str, docs: &[&str]) -> Vec<f32> {
    let q: Vec<String> = {
        let mut t = tokens(query);
        t.sort();
        t.dedup();
        t
    };
    if docs.is_empty() {
        return vec![];
    }
    let doc_tokens: Vec<Vec<String>> = docs.iter().map(|d| tokens(d)).collect();
    let n = docs.len() as f32;
    let avgdl = (doc_tokens.iter().map(Vec::len).sum::<usize>() as f32 / n).max(1.0);
    let mut df: HashMap<&str, u32> = HashMap::new();
    for dt in &doc_tokens {
        let mut seen: Vec<&str> = dt.iter().map(String::as_str).collect();
        seen.sort_unstable();
        seen.dedup();
        for t in seen {
            *df.entry(t).or_insert(0) += 1;
        }
    }
    doc_tokens
        .iter()
        .map(|dt| {
            let tf = super::text::term_freq(dt);
            let dl = dt.len() as f32;
            q.iter()
                .map(|term| {
                    let f = *tf.get(term.as_str()).unwrap_or(&0) as f32;
                    if f == 0.0 {
                        return 0.0;
                    }
                    let d = *df.get(term.as_str()).unwrap_or(&0) as f32;
                    let idf = ((n - d + 0.5) / (d + 0.5) + 1.0).ln();
                    idf * f * (K1 + 1.0) / (f + K1 * (1.0 - B + B * dl / avgdl))
                })
                .sum()
        })
        .collect()
}

/// Indices of documents sorted best first.
#[allow(dead_code)]
pub fn rank(query: &str, docs: &[&str]) -> Vec<(usize, f32)> {
    let scores = bm25_scores(query, docs);
    let mut idx: Vec<(usize, f32)> = scores.into_iter().enumerate().collect();
    idx.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    idx
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn research_bm25_ranking() {
        let docs = [
            "The cat sat on the mat and purred.",
            "Black holes emit Hawking radiation according to quantum field theory.",
            "Hawking radiation from black holes was predicted in 1974; black hole evaporation follows.",
            "Gardening tips for spring.",
        ];
        let r = rank("black hole Hawking radiation", &docs);
        assert_eq!(r[0].0, 2);
        assert_eq!(r[1].0, 1);
        assert!(r[2].1 == 0.0 && r[3].1 == 0.0);
        assert!(bm25_scores("anything", &[]).is_empty());
    }
}
