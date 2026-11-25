//! Wikimedia Commons image search with licence metadata.

use anyhow::Result;
use serde_json::Value;

use crate::model::ImageRef;

use super::Research;
use super::http::{TTL_META, enc};
use super::text::strip_tags;

/// True for free licences we may embed: Creative Commons (without NC/ND), CC0, public
/// domain, GFDL and similar free licences.
pub fn is_free_licence(lic: &str) -> bool {
    let l = lic.to_lowercase().replace(['_', '-'], " ");
    if l.trim().is_empty() {
        return false;
    }
    if l.contains(" nc") || l.contains("noncommercial") || l.contains("non commercial") || l.contains(" nd") || l.contains("noderiv") {
        return false;
    }
    if l.contains("fair use") || l.contains("all rights reserved") || l.contains("copyright") && !l.contains("copyrighted free use") {
        return false;
    }
    l.starts_with("cc")
        || l.contains("creativecommons.org")
        || l.contains("creative commons")
        || l.contains("public domain")
        || l.starts_with("pd")
        || l.contains("cc0")
        || l.contains("gfdl")
        || l.contains("copyrighted free use")
        || l == "attribution"
        || l.contains("mit license")
        || l.contains("ogl")
}

/// Human-readable licence label from Unpaywall/OpenAlex ids ("cc-by") or URLs.
pub fn licence_label(lic: &str) -> String {
    let l = lic.trim();
    if l.is_empty() {
        return "Unknown licence".into();
    }
    let lower = l.to_lowercase();
    if lower.contains("creativecommons.org/publicdomain") || lower == "cc0" {
        return "CC0".into();
    }
    if let Some(rest) = lower.split("/licenses/").nth(1) {
        let mut parts = rest.trim_end_matches('/').split('/');
        let kind = parts.next().unwrap_or("").to_uppercase();
        let ver = parts.next().unwrap_or("");
        return format!("CC {kind} {ver}").trim().to_string();
    }
    if let Some(kind) = lower.strip_prefix("cc-") {
        return format!("CC {}", kind.to_uppercase());
    }
    l.to_string()
}

fn s(v: &Value) -> Option<String> {
    v.as_str().map(str::trim).filter(|s| !s.is_empty()).map(String::from)
}

/// Parse the Commons API response (generator=search, prop=imageinfo) into images, best first.
pub fn parse_commons(v: &Value) -> Vec<ImageRef> {
    let mut pages: Vec<&Value> = match &v["query"]["pages"] {
        Value::Object(m) => m.values().collect(),
        Value::Array(a) => a.iter().collect(),
        _ => vec![],
    };
    pages.sort_by_key(|p| p["index"].as_i64().unwrap_or(i64::MAX));
    let mut out = Vec::new();
    for p in pages {
        let info = &p["imageinfo"][0];
        let Some(mut url) = s(&info["url"]) else { continue };
        let mime = s(&info["mime"]).unwrap_or_default();
        match mime.as_str() {
            "image/jpeg" | "image/png" | "image/webp" | "image/gif" => {}
            // Diagrams (SVG) and TIFF scans: use Commons' PNG/JPEG rendering of them.
            "image/svg+xml" | "image/tiff" => match s(&info["thumburl"]) {
                Some(t) => url = t,
                None => continue,
            },
            _ => continue,
        }
        let w = info["width"].as_u64().unwrap_or(0);
        let h = info["height"].as_u64().unwrap_or(0);
        if (w > 0 && w < 200) || (h > 0 && h < 150) {
            continue;
        }
        let meta = &info["extmetadata"];
        let licence = s(&meta["LicenseShortName"]["value"]).map(|l| strip_tags(&l)).unwrap_or_default();
        let credit = s(&meta["Artist"]["value"]).map(|a| strip_tags(&a)).or_else(|| s(&meta["Credit"]["value"]).map(|a| strip_tags(&a))).unwrap_or_default();
        let title = s(&meta["ObjectName"]["value"])
            .map(|t| strip_tags(&t))
            .or_else(|| s(&p["title"]).map(|t| t.trim_start_matches("File:").rsplit_once('.').map(|x| x.0.to_string()).unwrap_or(t)))
            .unwrap_or_default();
        let free = is_free_licence(&licence) || meta["License"]["value"].as_str().is_some_and(|x| x.starts_with("pd") || x.starts_with("cc"));
        out.push(ImageRef {
            thumb_url: s(&info["thumburl"]),
            page_url: s(&info["descriptionurl"]).unwrap_or_else(|| url.clone()),
            url,
            title,
            credit: credit.chars().take(200).collect(),
            licence: if licence.is_empty() { "See file page".into() } else { licence },
            licence_url: s(&meta["LicenseUrl"]["value"]),
            from_paper: false,
            embeddable: free,
        });
    }
    out
}

impl Research {
    /// Lead images of the English Wikipedia articles that best match `query`, best first.
    /// Only images hosted on Commons are returned (they carry a licence we can check);
    /// Wikipedia's local non-free images are skipped. The image title is the article's name.
    pub(crate) async fn wikipedia_images(&self, query: &str, limit: u32) -> Result<Vec<ImageRef>> {
        let url = format!(
            "https://en.wikipedia.org/w/api.php?action=query&format=json&generator=search&gsrsearch={}&gsrnamespace=0&gsrlimit={}&prop=pageimages&piprop=name",
            enc(query.trim()),
            limit.clamp(1, 10)
        );
        let v = self.get_json(&url, TTL_META).await?;
        let mut pages: Vec<(i64, String, String)> = match &v["query"]["pages"] {
            Value::Object(m) => m.values().filter_map(|p| Some((p["index"].as_i64().unwrap_or(i64::MAX), s(&p["title"])?, s(&p["pageimage"])?))).collect(),
            _ => vec![],
        };
        pages.sort_by_key(|p| p.0);
        if pages.is_empty() {
            return Ok(vec![]);
        }
        let titles = pages.iter().map(|p| format!("File:{}", p.2)).collect::<Vec<_>>().join("|");
        let url = format!(
            "https://commons.wikimedia.org/w/api.php?action=query&format=json&titles={}&prop=imageinfo&iiprop=url%7Cextmetadata%7Cmime%7Csize&iiurlwidth=640&iiextmetadatafilter=LicenseShortName%7CLicenseUrl%7CLicense%7CArtist%7CCredit%7CObjectName",
            enc(&titles)
        );
        let v = self.get_json(&url, TTL_META).await?;
        // Files missing from Commons (local, often non-free) have no imageinfo and drop out here.
        let norm = |t: &str| t.trim_start_matches("File:").replace('_', " ").to_lowercase();
        let mut by_title = std::collections::HashMap::new();
        if let Value::Object(m) = &v["query"]["pages"] {
            for p in m.values() {
                let Some(t) = s(&p["title"]) else { continue };
                if let Some(img) = parse_commons(&serde_json::json!({ "query": { "pages": [p] } })).into_iter().next() {
                    by_title.insert(norm(&t), img);
                }
            }
        }
        let mut out = vec![];
        for (_, article, file) in &pages {
            if let Some(img) = by_title.get(&norm(file)) {
                let mut img = img.clone();
                img.title = format!("{article} (Wikipedia)");
                if img.embeddable {
                    out.push(img);
                }
            }
        }
        Ok(out)
    }

    /// Commons file search for a query; images only, best first.
    pub(crate) async fn commons_search(&self, query: &str, limit: u32) -> Result<Vec<ImageRef>> {
        let q = format!("{} filetype:bitmap", query.trim());
        let url = format!(
            "https://commons.wikimedia.org/w/api.php?action=query&format=json&generator=search&gsrsearch={}&gsrnamespace=6&gsrlimit={}&prop=imageinfo&iiprop=url%7Cextmetadata%7Cmime%7Csize&iiurlwidth=640&iiextmetadatafilter=LicenseShortName%7CLicenseUrl%7CLicense%7CArtist%7CCredit%7CObjectName",
            enc(&q),
            limit.clamp(1, 20)
        );
        let v = self.get_json(&url, TTL_META).await?;
        Ok(parse_commons(&v))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn research_wikimedia_licence_embeddable() {
        for free in ["CC BY-SA 4.0", "CC BY 2.0", "CC0", "Public domain", "PD-USGov-NASA", "cc-by", "https://creativecommons.org/licenses/by/4.0/", "GFDL"] {
            assert!(is_free_licence(free), "{free}");
        }
        for not in ["CC BY-NC 4.0", "cc-by-nc-nd", "CC BY-ND 2.0", "Fair use", "", "All rights reserved", "Copyright Getty Images"] {
            assert!(!is_free_licence(not), "{not}");
        }
        assert_eq!(licence_label("https://creativecommons.org/licenses/by/4.0/"), "CC BY 4.0");
        assert_eq!(licence_label("cc-by-sa"), "CC BY-SA");

        let v = serde_json::json!({"query": {"pages": {
            "2": {"index": 2, "title": "File:B.jpg", "imageinfo": [{"url": "https://u/B.jpg", "mime": "image/jpeg", "width": 800, "height": 600, "descriptionurl": "https://c/File:B.jpg",
                "extmetadata": {"LicenseShortName": {"value": "Fair use"}, "Artist": {"value": "<a href='x'>Bob</a>"}}}]},
            "1": {"index": 1, "title": "File:A.png", "imageinfo": [{"url": "https://u/A.png", "thumburl": "https://u/t/A.png", "mime": "image/png", "width": 800, "height": 600, "descriptionurl": "https://c/File:A.png",
                "extmetadata": {"LicenseShortName": {"value": "CC BY-SA 4.0"}, "Artist": {"value": "<span>Alice</span>"}, "LicenseUrl": {"value": "https://creativecommons.org/licenses/by-sa/4.0"}}}]},
            "3": {"index": 3, "title": "File:C.pdf", "imageinfo": [{"url": "https://u/C.pdf", "mime": "application/pdf"}]}
        }}});
        let imgs = parse_commons(&v);
        assert_eq!(imgs.len(), 2);
        assert_eq!(imgs[0].title, "A");
        assert_eq!(imgs[0].credit, "Alice");
        assert!(imgs[0].embeddable);
        assert!(!imgs[1].embeddable);
        assert_eq!(imgs[1].credit, "Bob");
    }
}
