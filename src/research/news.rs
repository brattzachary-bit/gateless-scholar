//! News sources: GDELT DOC 2.0 (artlist JSON), outlet RSS/Atom feeds, and title clustering.

use std::collections::HashSet;

use anyhow::Result;
use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use quick_xml::Reader;
use quick_xml::events::Event;
use serde_json::Value;

use crate::model::{ContentKind, ImageRef, Paper};

use super::Research;
use super::http::{Get, TTL_NEWS, enc};
use super::text::{keywords, overlap, strip_tags, token_set};

fn s(v: &Value) -> Option<String> {
    v.as_str().map(str::trim).filter(|s| !s.is_empty()).map(String::from)
}

/// Host without "www." (used as the outlet id).
pub fn domain_of(url: &str) -> String {
    url::Url::parse(url).ok().and_then(|u| u.host_str().map(|h| h.trim_start_matches("www.").to_lowercase())).unwrap_or_default()
}

/// A finished news "paper" record: kind News, key from URL.
/// Drop tracking parameters (utm_*, at_*, fbclid, …) from an article URL.
pub fn clean_url(u: &str) -> String {
    let Ok(mut parsed) = url::Url::parse(u) else { return u.to_string() };
    let kept: Vec<(String, String)> = parsed
        .query_pairs()
        .filter(|(k, _)| !(k.starts_with("utm_") || k.starts_with("at_") || matches!(k.as_ref(), "fbclid" | "gclid" | "cmpid" | "ref" | "src")))
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    if kept.is_empty() {
        parsed.set_query(None);
    } else {
        parsed.query_pairs_mut().clear().extend_pairs(kept);
    }
    parsed.set_fragment(None);
    parsed.to_string()
}

pub fn news_paper(title: String, url: String, outlet: String, date: Option<NaiveDate>, summary: Option<String>, image: Option<String>, source: &str) -> Paper {
    let url = clean_url(&url);
    let mut p = Paper {
        title: strip_tags(&title),
        url: Some(url.clone()),
        venue: Some(outlet.clone()),
        date,
        year: date.map(|d| chrono::Datelike::year(&d)),
        abstract_text: summary.map(|s| strip_tags(&s)).filter(|s| s.split_whitespace().count() >= 5),
        found_in: vec![source.into()],
        kind: ContentKind::News,
        language: Some("en".into()),
        ..Default::default()
    };
    if let Some(img) = image.filter(|i| i.starts_with("http")) {
        p.image = Some(link_only_image(&img, &url, &outlet, &p.title));
    }
    p.key = format!("url:{}", crate::model::short_hash(&url));
    p
}

/// News photos are usually copyrighted: link-only.
pub fn link_only_image(img: &str, page: &str, outlet: &str, title: &str) -> ImageRef {
    ImageRef {
        url: img.to_string(),
        thumb_url: None,
        page_url: page.to_string(),
        title: title.to_string(),
        credit: outlet.to_string(),
        licence: "Copyright of the publisher (link only)".into(),
        licence_url: None,
        from_paper: false,
        embeddable: false,
    }
}

// --------------------------------------------------------------------------------------- GDELT

pub fn parse_gdelt(v: &Value) -> Vec<Paper> {
    v["articles"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|a| {
            let url = s(&a["url"])?;
            let title = s(&a["title"])?;
            let lang = s(&a["language"]).unwrap_or_default();
            if !lang.is_empty() && !lang.eq_ignore_ascii_case("english") {
                return None;
            }
            let date = s(&a["seendate"]).and_then(|d| NaiveDateTime::parse_from_str(&d, "%Y%m%dT%H%M%SZ").ok()).map(|d| d.date());
            let outlet = s(&a["domain"]).unwrap_or_else(|| domain_of(&url));
            Some(news_paper(title, url, outlet, date, None, s(&a["socialimage"]), "gdelt"))
        })
        .collect()
}

/// GDELT query from free text: the key terms (ANDed), plus English only.
pub fn gdelt_query(text: &str, max_terms: usize) -> String {
    let kws: Vec<String> = keywords(text, max_terms)
        .into_iter()
        .filter(|k| k.chars().count() >= 3)
        .map(|k| if k.contains('-') { format!("\"{}\"", k.replace('-', " ")) } else { k })
        .collect();
    format!("{} sourcelang:english", kws.join(" "))
}

impl Research {
    /// GDELT DOC 2.0 article list. `query` is a GDELT query string.
    pub(crate) async fn gdelt_search(&self, query: &str, since: Option<NaiveDate>, max: u32) -> Result<Vec<Paper>> {
        let window = match since {
            // `startdatetime` alone returns nothing; a relative timespan works (max ~3 months).
            Some(d) => {
                let days = (Utc::now().date_naive() - d).num_days().clamp(1, 85);
                format!("&timespan={days}d")
            }
            None => "&timespan=7d".into(),
        };
        let url = format!("https://api.gdeltproject.org/api/v2/doc/doc?query={}&mode=artlist&format=json&maxrecords={}&sort=hybridrel{window}", enc(query), max.clamp(1, 250));
        // GDELT answers rate limits and query errors with plain text and status 200, so cache
        // by hand and only cache real JSON.
        if let Some((body, fetched)) = self.db.cache_get(&url) {
            if (Utc::now() - fetched).to_std().unwrap_or_default() < TTL_NEWS {
                if let Ok(v) = serde_json::from_slice::<Value>(&body) {
                    return Ok(parse_gdelt(&v));
                }
            }
        }
        let mut attempt = 0;
        let text = loop {
            let body = self.get_bytes(&url, Get { ttl: None, retries: 1, timeout: Some(std::time::Duration::from_secs(45)), ..Default::default() }).await?;
            let text = String::from_utf8_lossy(&body).into_owned();
            if text.trim_start().starts_with("Please limit requests") && attempt < 2 {
                attempt += 1;
                tokio::time::sleep(std::time::Duration::from_secs(6)).await;
                continue;
            }
            break text;
        };
        let trimmed = text.trim();
        if trimmed.is_empty() || trimmed == "{}" {
            return Ok(vec![]);
        }
        if !trimmed.starts_with('{') {
            anyhow::bail!("GDELT: {}", trimmed.chars().take(160).collect::<String>());
        }
        let _ = self.db.cache_put(&url, trimmed.as_bytes());
        let v: Value = serde_json::from_str(trimmed).unwrap_or(Value::Null);
        Ok(parse_gdelt(&v))
    }
}

// ----------------------------------------------------------------------------------------- RSS

fn parse_date_any(s: &str) -> Option<NaiveDate> {
    let s = s.trim();
    DateTime::parse_from_rfc2822(s)
        .or_else(|_| DateTime::parse_from_rfc3339(s))
        .map(|d| d.date_naive())
        .ok()
        .or_else(|| NaiveDate::parse_from_str(s.get(..10).unwrap_or(s), "%Y-%m-%d").ok())
        .or_else(|| {
            // RFC 2822 variants with zone names chrono rejects ("EST", "GMT+1").
            let parts: Vec<&str> = s.split_whitespace().collect();
            if parts.len() >= 4 {
                NaiveDate::parse_from_str(&parts[1..4].join(" "), "%d %b %Y").ok()
            } else {
                None
            }
        })
}

/// Parse an RSS 2.0 or Atom feed. Returns (channel title, items).
pub fn parse_feed(xml: &str, feed_url: &str) -> (String, Vec<Paper>) {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut channel_title = String::new();
    let mut items = Vec::new();
    let mut in_item = false;
    let mut item_level = 0usize;
    let mut text = String::new();
    let (mut title, mut link, mut desc, mut date, mut image) = (String::new(), String::new(), String::new(), String::new(), String::new());
    let mut path: Vec<String> = Vec::new();
    let attr = |e: &quick_xml::events::BytesStart, k: &str| -> Option<String> {
        e.attributes().flatten().find(|a| a.key.local_name().as_ref() == k.as_bytes()).and_then(|a| a.unescape_value().ok()).map(|v| v.to_string())
    };
    loop {
        let ev = reader.read_event();
        match ev {
            Ok(Event::Start(ref e)) | Ok(Event::Empty(ref e)) => {
                let is_empty = matches!(ev, Ok(Event::Empty(_)));
                let name = String::from_utf8_lossy(e.local_name().as_ref()).to_lowercase();
                if (name == "item" || name == "entry") && !is_empty {
                    in_item = true;
                    item_level = path.len() + 1;
                    title.clear();
                    link.clear();
                    desc.clear();
                    date.clear();
                    image.clear();
                }
                if in_item {
                    if name == "link" {
                        if let Some(h) = attr(e, "href") {
                            let rel = attr(e, "rel").unwrap_or_else(|| "alternate".into());
                            if rel == "alternate" && link.is_empty() {
                                link = h;
                            } else if rel == "enclosure" && image.is_empty() && attr(e, "type").is_some_and(|t| t.starts_with("image")) {
                                image = h;
                            }
                        }
                    }
                    if matches!(name.as_str(), "content" | "thumbnail" | "enclosure") && image.is_empty() {
                        if let Some(u) = attr(e, "url") {
                            let ty = attr(e, "type").unwrap_or_default();
                            let medium = attr(e, "medium").unwrap_or_default();
                            if ty.starts_with("image") || medium == "image" || name == "thumbnail" || u.ends_with(".jpg") || u.ends_with(".png") || u.ends_with(".jpeg") {
                                image = u;
                            }
                        }
                    }
                }
                if !is_empty {
                    path.push(name);
                    text.clear();
                }
            }
            Ok(Event::Text(t)) => {
                if let Ok(u) = t.unescape() {
                    text.push_str(&u);
                }
            }
            Ok(Event::CData(t)) => text.push_str(&String::from_utf8_lossy(&t)),
            Ok(Event::End(e)) => {
                let name = String::from_utf8_lossy(e.local_name().as_ref()).to_lowercase();
                let t = text.trim().to_string();
                if in_item {
                    // Only direct children of the item.
                    if path.len() == item_level + 1 {
                        match name.as_str() {
                            "title" if title.is_empty() => title = strip_tags(&t),
                            "link" if link.is_empty() && !t.is_empty() => link = t.clone(),
                            "guid" if link.is_empty() && t.starts_with("http") => link = t.clone(),
                            "description" | "summary" if desc.is_empty() => desc = strip_tags(&t),
                            "encoded" | "content" if desc.is_empty() => desc = strip_tags(&t),
                            "pubdate" | "published" | "updated" | "date" if date.is_empty() => date = t.clone(),
                            _ => {}
                        }
                    }
                    if name == "item" || name == "entry" {
                        in_item = false;
                        if !title.is_empty() && link.starts_with("http") {
                            let outlet = String::new();
                            let summary: String = desc.split_whitespace().take(120).collect::<Vec<_>>().join(" ");
                            items.push((title.clone(), link.clone(), outlet, parse_date_any(&date), (!summary.is_empty()).then_some(summary), (!image.is_empty()).then(|| image.clone())));
                        }
                    }
                } else if name == "title" && channel_title.is_empty() && path.iter().any(|p| p == "channel" || p == "feed") {
                    channel_title = strip_tags(&t);
                }
                path.pop();
                text.clear();
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                log::warn!("feed parse error in {feed_url}: {e}");
                break;
            }
            _ => {}
        }
    }
    let fallback = domain_of(feed_url);
    let outlet = clean_outlet(&channel_title, &fallback);
    let papers = items
        .into_iter()
        .map(|(title, link, _, date, summary, image)| news_paper(title, link, outlet.clone(), date, summary, image, "rss"))
        .collect();
    (outlet, papers)
}

/// "BBC News - Science & Environment" → "BBC News"; empty → domain.
fn clean_outlet(channel: &str, domain: &str) -> String {
    let c = channel.trim();
    if c.is_empty() {
        return domain.to_string();
    }
    // "Science | The Guardian" puts the outlet last; "BBC News - Science" and
    // "The Conversation – Articles" put it first.
    let head = if let Some((_, last)) = c.rsplit_once(" | ") {
        last
    } else {
        c.split(" - ").next().unwrap_or(c).split(" – ").next().unwrap_or(c).split(" : ").next().unwrap_or(c)
    }
    .trim();
    if head.is_empty() { domain.to_string() } else { head.to_string() }
}

impl Research {
    /// All items from the configured outlet feeds (in parallel; failing feeds are skipped).
    pub(crate) async fn rss_items(&self) -> Vec<Paper> {
        use futures::StreamExt;
        let feeds = self.config.news_feeds.clone();
        let results: Vec<Vec<Paper>> = futures::stream::iter(feeds)
            .map(|f| async move {
                match self.get_text(&f, TTL_NEWS).await {
                    Ok(xml) => parse_feed(&xml, &f).1,
                    Err(e) => {
                        log::warn!("RSS feed {f} failed: {e:#}");
                        vec![]
                    }
                }
            })
            .buffer_unordered(6)
            .collect()
            .await;
        results.into_iter().flatten().collect()
    }
}

// ---------------------------------------------------------------------------------- clustering

/// Whether two titles describe the same event.
pub fn same_event(a: &HashSet<String>, b: &HashSet<String>) -> bool {
    let (shared, ov) = overlap(a, b);
    shared >= 2 && (super::text::jaccard(a, b) >= 0.25 || ov >= 0.5)
}

/// Group articles into events by title-token overlap (single link). Clusters come back with
/// most distinct outlets first, then most recent. Within a cluster the first article is the
/// most representative (highest total similarity to the others).
pub fn cluster_titles(articles: Vec<Paper>) -> Vec<Vec<Paper>> {
    let sets: Vec<HashSet<String>> = articles.iter().map(|a| token_set(&a.title)).collect();
    let n = articles.len();
    let mut parent: Vec<usize> = (0..n).collect();
    fn find(p: &mut Vec<usize>, i: usize) -> usize {
        let mut r = i;
        while p[r] != r {
            r = p[r];
        }
        let mut c = i;
        while p[c] != r {
            let next = p[c];
            p[c] = r;
            c = next;
        }
        r
    }
    for i in 0..n {
        for j in i + 1..n {
            if same_event(&sets[i], &sets[j]) {
                let (ri, rj) = (find(&mut parent, i), find(&mut parent, j));
                if ri != rj {
                    parent[rj] = ri;
                }
            }
        }
    }
    let mut groups: std::collections::BTreeMap<usize, Vec<usize>> = Default::default();
    for i in 0..n {
        let r = find(&mut parent, i);
        groups.entry(r).or_default().push(i);
    }
    let mut clusters: Vec<Vec<usize>> = groups.into_values().collect();
    for c in clusters.iter_mut() {
        // Representative first: most similar to the rest.
        let score = |i: usize| -> f32 { c.iter().filter(|&&j| j != i).map(|&j| super::text::jaccard(&sets[i], &sets[j])).sum() };
        let mut scored: Vec<(usize, f32)> = c.iter().map(|&i| (i, score(i))).collect();
        scored.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        *c = scored.into_iter().map(|x| x.0).collect();
    }
    let outlets = |c: &Vec<usize>| -> usize { c.iter().map(|&i| articles[i].venue.clone().unwrap_or_default().to_lowercase()).collect::<HashSet<_>>().len() };
    let newest = |c: &Vec<usize>| -> Option<NaiveDate> { c.iter().filter_map(|&i| articles[i].date).max() };
    clusters.sort_by(|a, b| outlets(b).cmp(&outlets(a)).then(newest(b).cmp(&newest(a))));
    let mut slots: Vec<Option<Paper>> = articles.into_iter().map(Some).collect();
    clusters.into_iter().map(|c| c.into_iter().filter_map(|i| slots[i].take()).collect()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a(title: &str, outlet: &str) -> Paper {
        news_paper(title.into(), format!("https://{outlet}/{}", crate::model::short_hash(title)), outlet.into(), None, None, None, "test")
    }

    #[test]
    fn research_title_clustering() {
        let arts = vec![
            a("NASA delays Artemis II moon mission launch to April", "bbc.co.uk"),
            a("Wildfire spreads across southern California hills", "npr.org"),
            a("Artemis II launch delayed again, NASA says", "reuters.com"),
            a("NASA's Artemis II moon launch pushed back", "theguardian.com"),
            a("Stock markets rally on tech earnings", "ft.com"),
        ];
        let cl = cluster_titles(arts);
        assert_eq!(cl.len(), 3, "{:#?}", cl.iter().map(|c| c.iter().map(|p| p.title.clone()).collect::<Vec<_>>()).collect::<Vec<_>>());
        assert_eq!(cl[0].len(), 3);
        assert!(cl[0].iter().all(|p| p.title.contains("Artemis")));
    }

    #[test]
    fn research_rss_and_atom_parsing() {
        let rss = r#"<?xml version="1.0"?><rss xmlns:media="http://search.yahoo.com/mrss/"><channel><title>BBC News - Science</title>
        <item><title>Comet seen &amp; photographed</title><link>https://bbc.co.uk/1</link><description><![CDATA[<p>A comet was seen over the UK last night by many people.</p>]]></description>
        <pubDate>Mon, 24 Nov 2025 10:00:00 GMT</pubDate><media:thumbnail width="240" url="https://img/1.jpg"/></item></channel></rss>"#;
        let (outlet, items) = parse_feed(rss, "https://feeds.bbci.co.uk/x.xml");
        assert_eq!(outlet, "BBC News");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].title, "Comet seen & photographed");
        assert_eq!(items[0].url.as_deref(), Some("https://bbc.co.uk/1"));
        assert_eq!(clean_url("https://x.com/a?id=3&utm_source=rss&at_medium=RSS#top"), "https://x.com/a?id=3");
        assert_eq!(clean_outlet("Science | The Guardian", "theguardian.com"), "The Guardian");
        assert_eq!(clean_outlet("The Conversation – Articles (GLOBAL)", "x"), "The Conversation");
        assert_eq!(items[0].date, NaiveDate::from_ymd_opt(2025, 11, 24));
        assert!(items[0].image.as_ref().is_some_and(|i| !i.embeddable && i.url == "https://img/1.jpg"));
        assert_eq!(items[0].kind, ContentKind::News);

        let atom = r#"<feed xmlns="http://www.w3.org/2005/Atom"><title>Ars Technica</title><entry><title>Quantum chip</title>
        <link rel="alternate" href="https://ars/q"/><updated>2025-11-20T12:00:00Z</updated><summary>Short summary of a quantum chip story here.</summary></entry></feed>"#;
        let (o2, it2) = parse_feed(atom, "https://feeds.arstechnica.com/x");
        assert_eq!(o2, "Ars Technica");
        assert_eq!(it2[0].url.as_deref(), Some("https://ars/q"));
        assert_eq!(it2[0].date, NaiveDate::from_ymd_opt(2025, 11, 20));

        let g = serde_json::json!({"articles": [
            {"url": "https://x.com/a", "title": "Hello world story", "seendate": "20251124T101500Z", "domain": "x.com", "language": "English", "socialimage": "https://x.com/i.jpg"},
            {"url": "https://y.fr/b", "title": "Bonjour", "seendate": "20251124T101500Z", "domain": "y.fr", "language": "French"}
        ]});
        let gp = parse_gdelt(&g);
        assert_eq!(gp.len(), 1);
        assert_eq!(gp[0].venue.as_deref(), Some("x.com"));
        assert_eq!(gp[0].date, NaiveDate::from_ymd_opt(2025, 11, 24));
    }
}
