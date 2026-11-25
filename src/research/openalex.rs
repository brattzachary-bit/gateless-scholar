//! OpenAlex works search (https://docs.openalex.org).

use anyhow::Result;
use chrono::NaiveDate;
use serde_json::Value;

use crate::model::{Paper, SortBy};

use super::Research;
use super::dedupe::{arxiv_from_doi_or_url, finalize};
use super::http::{TTL_META, TTL_SEARCH, enc};
use super::text::strip_tags;

const BASE: &str = "https://api.openalex.org";
const SELECT: &str = "id,doi,display_name,publication_year,publication_date,authorships,primary_location,best_oa_location,locations,open_access,cited_by_count,abstract_inverted_index,ids,topics,language,type";

/// Rebuild an abstract from OpenAlex's `abstract_inverted_index` ({word: [positions]}).
pub fn reconstruct_abstract(inv: &Value) -> Option<String> {
    let obj = inv.as_object()?;
    let mut slots: Vec<(u64, &str)> = Vec::new();
    for (word, positions) in obj {
        for pos in positions.as_array().into_iter().flatten() {
            if let Some(p) = pos.as_u64() {
                slots.push((p, word.as_str()));
            }
        }
    }
    if slots.is_empty() {
        return None;
    }
    slots.sort_by_key(|s| s.0);
    let text = slots.iter().map(|s| s.1).collect::<Vec<_>>().join(" ");
    let text = strip_tags(&text);
    let text = text.trim_start_matches("Abstract ").trim_start_matches("ABSTRACT ").to_string();
    (text.split_whitespace().count() >= 15).then_some(text)
}

fn s(v: &Value) -> Option<String> {
    v.as_str().map(str::trim).filter(|s| !s.is_empty()).map(String::from)
}

pub fn parse_work(w: &Value) -> Option<Paper> {
    let title = strip_tags(&s(&w["display_name"]).or_else(|| s(&w["title"]))?);
    if title.is_empty() {
        return None;
    }
    let mut p = Paper {
        title,
        doi: s(&w["doi"]).map(|d| crate::model::normalize_doi(&d)),
        abstract_text: reconstruct_abstract(&w["abstract_inverted_index"]),
        year: w["publication_year"].as_i64().map(|y| y as i32),
        date: s(&w["publication_date"]).and_then(|d| NaiveDate::parse_from_str(&d, "%Y-%m-%d").ok()),
        cited_by_count: w["cited_by_count"].as_u64().map(|c| c.min(u32::MAX as u64) as u32),
        openalex_id: s(&w["id"]).map(|i| i.rsplit('/').next().unwrap_or(&i).to_string()),
        language: s(&w["language"]),
        found_in: vec!["openalex".into()],
        ..Default::default()
    };
    p.authors = w["authorships"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|a| s(&a["author"]["display_name"]).or_else(|| s(&a["raw_author_name"])))
        .collect();
    let primary = &w["primary_location"];
    p.venue = s(&primary["source"]["display_name"]);
    p.publisher = s(&primary["source"]["host_organization_name"]);
    p.url = s(&primary["landing_page_url"]).or_else(|| p.doi.as_ref().map(|d| format!("https://doi.org/{d}")));
    if let Some(pmcid) = s(&w["ids"]["pmcid"]) {
        let id = pmcid.trim_end_matches('/').rsplit('/').next().unwrap_or(&pmcid).to_uppercase();
        p.pmcid = Some(if id.starts_with("PMC") { id } else { format!("PMC{id}") });
    }
    // arXiv id from any location.
    for loc in std::iter::once(primary).chain(w["locations"].as_array().into_iter().flatten()) {
        if p.arxiv_id.is_some() {
            break;
        }
        p.arxiv_id = s(&loc["landing_page_url"]).and_then(|u| arxiv_from_doi_or_url(&u)).or_else(|| s(&loc["pdf_url"]).and_then(|u| arxiv_from_doi_or_url(&u)));
    }
    let oa = &w["open_access"];
    let best = &w["best_oa_location"];
    if oa["is_oa"].as_bool() == Some(true) {
        p.oa_url = s(&best["pdf_url"]).or_else(|| s(&best["landing_page_url"])).or_else(|| s(&oa["oa_url"]));
        p.oa_status = s(&oa["oa_status"]);
        p.oa_version = s(&best["version"]);
        p.oa_licence = s(&best["license"]);
    } else {
        p.oa_status = s(&oa["oa_status"]);
    }
    p.subjects = w["topics"]
        .as_array()
        .into_iter()
        .flatten()
        .take(3)
        .flat_map(|t| [s(&t["display_name"]), s(&t["field"]["display_name"])])
        .flatten()
        .fold(Vec::new(), |mut acc: Vec<String>, x| {
            if !acc.contains(&x) {
                acc.push(x);
            }
            acc
        });
    finalize(&mut p);
    Some(p)
}

pub struct OaSearch<'a> {
    pub query: &'a str,
    pub from: Option<NaiveDate>,
    pub to: Option<NaiveDate>,
    pub oa_only: bool,
    pub need_abstract: bool,
    pub sort: SortBy,
    pub page: u32,
    pub per_page: u32,
    /// Extra raw filter terms, e.g. "author.id:A123".
    pub extra_filter: Option<String>,
}

impl Default for OaSearch<'_> {
    fn default() -> Self {
        OaSearch { query: "", from: None, to: None, oa_only: false, need_abstract: false, sort: SortBy::Relevance, page: 0, per_page: 20, extra_filter: None }
    }
}

/// OpenAlex API key: env `OPENALEX_API_KEY`, else the keyring entry "openalex-api-key".
/// Without one, OpenAlex gives a small shared daily budget per IP (about 100 searches).
static OPENALEX_KEY: tokio::sync::OnceCell<Option<String>> = tokio::sync::OnceCell::const_new();

async fn openalex_key() -> Option<String> {
    OPENALEX_KEY
        .get_or_init(|| async {
            if let Ok(k) = std::env::var("OPENALEX_API_KEY") {
                if !k.trim().is_empty() {
                    return Some(k.trim().to_string());
                }
            }
            tokio::task::spawn_blocking(|| crate::providers::secrets::get_api_key("openalex-api-key").ok().flatten())
                .await
                .ok()
                .flatten()
                .map(|k| k.trim().to_string())
                .filter(|k| !k.is_empty())
        })
        .await
        .clone()
}

impl Research {
    /// `&mailto=…` (polite pool) and `&api_key=…` when configured. Not part of the cache key.
    async fn openalex_suffix(&self) -> String {
        let mut s = self.contact_email().map(|e| format!("&mailto={}", enc(e))).unwrap_or_default();
        if let Some(k) = openalex_key().await {
            s.push_str(&format!("&api_key={}", enc(&k)));
        }
        s
    }

    /// GET an OpenAlex URL, caching under the URL without credentials.
    async fn openalex_get(&self, url: &str, ttl: std::time::Duration) -> Result<Value> {
        let suffix = self.openalex_suffix().await;
        if let Some((body, fetched)) = self.db.cache_get(url) {
            if (chrono::Utc::now() - fetched).to_std().unwrap_or_default() < ttl {
                if let Ok(v) = serde_json::from_slice(&body) {
                    return Ok(v);
                }
            }
        }
        let full = format!("{url}{suffix}");
        match self.get_json_with(&full, super::http::Get { ttl: None, retries: 2, ..Default::default() }).await {
            Ok(v) => {
                if let Ok(b) = serde_json::to_vec(&v) {
                    let _ = self.db.cache_put(url, &b);
                }
                Ok(v)
            }
            Err(e) => match self.db.cache_get(url).and_then(|(b, _)| serde_json::from_slice(&b).ok()) {
                Some(v) => {
                    log::warn!("OpenAlex failed ({e:#}); using stale cache");
                    Ok(v)
                }
                None => Err(e),
            },
        }
    }

    pub(crate) async fn openalex_search(&self, q: &OaSearch<'_>) -> Result<Vec<Paper>> {
        let mut filters = vec!["is_paratext:false".to_string(), "is_retracted:false".to_string()];
        if let Some(f) = q.from {
            filters.push(format!("from_publication_date:{f}"));
        }
        if let Some(t) = q.to {
            filters.push(format!("to_publication_date:{t}"));
        }
        if q.oa_only {
            filters.push("is_oa:true".into());
        }
        if q.need_abstract {
            filters.push("has_abstract:true".into());
        }
        if let Some(x) = &q.extra_filter {
            filters.push(x.clone());
        }
        let sort = match q.sort {
            SortBy::Relevance if !q.query.trim().is_empty() => "",
            SortBy::Relevance | SortBy::Newest => "&sort=publication_date:desc",
            SortBy::MostCited => "&sort=cited_by_count:desc",
        };
        // Relevance ranking uses `search` (title, abstract and full text); date / citation
        // sorts use the stricter title+abstract filter so the top hits are actually on topic.
        let search = if q.query.trim().is_empty() {
            String::new()
        } else if q.sort == SortBy::Relevance {
            format!("&search={}", enc(q.query.trim()))
        } else {
            filters.push(format!("title_and_abstract.search:{}", q.query.replace([',', ':'], " ").trim()));
            String::new()
        };
        let url = format!(
            "{BASE}/works?filter={}{search}{sort}&per-page={}&page={}&select={SELECT}",
            enc(&filters.join(",")),
            q.per_page.clamp(1, 200),
            q.page + 1,
        );
        let v = self.openalex_get(&url, TTL_SEARCH).await?;
        Ok(v["results"].as_array().into_iter().flatten().filter_map(parse_work).collect())
    }

    /// One work by DOI (or OpenAlex id "W123").
    pub(crate) async fn openalex_work(&self, id: &str) -> Result<Option<Paper>> {
        let path = if id.starts_with('W') {
            id.to_string()
        } else {
            let doi = crate::model::normalize_doi(id);
            // Keep '/' readable; escape only what would break the path.
            format!("https://doi.org/{}", doi.replace('%', "%25").replace('?', "%3F").replace('#', "%23").replace(' ', "%20"))
        };
        let url = format!("{BASE}/works/{path}?select={SELECT}");
        let v = self.openalex_get(&url, TTL_META).await?;
        Ok(parse_work(&v))
    }

    /// First author id matching a name.
    pub(crate) async fn openalex_author_id(&self, name: &str) -> Result<Option<String>> {
        let url = format!("{BASE}/authors?search={}&per-page=1&select=id,display_name,works_count", enc(name));
        let v = self.openalex_get(&url, TTL_META).await?;
        Ok(v["results"][0]["id"].as_str().map(|i| i.rsplit('/').next().unwrap_or(i).to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn research_openalex_inverted_index() {
        let inv = serde_json::json!({
            "Despite": [0], "growing": [1], "interest": [2], "in": [3, 57], "Open": [4], "Access": [5],
            "the": [6, 8, 10, 12, 14, 16, 18], "x": [7, 9, 11, 13, 15, 17]
        });
        let a = reconstruct_abstract(&inv).unwrap_or_default();
        assert!(a.starts_with("Despite growing interest in Open Access the x the x"));
        assert!(a.ends_with(" in"));
        assert_eq!(reconstruct_abstract(&serde_json::json!(null)), None);
        let w = serde_json::json!({
            "id": "https://openalex.org/W1", "doi": "https://doi.org/10.1/ABC", "display_name": "A <i>title</i>",
            "publication_year": 2024, "publication_date": "2024-02-03",
            "authorships": [{"author": {"display_name": "Ada Lovelace"}}],
            "primary_location": {"landing_page_url": "https://doi.org/10.1/abc", "source": {"display_name": "Nature"}},
            "locations": [{"landing_page_url": "https://arxiv.org/abs/2402.00001v1"}],
            "open_access": {"is_oa": false, "oa_status": "closed"},
            "ids": {"pmcid": "https://www.ncbi.nlm.nih.gov/pmc/articles/pmc123"}
        });
        let p = parse_work(&w).unwrap_or_default();
        assert_eq!(p.title, "A title");
        assert_eq!(p.key, "doi:10.1/abc");
        assert_eq!(p.arxiv_id.as_deref(), Some("2402.00001"));
        assert_eq!(p.pmcid.as_deref(), Some("PMC123"));
        assert_eq!(p.kind, crate::model::ContentKind::FreeFullText);
    }
}
