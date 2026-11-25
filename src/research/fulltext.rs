//! Finding and reading a legal free full text: arXiv HTML (or ar5iv / PDF), Europe PMC JATS,
//! Unpaywall's best OA location (HTML or PDF), or a known OA URL. Nothing behind a login.

use std::sync::LazyLock;
use std::time::Duration;

use regex::Regex;

use crate::model::{FullText, FullTextSection, ImageRef, Paper};

use super::Research;
use super::text::{html_sections, pdf_text_sections, strip_bracket_citations, strip_tags, word_count};
use super::wikimedia::{is_free_licence, licence_label};

/// Minimum words for a fetched document to count as the full text (not just a landing page).
const MIN_FULLTEXT_WORDS: usize = 1200;

/// A full text plus any figures that came with it.
#[derive(Clone, Debug, Default)]
pub struct FoundText {
    pub text: FullText,
    pub figures: Vec<ImageRef>,
}

fn clean_sections(mut secs: Vec<FullTextSection>) -> Vec<FullTextSection> {
    for s in &mut secs {
        for p in &mut s.paragraphs {
            *p = strip_bracket_citations(p);
        }
        s.paragraphs.retain(|p| p.split_whitespace().count() >= 5);
    }
    secs.retain(|s| !s.paragraphs.is_empty());
    secs
}

/// Extract text from PDF bytes (off the async runtime; panics in the parser are contained).
pub async fn pdf_to_text(bytes: Vec<u8>) -> Option<String> {
    let job = tokio::task::spawn_blocking(move || pdf_extract::extract_text_from_mem(&bytes).ok());
    match tokio::time::timeout(Duration::from_secs(40), job).await {
        Ok(Ok(Some(t))) if t.split_whitespace().count() > 200 => Some(t),
        Ok(Err(e)) => {
            log::warn!("PDF extraction panicked: {e}");
            None
        }
        _ => None,
    }
}

/// Figures in an arXiv/LaTeXML HTML page: (absolute image url, caption).
pub fn arxiv_html_figures(html: &str, base: &str) -> Vec<(String, String)> {
    static RE_FIG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?is)<figure\b[^>]*ltx_figure[^>]*>(.*?)</figure>").unwrap());
    static RE_IMG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?is)<img\b[^>]*?src="([^"]+)""#).unwrap());
    static RE_CAP: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?is)<figcaption\b[^>]*>(.*?)</figcaption>").unwrap());
    let base_url = url::Url::parse(base).ok();
    RE_FIG
        .captures_iter(html)
        .filter_map(|c| {
            let inner = c.get(1)?.as_str();
            let src = RE_IMG.captures(inner)?.get(1)?.as_str();
            let abs = match &base_url {
                Some(b) => b.join(src).ok()?.to_string(),
                None => src.to_string(),
            };
            let cap = RE_CAP.captures(inner).and_then(|c| c.get(1)).map(|m| strip_tags(m.as_str())).unwrap_or_default();
            Some((abs, cap))
        })
        .take(6)
        .collect()
}

impl Research {
    /// Read a document at `url` (HTML or PDF) into sections; None if it is not a full text.
    pub(crate) async fn read_document(&self, url: &str) -> Option<(Vec<FullTextSection>, String)> {
        let bytes = match self.get_page(url).await {
            Ok(b) => b,
            Err(e) => {
                log::warn!("full text fetch failed for {url}: {e:#}");
                return None;
            }
        };
        let secs = if bytes.starts_with(b"%PDF") {
            pdf_text_sections(&pdf_to_text(bytes).await?)
        } else {
            let html = String::from_utf8_lossy(&bytes).into_owned();
            html_sections(&html)
        };
        let secs = clean_sections(secs);
        let wc = word_count(&secs);
        if wc < MIN_FULLTEXT_WORDS {
            log::debug!("{url}: only {wc} words, not a full text");
            return None;
        }
        Some((secs, url.to_string()))
    }

    /// Find and read the target's legal free full text, trying the cheapest good sources first.
    pub(crate) async fn find_full_text(&self, paper: &Paper) -> Option<FoundText> {
        // 1. arXiv: LaTeXML HTML (new papers), ar5iv (older ones), then the PDF.
        if let Some(id) = &paper.arxiv_id {
            let lic = paper.oa_licence.clone();
            for url in [format!("https://arxiv.org/html/{id}"), format!("https://ar5iv.labs.arxiv.org/html/{id}")] {
                let Ok(bytes) = self.get_page(&url).await else { continue };
                let html = String::from_utf8_lossy(&bytes).into_owned();
                let secs = clean_sections(html_sections(&html));
                if word_count(&secs) < MIN_FULLTEXT_WORDS {
                    continue;
                }
                let free = lic.as_deref().is_some_and(is_free_licence);
                let base = format!("{url}/");
                let figures = arxiv_html_figures(&html, &base)
                    .into_iter()
                    .map(|(img, cap)| ImageRef {
                        url: img,
                        thumb_url: None,
                        page_url: format!("https://arxiv.org/abs/{id}"),
                        title: cap.chars().take(200).collect(),
                        credit: format!("Figure from {} (arXiv:{id})", paper.author_line(2)),
                        licence: lic.as_deref().map(licence_label).unwrap_or_else(|| "arXiv licence (see abstract page)".into()),
                        licence_url: None,
                        from_paper: true,
                        embeddable: free,
                    })
                    .collect();
                return Some(FoundText {
                    text: FullText { url, licence: lic.clone(), version: Some("submittedVersion".into()), sections: secs },
                    figures,
                });
            }
            if let Some((secs, url)) = self.read_document(&format!("https://arxiv.org/pdf/{id}")).await {
                return Some(FoundText { text: FullText { url, licence: lic, version: Some("submittedVersion".into()), sections: secs }, figures: vec![] });
            }
        }

        // 2. Europe PMC open-access JATS.
        let mut pmcid = paper.pmcid.clone();
        if pmcid.is_none() {
            if let Some(doi) = &paper.doi {
                pmcid = self.europepmc_pmcid(doi).await.unwrap_or_else(|e| {
                    log::warn!("Europe PMC lookup failed: {e:#}");
                    None
                });
            }
        }
        if let Some(pmc) = &pmcid {
            match self.europepmc_fulltext(pmc).await {
                Ok(Some(ft)) if word_count(&ft.sections) >= MIN_FULLTEXT_WORDS / 2 => {
                    let figures = self.resolve_pmc_figures(pmc, ft.figures).await;
                    return Some(FoundText {
                        text: FullText {
                            url: format!("https://europepmc.org/article/PMC/{}", pmc.trim_start_matches("PMC")),
                            licence: ft.licence.as_deref().map(licence_label),
                            version: Some("publishedVersion".into()),
                            sections: clean_sections(ft.sections),
                        },
                        figures,
                    });
                }
                Ok(_) => {}
                Err(e) => log::warn!("Europe PMC full text failed for {pmc}: {e:#}"),
            }
        }

        // 3. Unpaywall locations (HTML landing pages of OA articles, then PDFs, then repositories).
        let mut candidates: Vec<(String, Option<String>, Option<String>)> = Vec::new();
        if let Some(doi) = &paper.doi {
            match self.unpaywall(doi).await {
                Ok(Some(loc)) if loc.is_oa => {
                    for u in [loc.landing_url.clone(), loc.pdf_url.clone()].into_iter().flatten().chain(loc.alternates.iter().take(3).cloned()) {
                        candidates.push((u, loc.licence.clone(), loc.version.clone()));
                    }
                }
                Ok(_) => {}
                Err(e) => log::warn!("Unpaywall failed for {doi}: {e:#}"),
            }
        }
        if let Some(u) = &paper.oa_url {
            candidates.push((u.clone(), paper.oa_licence.clone(), paper.oa_version.clone()));
        }
        let mut seen = std::collections::HashSet::new();
        candidates.retain(|c| seen.insert(c.0.clone()));
        for (url, lic, ver) in candidates.into_iter().take(5) {
            if let Some((secs, url)) = self.read_document(&url).await {
                return Some(FoundText { text: FullText { url, licence: lic.as_deref().map(licence_label), version: ver, sections: secs }, figures: vec![] });
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn research_arxiv_figures() {
        let html = r#"<figure id="S1.F1" class="ltx_figure"><img src="x1.png" class="ltx_graphics" alt=""/><figcaption class="ltx_caption">Figure 1: A diagram.</figcaption></figure>"#;
        let f = arxiv_html_figures(html, "https://arxiv.org/html/2401.01234/");
        assert_eq!(f, vec![("https://arxiv.org/html/2401.01234/x1.png".to_string(), "Figure 1: A diagram.".to_string())]);
    }
}
