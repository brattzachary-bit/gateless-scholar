//! Crossref works search, Unpaywall OA lookup, CORE search and Europe PMC (OA full text + figures).

use anyhow::Result;
use chrono::NaiveDate;
use serde_json::Value;

use crate::model::{FullTextSection, ImageRef, Paper, SortBy};

use super::Research;
use super::dedupe::finalize;
use super::http::{Get, TTL_META, TTL_SEARCH, enc};
use super::text::strip_tags;

fn s(v: &Value) -> Option<String> {
    v.as_str().map(str::trim).filter(|s| !s.is_empty()).map(String::from)
}

// ------------------------------------------------------------------------------------ Crossref

fn crossref_date(v: &Value) -> (Option<i32>, Option<NaiveDate>) {
    let parts = &v["date-parts"][0];
    let y = parts[0].as_i64().map(|y| y as i32);
    let d = match (y, parts[1].as_u64(), parts[2].as_u64()) {
        (Some(y), Some(m), Some(d)) => NaiveDate::from_ymd_opt(y, m as u32, d as u32),
        (Some(y), Some(m), None) => NaiveDate::from_ymd_opt(y, m as u32, 1),
        _ => None,
    };
    (y, d)
}

pub fn parse_crossref(w: &Value) -> Option<Paper> {
    let title = strip_tags(&s(&w["title"][0])?);
    if title.is_empty() {
        return None;
    }
    let (year, date) = [&w["published"], &w["published-online"], &w["published-print"], &w["issued"]]
        .into_iter()
        .map(crossref_date)
        .find(|(y, _)| y.is_some())
        .unwrap_or((None, None));
    let mut p = Paper {
        title,
        doi: s(&w["DOI"]).map(|d| crate::model::normalize_doi(&d)),
        abstract_text: s(&w["abstract"]).map(|a| strip_tags(&a)).map(|a| a.trim_start_matches("Abstract").trim().to_string()).filter(|a| a.len() > 40),
        year,
        date,
        venue: s(&w["container-title"][0]),
        publisher: s(&w["publisher"]),
        url: s(&w["URL"]),
        cited_by_count: w["is-referenced-by-count"].as_u64().map(|c| c.min(u32::MAX as u64) as u32),
        found_in: vec!["crossref".into()],
        ..Default::default()
    };
    p.authors = w["author"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|a| match (s(&a["given"]), s(&a["family"])) {
            (Some(g), Some(f)) => Some(format!("{g} {f}")),
            (None, Some(f)) => Some(f),
            _ => s(&a["name"]),
        })
        .collect();
    p.subjects = w["subject"].as_array().into_iter().flatten().filter_map(s).take(4).collect();
    // A Creative Commons licence plus a full-text link means a legal free copy.
    let cc = w["license"].as_array().into_iter().flatten().filter_map(|l| s(&l["URL"])).find(|u| u.contains("creativecommons.org"));
    if let Some(lic) = cc {
        if let Some(link) = w["link"].as_array().into_iter().flatten().filter_map(|l| s(&l["URL"])).next() {
            p.oa_url = Some(link);
            p.oa_licence = Some(licence_from_url(&lic));
            p.oa_status = Some("gold".into());
            p.oa_version = Some("publishedVersion".into());
        }
    }
    finalize(&mut p);
    Some(p)
}

/// Licence from a prose licence statement ("…Creative Commons Attribution License…").
pub fn licence_from_text(t: &str) -> Option<String> {
    let l = t.to_lowercase();
    if let Some(i) = l.find("creativecommons.org/") {
        let url: String = l[i..].chars().take_while(|c| !c.is_whitespace() && *c != '"' && *c != ')').collect();
        return Some(format!("https://{url}"));
    }
    if l.contains("cc0") || l.contains("public domain dedication") {
        return Some("cc0".into());
    }
    if l.contains("creative commons attribution") || l.contains("cc by") || l.contains("cc-by") {
        let mut kind = String::from("cc-by");
        if l.contains("noncommercial") || l.contains("non-commercial") {
            kind.push_str("-nc");
        }
        if l.contains("noderiv") || l.contains("no derivatives") {
            kind.push_str("-nd");
        }
        if l.contains("sharealike") || l.contains("share alike") {
            kind.push_str("-sa");
        }
        return Some(kind);
    }
    None
}

fn licence_from_url(u: &str) -> String {
    let l = u.to_lowercase();
    if l.contains("publicdomain/zero") {
        return "cc0".into();
    }
    if let Some(rest) = l.split("/licenses/").nth(1) {
        let kind = rest.split('/').next().unwrap_or("");
        return format!("cc-{kind}");
    }
    u.to_string()
}

impl Research {
    pub(crate) async fn crossref_search(&self, query: &str, offset: u32, rows: u32, years: (Option<i32>, Option<i32>), sort: SortBy, need_abstract: bool) -> Result<Vec<Paper>> {
        let mut filters = vec!["type:journal-article".to_string()];
        if let Some(f) = years.0 {
            filters.push(format!("from-pub-date:{f}"));
        }
        if let Some(t) = years.1 {
            filters.push(format!("until-pub-date:{t}"));
        }
        if need_abstract {
            filters.push("has-abstract:true".into());
        }
        let sort = match sort {
            SortBy::Relevance => "",
            SortBy::Newest => "&sort=published&order=desc",
            SortBy::MostCited => "&sort=is-referenced-by-count&order=desc",
        };
        let mailto = self.contact_email().map(|e| format!("&mailto={}", enc(e))).unwrap_or_default();
        let url = format!(
            "https://api.crossref.org/works?query={}&rows={}&offset={offset}&filter={}{sort}&select=DOI,title,author,published,published-online,published-print,issued,container-title,publisher,abstract,is-referenced-by-count,URL,subject,license,link{mailto}",
            enc(query),
            rows.clamp(1, 100),
            enc(&filters.join(","))
        );
        let v = self.get_json(&url, TTL_SEARCH).await?;
        Ok(v["message"]["items"].as_array().into_iter().flatten().filter_map(parse_crossref).collect())
    }
}

// ----------------------------------------------------------------------------------- Unpaywall

/// Unpaywall result for one DOI.
#[derive(Clone, Debug, Default)]
pub struct OaLocation {
    pub is_oa: bool,
    pub status: Option<String>,
    pub landing_url: Option<String>,
    pub pdf_url: Option<String>,
    pub version: Option<String>,
    pub licence: Option<String>,
    /// Other OA locations (repository copies), PDF or landing URLs, best first.
    pub alternates: Vec<String>,
}

/// Placeholder used when the user has not configured a contact email (Unpaywall needs one).
pub const UNPAYWALL_PLACEHOLDER_EMAIL: &str = "unpaywall@gateless-scholar.invalid";

impl Research {
    pub(crate) async fn unpaywall(&self, doi: &str) -> Result<Option<OaLocation>> {
        let email = self.contact_email().unwrap_or(UNPAYWALL_PLACEHOLDER_EMAIL);
        let url = format!("https://api.unpaywall.org/v2/{}?email={}", crate::model::normalize_doi(doi).replace('#', "%23").replace('?', "%3F"), enc(email));
        let v = match self.get_json_with(&url, Get { ttl: Some(TTL_META), retries: 1, ..Default::default() }).await {
            Ok(v) => v,
            Err(e) if e.to_string().contains("HTTP 404") => return Ok(None),
            Err(e) => return Err(e),
        };
        let best = &v["best_oa_location"];
        let mut loc = OaLocation {
            is_oa: v["is_oa"].as_bool().unwrap_or(false),
            status: s(&v["oa_status"]),
            landing_url: s(&best["url_for_landing_page"]),
            pdf_url: s(&best["url_for_pdf"]),
            version: s(&best["version"]),
            licence: s(&best["license"]),
            alternates: vec![],
        };
        for l in v["oa_locations"].as_array().into_iter().flatten() {
            for u in [s(&l["url_for_pdf"]), s(&l["url_for_landing_page"])].into_iter().flatten() {
                if Some(&u) != loc.pdf_url.as_ref() && Some(&u) != loc.landing_url.as_ref() && !loc.alternates.contains(&u) {
                    loc.alternates.push(u);
                }
            }
        }
        Ok(Some(loc))
    }
}

// ---------------------------------------------------------------------------------------- CORE

pub fn parse_core(w: &Value) -> Option<Paper> {
    let title = strip_tags(&s(&w["title"])?);
    let mut p = Paper {
        title,
        doi: s(&w["doi"]).map(|d| crate::model::normalize_doi(&d)),
        abstract_text: s(&w["abstract"]).map(|a| strip_tags(&a)).filter(|a| a.len() > 40),
        year: w["yearPublished"].as_i64().map(|y| y as i32),
        date: s(&w["publishedDate"]).and_then(|d| NaiveDate::parse_from_str(d.get(..10).unwrap_or(&d), "%Y-%m-%d").ok()),
        venue: s(&w["journals"][0]["title"]).or_else(|| s(&w["publisher"])),
        publisher: s(&w["publisher"]),
        arxiv_id: s(&w["arxivId"]),
        cited_by_count: w["citationCount"].as_u64().map(|c| c.min(u32::MAX as u64) as u32),
        found_in: vec!["core".into()],
        ..Default::default()
    };
    p.authors = w["authors"].as_array().into_iter().flatten().filter_map(|a| s(&a["name"])).collect();
    p.url = w["sourceFulltextUrls"].as_array().into_iter().flatten().filter_map(s).next().or_else(|| s(&w["links"][0]["url"]));
    if let Some(dl) = s(&w["downloadUrl"]) {
        p.oa_url = Some(dl);
        p.oa_status = Some("green".into());
    }
    if p.doi.is_some() && p.url.is_none() {
        p.url = p.doi.as_ref().map(|d| format!("https://doi.org/{d}"));
    }
    finalize(&mut p);
    Some(p)
}

impl Research {
    pub(crate) fn core_key(&self) -> Option<String> {
        self.config.core_api_key.as_ref().map(|k| k.trim().to_string()).filter(|k| !k.is_empty())
    }

    pub(crate) async fn core_search(&self, query: &str, offset: u32, limit: u32) -> Result<Vec<Paper>> {
        let Some(key) = self.core_key() else { return Ok(vec![]) };
        let url = format!("https://api.core.ac.uk/v3/search/works?q={}&offset={offset}&limit={}", enc(query), limit.clamp(1, 50));
        let g = Get { ttl: Some(TTL_SEARCH), retries: 2, headers: vec![("Authorization", format!("Bearer {key}"))], ..Default::default() };
        let v = self.get_json_with(&url, g).await?;
        Ok(v["results"].as_array().into_iter().flatten().filter_map(parse_core).collect())
    }
}

// ---------------------------------------------------------------------------------- Europe PMC

/// Europe PMC full text parsed from JATS XML.
#[derive(Clone, Debug, Default)]
pub struct PmcFullText {
    pub sections: Vec<FullTextSection>,
    pub licence: Option<String>,
    pub figures: Vec<ImageRef>,
}

impl Research {
    /// PMCID of an open-access record for a DOI (Europe PMC search), when there is one.
    pub(crate) async fn europepmc_pmcid(&self, doi: &str) -> Result<Option<String>> {
        let q = format!("DOI:\"{}\" AND OPEN_ACCESS:Y", crate::model::normalize_doi(doi));
        let url = format!("https://www.ebi.ac.uk/europepmc/webservices/rest/search?query={}&format=json&resultType=lite&pageSize=1", enc(&q));
        let v = self.get_json(&url, TTL_META).await?;
        Ok(v["resultList"]["result"][0]["pmcid"].as_str().map(String::from))
    }

    pub(crate) async fn europepmc_fulltext(&self, pmcid: &str) -> Result<Option<PmcFullText>> {
        let url = format!("https://www.ebi.ac.uk/europepmc/webservices/rest/{}/fullTextXML", enc(pmcid));
        let get = Get { ttl: Some(TTL_META), retries: 0, timeout: Some(std::time::Duration::from_secs(40)), ..Default::default() };
        let xml = match self.get_bytes(&url, get).await.map(|b| String::from_utf8_lossy(&b).into_owned()) {
            Ok(x) => x,
            Err(e) if e.to_string().contains("HTTP 404") => return Ok(None),
            Err(e) => return Err(e),
        };
        Ok(Some(parse_jats(&xml, pmcid)))
    }
}

impl Research {
    /// Europe PMC's own figure links are bot-blocked; PMC's article page links each figure on
    /// its CDN. Map the JATS graphic names to those URLs (only open-licence figures, so this
    /// costs nothing for closed ones). Unresolvable figures are dropped.
    pub(crate) async fn resolve_pmc_figures(&self, pmcid: &str, figures: Vec<ImageRef>) -> Vec<ImageRef> {
        let figures: Vec<ImageRef> = figures.into_iter().filter(|f| f.embeddable).collect();
        if figures.is_empty() {
            return vec![];
        }
        let page = format!("https://pmc.ncbi.nlm.nih.gov/articles/{pmcid}/");
        let html = match self.get_page(&page).await {
            Ok(b) => String::from_utf8_lossy(&b).into_owned(),
            Err(e) => {
                log::warn!("PMC figure page failed: {e:#}");
                return vec![];
            }
        };
        use regex::Regex;
        use std::sync::LazyLock;
        static RE_SRC: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"src="(https://cdn\.ncbi\.nlm\.nih\.gov/pmc/blobs/[^"]+)""#).unwrap());
        let blobs: Vec<String> = RE_SRC.captures_iter(&html).filter_map(|c| c.get(1)).map(|m| m.as_str().to_string()).collect();
        figures
            .into_iter()
            .filter_map(|mut f| {
                let name = f.url.rsplit('/').next().unwrap_or("").to_string();
                let stem = name.rsplit_once('.').map(|x| x.0.to_string()).unwrap_or(name.clone());
                let blob = blobs.iter().find(|b| b.ends_with(&format!("/{name}"))).or_else(|| blobs.iter().find(|b| b.rsplit('/').next().is_some_and(|last| last.starts_with(&format!("{stem}.")))))?;
                f.url = blob.clone();
                f.page_url = page.clone();
                f.credit = "Figure from the paper (via PubMed Central)".into();
                Some(f)
            })
            .collect()
    }
}

/// Parse JATS XML into sections (by `<sec><title>`), licence and figures.
pub fn parse_jats(xml: &str, pmcid: &str) -> PmcFullText {
    use regex::Regex;
    use std::sync::LazyLock;
    static RE_BODY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<body[^>]*>(.*)</body>").unwrap());
    static RE_DROP: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?s)<(fig|table-wrap|disp-formula|supplementary-material|ref-list)\b.*?</(fig|table-wrap|disp-formula|supplementary-material|ref-list)>").unwrap());
    static RE_TOKEN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<sec\b[^>]*>|</sec>|<title>(.*?)</title>|<p\b[^>]*>(.*?)</p>").unwrap());
    static RE_LIC: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"(?s)<license\b[^>]*?(?:xlink:href|href)="([^"]+)"|<ali:license_ref[^>]*>([^<]+)</ali:license_ref>"#).unwrap());
    static RE_FIG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<fig\b[^>]*>(.*?)</fig>").unwrap());
    static RE_GRAPHIC: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"<graphic[^>]*?xlink:href="([^"]+)""#).unwrap());
    static RE_CAPTION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<caption>(.*?)</caption>").unwrap());

    static RE_LIC_P: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<license\b[^>]*>(.*?)</license>").unwrap());
    let licence = RE_LIC
        .captures(xml)
        .and_then(|c| c.get(1).or(c.get(2)))
        .map(|m| m.as_str().trim().to_string())
        .or_else(|| RE_LIC_P.captures(xml).and_then(|c| c.get(1)).and_then(|m| licence_from_text(&strip_tags(m.as_str()))));
    let mut out = PmcFullText { licence: licence.clone(), ..Default::default() };

    // Figures (only usable when the article licence is open).
    for fc in RE_FIG.captures_iter(xml).take(6) {
        let inner = fc.get(1).map(|m| m.as_str()).unwrap_or("");
        let Some(href) = RE_GRAPHIC.captures(inner).and_then(|c| c.get(1)).map(|m| m.as_str().to_string()) else { continue };
        let caption = RE_CAPTION.captures(inner).and_then(|c| c.get(1)).map(|m| strip_tags(m.as_str())).unwrap_or_default();
        let lower = href.to_lowercase();
        let file = if [".jpg", ".jpeg", ".png", ".gif"].iter().any(|e| lower.ends_with(e)) { href.clone() } else { format!("{href}.jpg") };
        let lic = licence.clone().unwrap_or_default();
        out.figures.push(ImageRef {
            url: format!("https://europepmc.org/articles/{pmcid}/bin/{file}"),
            thumb_url: None,
            page_url: format!("https://europepmc.org/article/PMC/{}", pmcid.trim_start_matches("PMC")),
            title: caption.chars().take(200).collect(),
            credit: "Figure from the paper (via Europe PMC)".into(),
            licence: super::wikimedia::licence_label(&lic),
            licence_url: lic.starts_with("http").then(|| lic.clone()),
            from_paper: true,
            embeddable: super::wikimedia::is_free_licence(&lic),
        });
    }

    let body = RE_BODY.captures(xml).and_then(|c| c.get(1)).map(|m| m.as_str()).unwrap_or("");
    let body = RE_DROP.replace_all(body, " ");
    let mut stack: Vec<String> = Vec::new();
    let mut sections: Vec<FullTextSection> = vec![FullTextSection::default()];
    let mut pending_title = false;
    for c in RE_TOKEN.captures_iter(&body) {
        let whole = c.get(0).map(|m| m.as_str()).unwrap_or("");
        if whole.starts_with("<sec") {
            stack.push(String::new());
            pending_title = true;
        } else if whole == "</sec>" {
            stack.pop();
        } else if let Some(t) = c.get(1) {
            let t = strip_tags(t.as_str());
            if pending_title {
                if let Some(top) = stack.last_mut() {
                    *top = t.clone();
                }
                pending_title = false;
                let heading = stack.iter().filter(|s| !s.is_empty()).cloned().collect::<Vec<_>>().join(" – ");
                sections.push(FullTextSection { heading, paragraphs: vec![] });
            }
        } else if let Some(p) = c.get(2) {
            pending_title = false;
            let t = strip_tags(p.as_str());
            if t.split_whitespace().count() >= 5 {
                if let Some(last) = sections.last_mut() {
                    last.paragraphs.push(t);
                }
            }
        }
    }
    sections.retain(|s| !s.paragraphs.is_empty());
    out.sections = sections;
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn research_jats_parsing() {
        let xml = r#"<article><front><permissions><license xlink:href="https://creativecommons.org/licenses/by/4.0/"><p>CC BY</p></license></permissions></front>
        <body><sec><title>Introduction</title><p>Cells divide in <italic>many</italic> interesting ways.</p>
        <fig id="f1"><caption><p>Figure one shows cells.</p></caption><graphic xlink:href="nihms-1"/></fig>
        <sec><title>Background</title><p>Earlier work found five distinct things here.</p></sec></sec>
        <sec><title>Methods</title><p>We used a microscope on every sample.</p></sec></body></article>"#;
        let ft = parse_jats(xml, "PMC123");
        let heads: Vec<&str> = ft.sections.iter().map(|s| s.heading.as_str()).collect();
        assert_eq!(heads, vec!["Introduction", "Introduction – Background", "Methods"]);
        assert_eq!(ft.sections[0].paragraphs[0], "Cells divide in many interesting ways.");
        assert_eq!(ft.figures.len(), 1);
        assert!(ft.figures[0].embeddable && ft.figures[0].from_paper);
        assert_eq!(ft.figures[0].url, "https://europepmc.org/articles/PMC123/bin/nihms-1.jpg");
        assert_eq!(licence_from_url("https://creativecommons.org/licenses/by-nc/4.0/"), "cc-by-nc");
        assert_eq!(licence_from_text("distributed under the terms of the Creative Commons Attribution License, which permits").as_deref(), Some("cc-by"));
        let plos = r#"<permissions><license><license-p>This is an open-access article distributed under the terms of the Creative Commons Attribution License.</license-p></license></permissions><body><fig id="g1"><caption><title>PPV</title></caption><graphic xlink:href="pmed.0020124.g001"/></fig></body>"#;
        let ft = parse_jats(plos, "PMC1");
        assert_eq!(ft.licence.as_deref(), Some("cc-by"));
        assert!(ft.figures[0].embeddable);
        assert_eq!(ft.figures[0].url, "https://europepmc.org/articles/PMC1/bin/pmed.0020124.g001.jpg");
    }
}
