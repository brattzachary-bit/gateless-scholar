//! arXiv Atom API (export.arxiv.org/api/query).

use anyhow::Result;
use chrono::{DateTime, Datelike};
use quick_xml::Reader;
use quick_xml::events::Event;

use crate::model::Paper;

use super::Research;
use super::dedupe::{clean_arxiv_id, finalize};
use super::http::{TTL_META, TTL_SEARCH, enc};
use super::text::{keywords, strip_tags};

const BASE: &str = "https://export.arxiv.org/api/query";

/// Parse an arXiv Atom feed into papers.
pub fn parse_atom(xml: &str) -> Vec<Paper> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut out = Vec::new();
    let mut cur: Option<Paper> = None;
    let mut path: Vec<String> = Vec::new();
    let mut text = String::new();
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                let name = String::from_utf8_lossy(e.local_name().as_ref()).to_string();
                if name == "entry" {
                    cur = Some(Paper { found_in: vec!["arxiv".into()], ..Default::default() });
                }
                if name == "link" {
                    handle_link(&e, cur.as_mut());
                }
                if name == "category" {
                    handle_category(&e, cur.as_mut());
                }
                path.push(name);
                text.clear();
            }
            Ok(Event::Empty(e)) => {
                let name = String::from_utf8_lossy(e.local_name().as_ref()).to_string();
                if name == "link" {
                    handle_link(&e, cur.as_mut());
                } else if name == "category" || name == "primary_category" {
                    handle_category(&e, cur.as_mut());
                }
            }
            Ok(Event::Text(t)) => {
                if let Ok(s) = t.unescape() {
                    text.push_str(&s);
                }
            }
            Ok(Event::CData(t)) => text.push_str(&String::from_utf8_lossy(&t)),
            Ok(Event::End(e)) => {
                let name = String::from_utf8_lossy(e.local_name().as_ref()).to_string();
                let parent = path.len().checked_sub(2).and_then(|i| path.get(i)).cloned().unwrap_or_default();
                if let Some(p) = cur.as_mut() {
                    let t = text.trim();
                    match (name.as_str(), parent.as_str()) {
                        ("id", "entry") => {
                            if let Some(rest) = t.split("/abs/").nth(1) {
                                p.arxiv_id = Some(clean_arxiv_id(rest));
                            }
                        }
                        ("title", "entry") => p.title = collapse(t),
                        ("summary", "entry") => p.abstract_text = Some(strip_tags(&collapse(t))),
                        ("published", "entry") => {
                            if let Ok(d) = DateTime::parse_from_rfc3339(t) {
                                p.date = Some(d.date_naive());
                                p.year = Some(d.date_naive().year());
                            }
                        }
                        ("name", "author") => p.authors.push(collapse(t)),
                        ("doi", "entry") => p.doi = Some(crate::model::normalize_doi(t)),
                        ("journal_ref", "entry") => p.venue = Some(collapse(t)),
                        _ => {}
                    }
                }
                if name == "entry" {
                    if let Some(mut p) = cur.take() {
                        if p.arxiv_id.is_some() && !p.title.is_empty() && p.title != "Error" {
                            if p.venue.is_none() {
                                p.venue = Some("arXiv".into());
                            }
                            if let Some(a) = &p.arxiv_id {
                                p.url = Some(format!("https://arxiv.org/abs/{a}"));
                                p.oa_url = Some(format!("https://arxiv.org/abs/{a}"));
                                p.oa_status = Some("green".into());
                                p.oa_version = Some("submittedVersion".into());
                            }
                            finalize(&mut p);
                            out.push(p);
                        }
                    }
                }
                path.pop();
                text.clear();
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                log::warn!("arXiv Atom parse error: {e}");
                break;
            }
            _ => {}
        }
    }
    out
}

fn collapse(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn attr(e: &quick_xml::events::BytesStart, key: &str) -> Option<String> {
    e.attributes().flatten().find(|a| a.key.local_name().as_ref() == key.as_bytes()).and_then(|a| a.unescape_value().ok()).map(|v| v.to_string())
}

fn handle_link(e: &quick_xml::events::BytesStart, cur: Option<&mut Paper>) {
    let Some(p) = cur else { return };
    if attr(e, "title").as_deref() == Some("doi") {
        if let Some(h) = attr(e, "href") {
            p.doi.get_or_insert_with(|| crate::model::normalize_doi(&h));
        }
    }
}

fn handle_category(e: &quick_xml::events::BytesStart, cur: Option<&mut Paper>) {
    let Some(p) = cur else { return };
    if let Some(t) = attr(e, "term") {
        if !p.subjects.contains(&t) && p.subjects.len() < 4 {
            p.subjects.push(t);
        }
    }
}

/// Build an arXiv `search_query` from free text: AND of the top keywords over all fields.
pub fn arxiv_query(text: &str) -> String {
    let kws = keywords(text, 5);
    if kws.is_empty() {
        return format!("all:{}", text.trim());
    }
    kws.iter().map(|k| format!("all:{}", k.replace('-', " "))).collect::<Vec<_>>().join(" AND ")
}

impl Research {
    /// Search arXiv. `newest` sorts by submission date (otherwise relevance).
    pub(crate) async fn arxiv_search(&self, text: &str, start: u32, max: u32, newest: bool) -> Result<Vec<Paper>> {
        let sort = if newest { "submittedDate" } else { "relevance" };
        let url = format!("{BASE}?search_query={}&start={start}&max_results={}&sortBy={sort}&sortOrder=descending", enc(&arxiv_query(text)), max.clamp(1, 100));
        let xml = self.get_text(&url, TTL_SEARCH).await?;
        Ok(parse_atom(&xml))
    }

    pub(crate) async fn arxiv_by_id(&self, id: &str) -> Result<Option<Paper>> {
        let url = format!("{BASE}?id_list={}", enc(&clean_arxiv_id(id)));
        let xml = self.get_text(&url, TTL_META).await?;
        Ok(parse_atom(&xml).into_iter().next())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn research_arxiv_atom_parsing() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom" xmlns:arxiv="http://arxiv.org/schemas/atom">
  <title type="html">ArXiv Query</title>
  <id>http://arxiv.org/api/x</id>
  <entry>
    <id>http://arxiv.org/abs/2401.01234v2</id>
    <updated>2024-01-05T00:00:00Z</updated>
    <published>2024-01-03T18:00:00Z</published>
    <title>Attention Is   All
      You Need &amp; More</title>
    <summary>  We propose a new
      architecture.  </summary>
    <author><name>Ashish Vaswani</name></author>
    <author><name>Noam Shazeer</name><arxiv:affiliation>Google</arxiv:affiliation></author>
    <arxiv:doi>10.1000/abc</arxiv:doi>
    <link title="doi" href="http://dx.doi.org/10.1000/abc" rel="related"/>
    <link href="http://arxiv.org/abs/2401.01234v2" rel="alternate" type="text/html"/>
    <arxiv:primary_category term="cs.CL" scheme="http://arxiv.org/schemas/atom"/>
    <category term="cs.CL" scheme="http://arxiv.org/schemas/atom"/>
    <category term="cs.LG" scheme="http://arxiv.org/schemas/atom"/>
  </entry>
</feed>"#;
        let ps = parse_atom(xml);
        assert_eq!(ps.len(), 1);
        let p = &ps[0];
        assert_eq!(p.title, "Attention Is All You Need & More");
        assert_eq!(p.arxiv_id.as_deref(), Some("2401.01234"));
        assert_eq!(p.abstract_text.as_deref(), Some("We propose a new architecture."));
        assert_eq!(p.authors, vec!["Ashish Vaswani", "Noam Shazeer"]);
        assert_eq!(p.year, Some(2024));
        assert_eq!(p.doi.as_deref(), Some("10.1000/abc"));
        assert_eq!(p.subjects, vec!["cs.CL", "cs.LG"]);
        assert_eq!(p.kind, crate::model::ContentKind::FreeFullText);
        assert_eq!(p.key, "doi:10.1000/abc");
        assert!(arxiv_query("CRISPR base editing").contains("all:CRISPR AND"));
    }
}
