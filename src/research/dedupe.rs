//! Deduplicate papers from several APIs by DOI / arXiv id / normalised title, merging fields.

use std::collections::HashMap;

use crate::model::{ContentKind, Paper, normalize_doi};

use super::text::normalize_title;

/// Strip the version suffix from an arXiv id ("2401.01234v2" → "2401.01234").
pub fn clean_arxiv_id(id: &str) -> String {
    let id = id.trim();
    let id = id
        .trim_start_matches("https://arxiv.org/abs/")
        .trim_start_matches("http://arxiv.org/abs/")
        .trim_start_matches("https://arxiv.org/pdf/")
        .trim_start_matches("http://arxiv.org/pdf/")
        .trim_start_matches("arXiv:")
        .trim_start_matches("arxiv:")
        .trim_end_matches(".pdf");
    match id.rfind('v') {
        Some(pos) if pos > 0 && id[pos + 1..].chars().all(|c| c.is_ascii_digit()) && !id[pos + 1..].is_empty() => id[..pos].to_string(),
        _ => id.to_string(),
    }
}

/// arXiv id from an arXiv DataCite DOI (10.48550/arXiv.2401.01234) or an arxiv.org URL.
pub fn arxiv_from_doi_or_url(s: &str) -> Option<String> {
    let l = s.to_lowercase();
    if let Some(rest) = normalize_doi(&l).strip_prefix("10.48550/arxiv.") {
        return Some(clean_arxiv_id(rest));
    }
    for p in ["arxiv.org/abs/", "arxiv.org/pdf/", "arxiv.org/html/"] {
        if let Some(i) = l.find(p) {
            let rest = &s[i + p.len()..];
            let rest = rest.split(['?', '#']).next().unwrap_or(rest).trim_end_matches('/');
            if !rest.is_empty() {
                return Some(clean_arxiv_id(rest));
            }
        }
    }
    None
}

fn ids(p: &Paper) -> Vec<String> {
    let mut v = Vec::new();
    if let Some(d) = &p.doi {
        v.push(format!("doi:{}", normalize_doi(d)));
    }
    if let Some(a) = &p.arxiv_id {
        v.push(format!("arxiv:{}", clean_arxiv_id(a).to_lowercase()));
    }
    if let Some(o) = &p.openalex_id {
        v.push(format!("oa:{}", o.rsplit('/').next().unwrap_or(o)));
    }
    if let Some(s) = &p.s2_id {
        v.push(format!("s2:{s}"));
    }
    if let Some(u) = &p.url {
        v.push(format!("url:{}", u.trim_end_matches('/').trim_start_matches("https://").trim_start_matches("http://")));
    }
    let t = normalize_title(&p.title);
    if t.len() >= 20 {
        v.push(format!("title:{t}"));
    }
    v
}

/// Merge `b` into `a` (a wins where both have a value, except where b's is clearly better).
pub fn merge_into(a: &mut Paper, b: Paper) {
    if a.doi.is_none() {
        a.doi = b.doi;
    }
    if a.title.trim().is_empty() {
        a.title = b.title;
    }
    match (&a.abstract_text, &b.abstract_text) {
        (None, Some(_)) => a.abstract_text = b.abstract_text,
        (Some(x), Some(y)) if y.len() > x.len() + 40 => a.abstract_text = b.abstract_text,
        _ => {}
    }
    if b.authors.len() > a.authors.len() {
        a.authors = b.authors;
    }
    a.year = a.year.or(b.year);
    a.date = a.date.or(b.date);
    if a.venue.as_deref().is_none_or(str::is_empty) {
        a.venue = b.venue;
    }
    a.publisher = a.publisher.take().or(b.publisher);
    a.url = a.url.take().or(b.url);
    if a.oa_url.is_none() && b.oa_url.is_some() {
        a.oa_url = b.oa_url;
        a.oa_version = b.oa_version.clone().or(a.oa_version.take());
        a.oa_licence = b.oa_licence.clone().or(a.oa_licence.take());
    }
    a.oa_status = a.oa_status.take().or(b.oa_status);
    a.oa_version = a.oa_version.take().or(b.oa_version);
    a.oa_licence = a.oa_licence.take().or(b.oa_licence);
    a.cited_by_count = match (a.cited_by_count, b.cited_by_count) {
        (Some(x), Some(y)) => Some(x.max(y)),
        (x, y) => x.or(y),
    };
    for s in b.subjects {
        if !a.subjects.iter().any(|x| x.eq_ignore_ascii_case(&s)) {
            a.subjects.push(s);
        }
    }
    a.openalex_id = a.openalex_id.take().or(b.openalex_id);
    a.s2_id = a.s2_id.take().or(b.s2_id);
    a.arxiv_id = a.arxiv_id.take().or(b.arxiv_id);
    a.pmcid = a.pmcid.take().or(b.pmcid);
    for f in b.found_in {
        if !a.found_in.contains(&f) {
            a.found_in.push(f);
        }
    }
    a.image = a.image.take().or(b.image);
    a.language = a.language.take().or(b.language);
    finalize(a);
}

/// Recompute derived fields: arXiv id from DOI/URL, free-text link for arXiv, kind and key.
pub fn finalize(p: &mut Paper) {
    if p.arxiv_id.is_none() {
        p.arxiv_id = p.doi.as_deref().and_then(arxiv_from_doi_or_url).or_else(|| p.url.as_deref().and_then(arxiv_from_doi_or_url));
    }
    if let Some(a) = &p.arxiv_id {
        let a = clean_arxiv_id(a);
        if p.oa_url.is_none() {
            p.oa_url = Some(format!("https://arxiv.org/abs/{a}"));
            p.oa_status.get_or_insert_with(|| "green".into());
            p.oa_version.get_or_insert_with(|| "submittedVersion".into());
        }
        p.arxiv_id = Some(a);
    }
    if p.kind != ContentKind::News {
        p.kind = if p.oa_url.is_some() { ContentKind::FreeFullText } else { ContentKind::AbstractOnly };
    }
    p.title = p.title.trim().to_string();
    p.key = p.make_key();
}

/// Deduplicate, merging duplicates into the first occurrence (order preserved).
pub fn dedupe(papers: Vec<Paper>) -> Vec<Paper> {
    let mut out: Vec<Paper> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    for mut p in papers {
        finalize(&mut p);
        let pids = ids(&p);
        let hit = pids.iter().find_map(|id| index.get(id).copied()).filter(|&i| {
            // A title match alone also needs compatible years (reprints / errata share titles).
            let other = &out[i];
            let only_title = !pids.iter().filter(|id| !id.starts_with("title:")).any(|id| index.get(id) == Some(&i));
            !only_title || match (other.year, p.year) {
                (Some(x), Some(y)) => (x - y).abs() <= 1,
                _ => true,
            }
        });
        match hit {
            Some(i) => {
                merge_into(&mut out[i], p);
                for id in ids(&out[i]) {
                    index.entry(id).or_insert(i);
                }
            }
            None => {
                let i = out.len();
                for id in pids {
                    index.entry(id).or_insert(i);
                }
                out.push(p);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(title: &str, doi: Option<&str>, arxiv: Option<&str>, src: &str) -> Paper {
        Paper {
            title: title.into(),
            doi: doi.map(Into::into),
            arxiv_id: arxiv.map(Into::into),
            found_in: vec![src.into()],
            year: Some(2023),
            ..Default::default()
        }
    }

    #[test]
    fn research_dedupe_merges() {
        let mut a = p("Base editing in human cells: a review", Some("https://doi.org/10.1000/XYZ"), None, "openalex");
        a.abstract_text = Some("short".into());
        let mut b = p("Base Editing in Human Cells — A Review", Some("10.1000/xyz"), None, "semanticscholar");
        b.abstract_text = Some("a much longer abstract that is definitely more than forty characters long".into());
        b.cited_by_count = Some(10);
        let c = p("Some arXiv preprint about graphs and things", None, Some("2401.01234v2"), "arxiv");
        let d = p("Some arXiv preprint about graphs and things", Some("10.48550/arXiv.2401.01234"), None, "crossref");
        let e = p("Base editing in human cells: a review", None, None, "core");
        let out = dedupe(vec![a, b, c, d, e]);
        assert_eq!(out.len(), 2, "{out:#?}");
        assert_eq!(out[0].key, "doi:10.1000/xyz");
        assert!(out[0].abstract_text.as_deref().is_some_and(|s| s.starts_with("a much longer")));
        assert_eq!(out[0].cited_by_count, Some(10));
        assert_eq!(out[0].found_in, vec!["openalex", "semanticscholar", "core"]);
        assert_eq!(out[1].arxiv_id.as_deref(), Some("2401.01234"));
        assert_eq!(out[1].kind, ContentKind::FreeFullText);
        assert_eq!(out[0].kind, ContentKind::AbstractOnly);
        assert_eq!(clean_arxiv_id("hep-th/9901001v3"), "hep-th/9901001");
        assert_eq!(arxiv_from_doi_or_url("https://arxiv.org/pdf/2401.01234v1.pdf").as_deref(), Some("2401.01234"));
    }

    #[test]
    fn research_dedupe_title_year_guard() {
        let mut a = p("Erratum to a long paper title about something", None, None, "x");
        a.year = Some(2001);
        let mut b = p("Erratum to a long paper title about something", None, None, "y");
        b.year = Some(2015);
        assert_eq!(dedupe(vec![a, b]).len(), 2);
    }
}
