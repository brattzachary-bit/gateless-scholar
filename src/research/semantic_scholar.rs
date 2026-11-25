//! Semantic Scholar Graph API: search, paper lookup and citation contexts.

use anyhow::Result;
use chrono::NaiveDate;
use serde_json::Value;

use crate::model::Paper;

use super::Research;
use super::dedupe::finalize;
use super::http::{Get, TTL_META, TTL_SEARCH, enc};
use super::text::strip_tags;

const BASE: &str = "https://api.semanticscholar.org/graph/v1";
const FIELDS: &str = "title,abstract,authors,year,venue,externalIds,url,citationCount,openAccessPdf,publicationDate,fieldsOfStudy,isOpenAccess";

/// A paper citing the target, with the sentences in which it cites it.
#[derive(Clone, Debug, Default)]
pub struct CitingContext {
    pub paper: Paper,
    pub contexts: Vec<String>,
}

fn s(v: &Value) -> Option<String> {
    v.as_str().map(str::trim).filter(|s| !s.is_empty()).map(String::from)
}

pub fn parse_paper(v: &Value) -> Option<Paper> {
    let title = strip_tags(&s(&v["title"])?);
    if title.is_empty() {
        return None;
    }
    let ext = &v["externalIds"];
    let mut p = Paper {
        title,
        s2_id: s(&v["paperId"]),
        doi: s(&ext["DOI"]).map(|d| crate::model::normalize_doi(&d)),
        arxiv_id: s(&ext["ArXiv"]),
        pmcid: s(&ext["PubMedCentral"]).map(|p| if p.starts_with("PMC") { p } else { format!("PMC{p}") }),
        abstract_text: s(&v["abstract"]).map(|a| strip_tags(&a)),
        year: v["year"].as_i64().map(|y| y as i32),
        date: s(&v["publicationDate"]).and_then(|d| NaiveDate::parse_from_str(&d, "%Y-%m-%d").ok()),
        venue: s(&v["venue"]),
        url: s(&v["url"]),
        cited_by_count: v["citationCount"].as_u64().map(|c| c.min(u32::MAX as u64) as u32),
        found_in: vec!["semanticscholar".into()],
        ..Default::default()
    };
    p.authors = v["authors"].as_array().into_iter().flatten().filter_map(|a| s(&a["name"])).collect();
    p.subjects = v["fieldsOfStudy"].as_array().into_iter().flatten().filter_map(s).collect();
    if let Some(pdf) = s(&v["openAccessPdf"]["url"]) {
        p.oa_url = Some(pdf);
        p.oa_status = s(&v["openAccessPdf"]["status"]).map(|s| s.to_lowercase());
        p.oa_licence = s(&v["openAccessPdf"]["license"]);
    }
    // A DOI-carrying record's landing page is the DOI, which is more stable than the S2 page.
    if let Some(d) = &p.doi {
        p.url = Some(format!("https://doi.org/{d}"));
    }
    finalize(&mut p);
    Some(p)
}

impl Research {
    fn s2_get(&self) -> Get<'static> {
        let mut g = Get { ttl: Some(TTL_SEARCH), retries: 2, ..Default::default() };
        if let Some(k) = self.config.s2_api_key.as_ref().filter(|k| !k.trim().is_empty()) {
            g.headers.push(("x-api-key", k.trim().to_string()));
        }
        g
    }

    pub(crate) async fn s2_search(&self, query: &str, offset: u32, limit: u32, years: Option<(Option<i32>, Option<i32>)>, oa_only: bool) -> Result<Vec<Paper>> {
        let mut url = format!("{BASE}/paper/search?query={}&offset={offset}&limit={}&fields={FIELDS}", enc(query), limit.clamp(1, 100));
        if let Some((from, to)) = years {
            if from.is_some() || to.is_some() {
                url.push_str(&format!("&year={}-{}", from.map(|y| y.to_string()).unwrap_or_default(), to.map(|y| y.to_string()).unwrap_or_default()));
            }
        }
        if oa_only {
            url.push_str("&openAccessPdf");
        }
        let v = self.get_json_with(&url, self.s2_get()).await?;
        Ok(v["data"].as_array().into_iter().flatten().filter_map(parse_paper).collect())
    }

    /// The S2 identifier string for a paper: DOI:…, ARXIV:…, PMCID or the S2 id.
    pub(crate) fn s2_ref(p: &Paper) -> Option<String> {
        if let Some(id) = &p.s2_id {
            return Some(id.clone());
        }
        if let Some(d) = &p.doi {
            return Some(format!("DOI:{}", crate::model::normalize_doi(d)));
        }
        if let Some(a) = &p.arxiv_id {
            return Some(format!("ARXIV:{a}"));
        }
        None
    }

    pub(crate) async fn s2_paper(&self, id: &str) -> Result<Option<Paper>> {
        let url = format!("{BASE}/paper/{}?fields={FIELDS}", enc(id).replace("%3A", ":").replace("%2F", "/"));
        let mut g = self.s2_get();
        g.ttl = Some(TTL_META);
        let v = self.get_json_with(&url, g).await?;
        Ok(parse_paper(&v))
    }

    /// Best title match (S2 `/paper/search/match`).
    pub(crate) async fn s2_match_title(&self, title: &str) -> Result<Option<Paper>> {
        let url = format!("{BASE}/paper/search/match?query={}&fields={FIELDS}", enc(title));
        let mut g = self.s2_get();
        g.ttl = Some(TTL_META);
        let v = self.get_json_with(&url, g).await?;
        Ok(v["data"].as_array().and_then(|a| a.first()).and_then(parse_paper))
    }

    /// Papers citing `id`, with the citation-context sentences.
    pub(crate) async fn s2_citations(&self, id: &str, limit: u32) -> Result<Vec<CitingContext>> {
        let url = format!(
            "{BASE}/paper/{}/citations?fields=contexts,title,authors,year,venue,externalIds,url,citationCount,publicationDate&limit={}",
            enc(id).replace("%3A", ":").replace("%2F", "/"),
            limit.clamp(1, 1000)
        );
        let mut g = self.s2_get();
        g.ttl = Some(TTL_META);
        let v = self.get_json_with(&url, g).await?;
        Ok(v["data"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|c| {
                let paper = parse_paper(&c["citingPaper"])?;
                let contexts = c["contexts"].as_array().into_iter().flatten().filter_map(s).map(|t| strip_tags(&t)).collect();
                Some(CitingContext { paper, contexts })
            })
            .collect())
    }
}
