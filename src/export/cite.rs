//! Reference formatting. Cite real sources, never the AI article itself.
//!
//! - [`reference`] / [`paper_reference`]: one reference-list entry in Harvard (Cite Them Right
//!   flavour), APA 7 or Vancouver.
//! - [`bibtex`] / [`ris`] (and the `_sources` variants): bibliography files for Zotero etc.
//!
//! Author names may arrive as "First Middle Last" or "Last, First"; both are parsed.
//! Everything here is pure string formatting and never panics.

use chrono::{Datelike, NaiveDate};

use crate::model::*;

/// One reference-list entry for a source in the given style.
pub fn reference(s: &Source, style: CitationStyle) -> String {
    let r = Ref::from_source(s);
    match style {
        CitationStyle::Harvard => harvard(&r),
        CitationStyle::Apa => apa(&r),
        CitationStyle::Vancouver => vancouver(&r),
    }
}

/// One reference-list entry for a paper in the given style.
pub fn paper_reference(p: &Paper, style: CitationStyle) -> String {
    let r = Ref::from_paper(p);
    match style {
        CitationStyle::Harvard => harvard(&r),
        CitationStyle::Apa => apa(&r),
        CitationStyle::Vancouver => vancouver(&r),
    }
}

/// A short in-text citation such as "(Smith et al., 2024)" (Harvard / APA) or "[3]"
/// (Vancouver, using the source's number).
pub fn in_text(s: &Source, style: CitationStyle) -> String {
    let r = Ref::from_source(s);
    match style {
        CitationStyle::Vancouver => format!("[{}]", s.id),
        CitationStyle::Harvard | CitationStyle::Apa => {
            let names: Vec<Name> = r.authors.iter().map(|a| Name::parse(a)).collect();
            let who = match names.len() {
                0 => r.venue.clone().or(r.publisher.clone()).unwrap_or_else(|| short_title(&r.title)),
                1 => names[0].family.clone(),
                2 => {
                    let and = if style == CitationStyle::Apa { "&" } else { "and" };
                    format!("{} {and} {}", names[0].family, names[1].family)
                }
                _ => format!("{} et al.", names[0].family),
            };
            let year = r.year().map(|y| y.to_string()).unwrap_or_else(|| "n.d.".into());
            format!("({who}, {year})")
        }
    }
}

/// BibTeX entries for papers (for Zotero etc.).
pub fn bibtex(papers: &[Paper]) -> String {
    let refs: Vec<Ref> = papers.iter().map(Ref::from_paper).collect();
    bibtex_refs(&refs)
}

/// BibTeX entries for article sources (e.g. a project's bibliography built from articles).
pub fn bibtex_sources(sources: &[Source]) -> String {
    let refs: Vec<Ref> = sources.iter().map(Ref::from_source).collect();
    bibtex_refs(&refs)
}

/// RIS entries for papers (for Zotero etc.).
pub fn ris(papers: &[Paper]) -> String {
    let refs: Vec<Ref> = papers.iter().map(Ref::from_paper).collect();
    ris_refs(&refs)
}

/// RIS entries for article sources.
pub fn ris_sources(sources: &[Source]) -> String {
    let refs: Vec<Ref> = sources.iter().map(Ref::from_source).collect();
    ris_refs(&refs)
}

/// The stable BibTeX cite keys [`bibtex`] would use for these papers, in order.
pub fn cite_keys(papers: &[Paper]) -> Vec<String> {
    let refs: Vec<Ref> = papers.iter().map(Ref::from_paper).collect();
    keys_for(&refs)
}

// ---------------------------------------------------------------------------------------------
// Internal reference record
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RefKind {
    /// Journal / conference paper.
    Article,
    /// Preprint (arXiv etc.) without a journal.
    Preprint,
    News,
    Web,
}

#[derive(Clone, Debug)]
struct Ref {
    kind: RefKind,
    title: String,
    authors: Vec<String>,
    year: Option<i32>,
    date: Option<NaiveDate>,
    venue: Option<String>,
    publisher: Option<String>,
    url: Option<String>,
    doi: Option<String>,
    accessed: Option<NaiveDate>,
    abstract_text: Option<String>,
    arxiv_id: Option<String>,
}

fn clean(o: &Option<String>) -> Option<String> {
    o.as_ref().map(|s| collapse_ws(s)).filter(|s| !s.is_empty())
}

fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

impl Ref {
    fn from_source(s: &Source) -> Self {
        let venue = clean(&s.venue);
        let doi = clean(&s.doi).map(|d| normalize_doi(&d));
        let kind = match s.kind {
            SourceKind::News => RefKind::News,
            SourceKind::Web => RefKind::Web,
            _ => {
                if venue.as_deref().is_some_and(is_preprint_venue) {
                    RefKind::Preprint
                } else if venue.is_some() || doi.is_some() {
                    RefKind::Article
                } else if s.url.is_some() {
                    RefKind::Web
                } else {
                    RefKind::Article
                }
            }
        };
        Ref {
            kind,
            title: collapse_ws(&s.title),
            authors: s.authors.iter().map(|a| collapse_ws(a)).filter(|a| !a.is_empty()).collect(),
            year: s.year,
            date: s.date,
            venue,
            publisher: clean(&s.publisher),
            url: clean(&s.url),
            doi,
            accessed: s.accessed,
            abstract_text: None,
            arxiv_id: None,
        }
    }

    fn from_paper(p: &Paper) -> Self {
        let venue = clean(&p.venue);
        let doi = clean(&p.doi).map(|d| normalize_doi(&d));
        let kind = if p.kind == ContentKind::News {
            RefKind::News
        } else if venue.as_deref().is_some_and(is_preprint_venue) || (venue.is_none() && p.arxiv_id.is_some()) {
            RefKind::Preprint
        } else if venue.is_some() || doi.is_some() {
            RefKind::Article
        } else if p.url.is_some() {
            RefKind::Web
        } else {
            RefKind::Article
        };
        Ref {
            kind,
            title: collapse_ws(&p.title),
            authors: p.authors.iter().map(|a| collapse_ws(a)).filter(|a| !a.is_empty()).collect(),
            year: p.year,
            date: p.date,
            venue,
            publisher: clean(&p.publisher),
            url: clean(&p.url).or_else(|| clean(&p.oa_url)),
            doi,
            accessed: None,
            abstract_text: clean(&p.abstract_text),
            arxiv_id: clean(&p.arxiv_id),
        }
    }

    fn year(&self) -> Option<i32> {
        self.year.or(self.date.map(|d| d.year()))
    }

    /// DOI link if there is a DOI, else the URL.
    fn link(&self) -> Option<String> {
        self.doi.as_ref().map(|d| format!("https://doi.org/{d}")).or_else(|| self.url.clone())
    }

    fn names(&self) -> Vec<Name> {
        self.authors.iter().map(|a| Name::parse(a)).collect()
    }
}

fn is_preprint_venue(v: &str) -> bool {
    let l = v.to_lowercase();
    ["arxiv", "biorxiv", "medrxiv", "chemrxiv", "ssrn", "preprint", "research square"].iter().any(|p| l.contains(p))
}

// ---------------------------------------------------------------------------------------------
// Names
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
struct Name {
    family: String,
    /// Given names (may already be initials like "J." or "J.A.").
    given: Vec<String>,
    suffix: Option<String>,
    /// Organisation / group author: printed as is.
    org: bool,
}

const PARTICLES: &[&str] = &[
    "van", "von", "de", "der", "den", "del", "della", "di", "da", "du", "le", "la", "dos", "das", "ter", "ten", "bin",
    "al", "el", "st.", "zu",
];
const SUFFIXES: &[&str] = &["jr", "jr.", "sr", "sr.", "ii", "iii", "iv"];
const ORG_WORDS: &[&str] = &[
    "organization", "organisation", "agency", "institute", "group", "consortium", "collaboration", "team", "university",
    "association", "society", "council", "committee", "department", "ministry", "office", "network", "foundation",
    "centre", "center", "commission", "inc", "ltd", "news", "staff", "reporters", "project", "initiative",
];

impl Name {
    fn parse(raw: &str) -> Name {
        let raw = collapse_ws(raw);
        let lower = raw.to_lowercase();
        let looks_org = lower.split(|c: char| !c.is_alphanumeric()).any(|w| ORG_WORDS.contains(&w));
        if let Some((last, first)) = raw.split_once(',') {
            let last = last.trim();
            let mut first = first.trim().to_string();
            let mut suffix = None;
            // "Last, Jr., First" or "Last, First, Jr."
            if let Some((a, b)) = first.split_once(',') {
                let (a, b) = (a.trim().to_string(), b.trim().to_string());
                if SUFFIXES.contains(&a.to_lowercase().as_str()) {
                    suffix = Some(a);
                    first = b;
                } else if SUFFIXES.contains(&b.to_lowercase().as_str()) {
                    suffix = Some(b);
                    first = a;
                }
            }
            if !last.is_empty() && !looks_org {
                return Name {
                    family: last.to_string(),
                    given: first.split_whitespace().map(str::to_string).collect(),
                    suffix,
                    org: false,
                };
            }
        }
        let mut tokens: Vec<&str> = raw.split_whitespace().collect();
        if tokens.is_empty() {
            return Name { family: String::new(), given: vec![], suffix: None, org: true };
        }
        if looks_org || tokens.len() > 5 {
            return Name { family: raw.clone(), given: vec![], suffix: None, org: true };
        }
        let mut suffix = None;
        if tokens.len() > 2
            && let Some(last) = tokens.last()
            && SUFFIXES.contains(&last.trim_end_matches(',').to_lowercase().as_str())
        {
            suffix = Some(last.trim_end_matches(',').to_string());
            tokens.pop();
        }
        if tokens.len() == 1 {
            return Name { family: tokens[0].to_string(), given: vec![], suffix, org: false };
        }
        // Family name = last token plus any lowercase particles right before it.
        let mut start = tokens.len() - 1;
        while start > 1 && PARTICLES.contains(&tokens[start - 1].to_lowercase().as_str()) {
            start -= 1;
        }
        let family = tokens[start..].join(" ");
        let given = tokens[..start].iter().map(|s| s.trim_end_matches(',').to_string()).collect();
        Name { family, given, suffix, org: false }
    }

    /// Initial letters of the given names, hyphenated names keep their hyphen ("Jean-Paul" → J-P).
    fn initials(&self) -> Vec<String> {
        let mut out = Vec::new();
        for g in &self.given {
            // "J.A." → J, A
            let parts: Vec<&str> = g.split('.').map(str::trim).filter(|p| !p.is_empty()).collect();
            for p in parts {
                let hy: Vec<String> = p
                    .split('-')
                    .filter_map(|h| h.chars().find(|c| c.is_alphabetic()))
                    .map(|c| c.to_uppercase().collect::<String>())
                    .collect();
                if !hy.is_empty() {
                    out.push(hy.join("-"));
                }
            }
        }
        out
    }

    /// "Smith, J.A." (Harvard).
    fn harvard(&self) -> String {
        if self.org {
            return self.family.clone();
        }
        let ini: String = self.initials().iter().map(|i| format!("{}.", i.replace('-', ".-"))).collect();
        let mut s = if ini.is_empty() { self.family.clone() } else { format!("{}, {}", self.family, ini) };
        if let Some(x) = &self.suffix {
            s.push_str(&format!(", {x}"));
        }
        s
    }

    /// "Smith, J. A." (APA).
    fn apa(&self) -> String {
        if self.org {
            return self.family.clone();
        }
        let ini: Vec<String> = self.initials().iter().map(|i| format!("{}.", i.replace('-', ".-"))).collect();
        let mut s = if ini.is_empty() { self.family.clone() } else { format!("{}, {}", self.family, ini.join(" ")) };
        if let Some(x) = &self.suffix {
            s.push_str(&format!(", {x}"));
        }
        s
    }

    /// "Smith JA" (Vancouver).
    fn vancouver(&self) -> String {
        if self.org {
            return self.family.clone();
        }
        let ini: String = self.initials().iter().map(|i| i.replace('-', "")).collect();
        let mut s = if ini.is_empty() { self.family.clone() } else { format!("{} {}", self.family, ini) };
        if let Some(x) = &self.suffix {
            s.push_str(&format!(" {}", x.trim_end_matches('.')));
        }
        s
    }

    /// "Smith, John A." for BibTeX / RIS.
    fn last_first(&self) -> String {
        if self.org || self.given.is_empty() {
            return self.family.clone();
        }
        match &self.suffix {
            Some(x) => format!("{}, {}, {}", self.family, x, self.given.join(" ")),
            None => format!("{}, {}", self.family, self.given.join(" ")),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------------------------

/// Append `p` unless the text already ends in sentence punctuation.
fn end(s: &str, p: char) -> String {
    let t = s.trim_end();
    if t.ends_with(['.', '?', '!']) { t.to_string() } else { format!("{t}{p}") }
}

fn short_title(t: &str) -> String {
    let words: Vec<&str> = t.split_whitespace().take(4).collect();
    let mut s = words.join(" ");
    if t.split_whitespace().count() > 4 {
        s.push('…');
    }
    s
}

const MONTHS: [&str; 12] = [
    "January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November",
    "December",
];

fn month_name(d: NaiveDate) -> &'static str {
    MONTHS.get(d.month0() as usize).copied().unwrap_or("")
}

/// "25 November 2026".
pub fn long_date(d: NaiveDate) -> String {
    format!("{} {} {}", d.day(), month_name(d), d.year())
}

fn short_month(d: NaiveDate) -> &'static str {
    month_name(d).get(..3).unwrap_or("")
}

fn join_and(names: &[String], and: &str) -> String {
    match names.len() {
        0 => String::new(),
        1 => names[0].clone(),
        n => format!("{} {and} {}", names[..n - 1].join(", "), names[n - 1]),
    }
}

// ---------------------------------------------------------------------------------------------
// Harvard (Cite Them Right style)
// ---------------------------------------------------------------------------------------------

fn harvard_authors(r: &Ref) -> Option<String> {
    let names = r.names();
    let f: Vec<String> = names.iter().map(Name::harvard).collect();
    match f.len() {
        0 => None,
        1..=3 => Some(join_and(&f, "and")),
        _ => Some(format!("{} et al.", f[0])),
    }
}

fn harvard(r: &Ref) -> String {
    let year = r.year().map(|y| y.to_string()).unwrap_or_else(|| "n.d.".into());
    let authors = harvard_authors(r);
    let title = if r.title.is_empty() { "Untitled".to_string() } else { r.title.clone() };
    let mut s = String::new();
    match r.kind {
        RefKind::Article | RefKind::Preprint => {
            match &authors {
                Some(a) => s.push_str(&format!("{a} ({year}) '{title}'")),
                None => s.push_str(&format!("'{title}' ({year})")),
            }
            match (&r.venue, &r.publisher) {
                (Some(v), _) => s.push_str(&format!(", {v}.")),
                (None, Some(p)) => s.push_str(&format!(". {p}.")),
                (None, None) => s.push('.'),
            }
        }
        RefKind::News => {
            let outlet = r.venue.clone().or(r.publisher.clone());
            match (&authors, &outlet) {
                (Some(a), _) => s.push_str(&format!("{a} ({year}) '{title}'")),
                (None, Some(o)) => s.push_str(&format!("{o} ({year}) '{title}'")),
                (None, None) => s.push_str(&format!("'{title}' ({year})")),
            }
            if authors.is_some()
                && let Some(o) = &outlet
            {
                s.push_str(&format!(", {o}"));
            }
            if let Some(d) = r.date {
                s.push_str(&format!(", {} {}", d.day(), month_name(d)));
            }
            s.push('.');
        }
        RefKind::Web => {
            let who = authors.clone().or(r.publisher.clone()).or(r.venue.clone());
            match &who {
                Some(w) => s.push_str(&format!("{w} ({year}) {}", end(&title, '.'))),
                None => s.push_str(&format!("{} ({year})", end(&title, '.'))),
            }
            if !s.ends_with(['.', '?', '!']) {
                s.push('.');
            }
        }
    }
    if let Some(link) = r.link() {
        s.push_str(&format!(" Available at: {link}"));
        if let Some(acc) = r.accessed {
            s.push_str(&format!(" (Accessed: {})", long_date(acc)));
        }
        s.push('.');
    }
    s
}

// ---------------------------------------------------------------------------------------------
// APA 7
// ---------------------------------------------------------------------------------------------

fn apa_authors(r: &Ref) -> Option<String> {
    let f: Vec<String> = r.names().iter().map(Name::apa).collect();
    match f.len() {
        0 => None,
        1 => Some(f[0].clone()),
        2 => Some(format!("{}, & {}", f[0], f[1])),
        n if n <= 20 => Some(format!("{}, & {}", f[..n - 1].join(", "), f[n - 1])),
        n => Some(format!("{}, . . . {}", f[..19].join(", "), f[n - 1])),
    }
}

fn apa(r: &Ref) -> String {
    let year = r.year().map(|y| y.to_string()).unwrap_or_else(|| "n.d.".into());
    let date_part = match (r.kind, r.date) {
        (RefKind::News | RefKind::Web, Some(d)) => format!("{}, {} {}", d.year(), month_name(d), d.day()),
        _ => year,
    };
    let title = if r.title.is_empty() { "Untitled".to_string() } else { r.title.clone() };
    let authors = apa_authors(r).or_else(|| {
        // Web pages by an organisation: the organisation is the author.
        if r.kind == RefKind::Web { r.publisher.clone() } else { None }
    });
    let mut s = match &authors {
        Some(a) => format!("{} ({date_part}). {}", end(a, '.'), end(&title, '.')),
        None => format!("{} ({date_part}).", end(&title, '.')),
    };
    let container = match r.kind {
        RefKind::Article | RefKind::News => r.venue.clone().or(r.publisher.clone()),
        RefKind::Preprint => r.venue.clone().or(Some("Preprint".into())),
        RefKind::Web => {
            let site = r.venue.clone().or(r.publisher.clone());
            // Omit the site name when it repeats the author.
            if site.is_some() && site == authors { None } else { site }
        }
    };
    if let Some(c) = container {
        s.push_str(&format!(" {}", end(&c, '.')));
    }
    if let Some(link) = r.link() {
        s.push_str(&format!(" {link}"));
    }
    s
}

// ---------------------------------------------------------------------------------------------
// Vancouver (ICMJE / NLM)
// ---------------------------------------------------------------------------------------------

fn vancouver_authors(r: &Ref) -> Option<String> {
    let f: Vec<String> = r.names().iter().map(Name::vancouver).collect();
    match f.len() {
        0 => None,
        1..=6 => Some(f.join(", ")),
        _ => Some(format!("{}, et al", f[..6].join(", "))),
    }
}

fn vancouver(r: &Ref) -> String {
    let year = r.year().map(|y| y.to_string()).unwrap_or_else(|| "[date unknown]".into());
    let title = if r.title.is_empty() { "Untitled".to_string() } else { r.title.clone() };
    let mut s = String::new();
    if let Some(a) = vancouver_authors(r) {
        s.push_str(&end(&a, '.'));
        s.push(' ');
    }
    let cited = r.accessed.map(|d| format!(" [cited {} {} {}]", d.year(), short_month(d), d.day())).unwrap_or_default();
    match r.kind {
        RefKind::Article | RefKind::Preprint => {
            s.push_str(&end(&title, '.'));
            match (&r.venue, &r.publisher) {
                (Some(v), _) => s.push_str(&format!(" {}", end(v, '.'))),
                (None, Some(p)) => s.push_str(&format!(" {}", end(p, '.'))),
                _ => {}
            }
            s.push_str(&format!(" {year}."));
            if let Some(d) = &r.doi {
                s.push_str(&format!(" doi:{d}"));
            } else if let Some(u) = &r.url {
                s.push_str(&format!(" Available from: {u}"));
            }
        }
        RefKind::News => {
            s.push_str(&end(&title, '.'));
            let outlet = r.venue.clone().or(r.publisher.clone());
            if let Some(o) = outlet {
                s.push_str(&format!(" {} [Internet].", o.trim_end_matches('.')));
            }
            let date = r.date.map(|d| format!("{} {} {}", d.year(), short_month(d), d.day())).unwrap_or(year);
            s.push_str(&format!(" {date}{cited}."));
            if let Some(u) = r.link() {
                s.push_str(&format!(" Available from: {u}"));
            }
        }
        RefKind::Web => {
            s.push_str(&format!("{} [Internet].", title.trim_end_matches('.')));
            if let Some(p) = r.publisher.clone().or(r.venue.clone()) {
                s.push_str(&format!(" {}; {year}{cited}.", p.trim_end_matches('.')));
            } else {
                s.push_str(&format!(" {year}{cited}."));
            }
            if let Some(u) = r.link() {
                s.push_str(&format!(" Available from: {u}"));
            }
        }
    }
    s
}

// ---------------------------------------------------------------------------------------------
// BibTeX
// ---------------------------------------------------------------------------------------------

/// Fold common accented Latin letters to ASCII for cite keys.
fn fold_ascii(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        let r = match c {
            'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ą' | 'ă' => "a",
            'æ' => "ae",
            'ç' | 'ć' | 'č' => "c",
            'ď' | 'đ' | 'ð' => "d",
            'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ę' | 'ě' => "e",
            'ì' | 'í' | 'î' | 'ï' | 'ī' | 'ı' => "i",
            'ł' | 'ľ' => "l",
            'ñ' | 'ń' | 'ň' => "n",
            'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'ő' => "o",
            'œ' => "oe",
            'ř' => "r",
            'ś' | 'š' | 'ş' => "s",
            'ß' => "ss",
            'ť' | 'ţ' => "t",
            'ù' | 'ú' | 'û' | 'ü' | 'ū' | 'ů' | 'ű' => "u",
            'ý' | 'ÿ' => "y",
            'ź' | 'ż' | 'ž' => "z",
            'þ' => "th",
            _ => {
                let l: String = c.to_lowercase().collect();
                out.push_str(&l);
                continue;
            }
        };
        out.push_str(r);
    }
    out
}

fn key_word(s: &str) -> String {
    fold_ascii(s).chars().filter(|c| c.is_ascii_alphanumeric()).collect()
}

const STOP: &[&str] = &[
    "a", "an", "the", "on", "of", "in", "for", "and", "to", "from", "with", "by", "at", "is", "are", "via", "towards",
    "toward", "how", "what", "why", "do", "does",
];

fn base_key(r: &Ref) -> String {
    let who = r
        .names()
        .first()
        .map(|n| {
            if n.org {
                n.family.split_whitespace().find(|w| !STOP.contains(&w.to_lowercase().as_str())).map(key_word).unwrap_or_default()
            } else {
                // Use the last word of the family name ("van der Berg" → "berg"), like Zotero.
                n.family.split_whitespace().last().map(key_word).unwrap_or_default()
            }
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "anon".into());
    let year = r.year().map(|y| y.to_string()).unwrap_or_else(|| "nd".into());
    let word = r
        .title
        .split(|c: char| c.is_whitespace() || c == '-' || c == ':')
        .map(key_word)
        .find(|w| !w.is_empty() && !STOP.contains(&w.as_str()))
        .unwrap_or_default();
    format!("{who}{year}{word}")
}

fn keys_for(refs: &[Ref]) -> Vec<String> {
    let base: Vec<String> = refs.iter().map(base_key).collect();
    let mut out = Vec::with_capacity(base.len());
    for (i, k) in base.iter().enumerate() {
        let total = base.iter().filter(|b| *b == k).count();
        if total > 1 {
            let nth = base[..i].iter().filter(|b| *b == k).count();
            out.push(format!("{k}{}", suffix_letters(nth)));
        } else {
            out.push(k.clone());
        }
    }
    out
}

/// 0 → a, 25 → z, 26 → aa, ...
fn suffix_letters(mut n: usize) -> String {
    let mut s = Vec::new();
    loop {
        s.push((b'a' + (n % 26) as u8) as char);
        if n < 26 {
            break;
        }
        n = n / 26 - 1;
    }
    s.iter().rev().collect()
}

/// Escape text for a BibTeX field value in braces.
pub fn bibtex_escape(s: &str) -> String {
    let mut out = String::new();
    for c in collapse_ws(s).chars() {
        match c {
            '\\' => out.push_str("\\textbackslash{}"),
            '{' => out.push_str("\\{"),
            '}' => out.push_str("\\}"),
            '&' => out.push_str("\\&"),
            '%' => out.push_str("\\%"),
            '$' => out.push_str("\\$"),
            '#' => out.push_str("\\#"),
            '_' => out.push_str("\\_"),
            '~' => out.push_str("\\textasciitilde{}"),
            '^' => out.push_str("\\textasciicircum{}"),
            _ => out.push(c),
        }
    }
    out
}

fn bibtex_refs(refs: &[Ref]) -> String {
    let keys = keys_for(refs);
    let mut out = String::new();
    for (r, key) in refs.iter().zip(keys) {
        let entry_type = match r.kind {
            RefKind::Article => "article",
            RefKind::Preprint | RefKind::News | RefKind::Web => "misc",
        };
        let mut fields: Vec<(&str, String)> = Vec::new();
        if !r.authors.is_empty() {
            let names: Vec<String> = r
                .names()
                .iter()
                .map(|n| if n.org { format!("{{{}}}", bibtex_escape(&n.family)) } else { bibtex_escape(&n.last_first()) })
                .collect();
            fields.push(("author", names.join(" and ")));
        }
        fields.push(("title", bibtex_escape(&r.title)));
        match r.kind {
            RefKind::Article => {
                if let Some(v) = &r.venue {
                    fields.push(("journal", bibtex_escape(v)));
                }
            }
            RefKind::Preprint => {
                if let Some(v) = &r.venue {
                    fields.push(("howpublished", bibtex_escape(v)));
                }
                if let Some(a) = &r.arxiv_id {
                    fields.push(("eprint", bibtex_escape(a)));
                    fields.push(("archiveprefix", "arXiv".into()));
                }
            }
            RefKind::News => {
                if let Some(o) = r.venue.as_ref().or(r.publisher.as_ref()) {
                    fields.push(("howpublished", bibtex_escape(o)));
                }
                fields.push(("note", "News article".into()));
            }
            RefKind::Web => {
                if let Some(o) = r.venue.as_ref() {
                    fields.push(("howpublished", bibtex_escape(o)));
                }
            }
        }
        if let Some(p) = &r.publisher
            && r.kind != RefKind::News
        {
            fields.push(("publisher", bibtex_escape(p)));
        }
        if let Some(y) = r.year() {
            fields.push(("year", y.to_string()));
        }
        if let Some(d) = r.date {
            fields.push(("date", d.format("%Y-%m-%d").to_string()));
        }
        if let Some(d) = &r.doi {
            fields.push(("doi", bibtex_escape(d)));
        }
        if let Some(u) = &r.url {
            // URLs are verbatim in biblatex; only braces need care.
            fields.push(("url", u.replace(['{', '}'], "")));
        }
        if let Some(a) = r.accessed {
            fields.push(("urldate", a.format("%Y-%m-%d").to_string()));
        }
        if let Some(ab) = &r.abstract_text {
            fields.push(("abstract", bibtex_escape(ab)));
        }
        out.push_str(&format!("@{entry_type}{{{key},\n"));
        let n = fields.len();
        for (i, (k, v)) in fields.into_iter().enumerate() {
            out.push_str(&format!("  {k} = {{{v}}}{}\n", if i + 1 < n { "," } else { "" }));
        }
        out.push_str("}\n\n");
    }
    out
}

// ---------------------------------------------------------------------------------------------
// RIS
// ---------------------------------------------------------------------------------------------

fn ris_line(out: &mut String, tag: &str, value: &str) {
    let v = collapse_ws(value);
    if !v.is_empty() {
        out.push_str(&format!("{tag}  - {v}\r\n"));
    }
}

fn ris_refs(refs: &[Ref]) -> String {
    let mut out = String::new();
    for r in refs {
        let ty = match r.kind {
            RefKind::News => "NEWS",
            RefKind::Article if r.venue.is_some() => "JOUR",
            RefKind::Article => "GEN",
            RefKind::Preprint => "UNPB",
            RefKind::Web => "ELEC",
        };
        ris_line(&mut out, "TY", ty);
        for n in r.names() {
            ris_line(&mut out, "AU", &n.last_first());
        }
        ris_line(&mut out, "TI", &r.title);
        if let Some(v) = &r.venue {
            ris_line(&mut out, "T2", v);
            if r.kind == RefKind::Article {
                ris_line(&mut out, "JO", v);
            }
        }
        if let Some(y) = r.year() {
            ris_line(&mut out, "PY", &y.to_string());
        }
        if let Some(d) = r.date {
            ris_line(&mut out, "DA", &d.format("%Y/%m/%d").to_string());
        }
        if let Some(p) = &r.publisher {
            ris_line(&mut out, "PB", p);
        }
        if let Some(d) = &r.doi {
            ris_line(&mut out, "DO", d);
        }
        if let Some(u) = &r.url {
            ris_line(&mut out, "UR", u);
        } else if let Some(l) = r.link() {
            ris_line(&mut out, "UR", &l);
        }
        if let Some(a) = r.accessed {
            ris_line(&mut out, "Y2", &a.format("%Y/%m/%d").to_string());
        }
        if let Some(ab) = &r.abstract_text {
            ris_line(&mut out, "AB", ab);
        }
        out.push_str("ER  - \r\n\r\n");
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    fn src(authors: &[&str]) -> Source {
        Source {
            id: 1,
            kind: SourceKind::Paper,
            paper_key: None,
            title: "CRISPR base editing in vivo".into(),
            authors: authors.iter().map(|s| s.to_string()).collect(),
            year: Some(2024),
            venue: Some("Nature".into()),
            publisher: None,
            url: Some("https://example.org/x".into()),
            doi: Some("10.1038/xyz".into()),
            accessed: Some(d(2026, 11, 25)),
            date: None,
        }
    }

    #[test]
    fn names_parse_both_orders() {
        let a = Name::parse("John Adam Smith");
        assert_eq!(a.family, "Smith");
        assert_eq!(a.initials(), vec!["J", "A"]);
        let b = Name::parse("Smith, John A.");
        assert_eq!(b.family, "Smith");
        assert_eq!(b.harvard(), "Smith, J.A.");
        assert_eq!(b.apa(), "Smith, J. A.");
        assert_eq!(b.vancouver(), "Smith JA");
        let c = Name::parse("Ludwig van Beethoven");
        assert_eq!(c.family, "van Beethoven");
        let e = Name::parse("Jean-Paul Sartre");
        assert_eq!(e.harvard(), "Sartre, J.-P.");
        assert_eq!(e.vancouver(), "Sartre JP");
        let o = Name::parse("World Health Organization");
        assert!(o.org);
        assert_eq!(o.harvard(), "World Health Organization");
        let j = Name::parse("Martin Luther King Jr.");
        assert_eq!(j.family, "King");
        assert_eq!(j.harvard(), "King, M.L., Jr.");
    }

    #[test]
    fn harvard_author_counts() {
        let one = reference(&src(&["John Smith"]), CitationStyle::Harvard);
        assert_eq!(
            one,
            "Smith, J. (2024) 'CRISPR base editing in vivo', Nature. Available at: https://doi.org/10.1038/xyz (Accessed: 25 November 2026)."
        );
        let two = reference(&src(&["John Smith", "Ann Doe"]), CitationStyle::Harvard);
        assert!(two.starts_with("Smith, J. and Doe, A. (2024) 'CRISPR"), "{two}");
        let three = reference(&src(&["John Smith", "Ann Doe", "Kim Lee"]), CitationStyle::Harvard);
        assert!(three.starts_with("Smith, J., Doe, A. and Lee, K. (2024)"), "{three}");
        let four = reference(&src(&["John Smith", "Ann Doe", "Kim Lee", "Bo Wu"]), CitationStyle::Harvard);
        assert!(four.starts_with("Smith, J. et al. (2024)"), "{four}");
        let zero = reference(&src(&[]), CitationStyle::Harvard);
        assert!(zero.starts_with("'CRISPR base editing in vivo' (2024), Nature."), "{zero}");
        let mut ny = src(&["John Smith"]);
        ny.year = None;
        assert!(reference(&ny, CitationStyle::Harvard).contains("(n.d.)"));
    }

    #[test]
    fn apa_author_counts() {
        let one = reference(&src(&["Smith, John A."]), CitationStyle::Apa);
        assert_eq!(one, "Smith, J. A. (2024). CRISPR base editing in vivo. Nature. https://doi.org/10.1038/xyz");
        let two = reference(&src(&["John Smith", "Ann Doe"]), CitationStyle::Apa);
        assert!(two.starts_with("Smith, J., & Doe, A. (2024)."), "{two}");
        let three = reference(&src(&["John Smith", "Ann Doe", "Kim Lee"]), CitationStyle::Apa);
        assert!(three.starts_with("Smith, J., Doe, A., & Lee, K. (2024)."), "{three}");
        let many: Vec<String> = (0..22).map(|i| format!("Author{i} Person")).collect();
        let refs: Vec<&str> = many.iter().map(String::as_str).collect();
        let big = reference(&src(&refs), CitationStyle::Apa);
        assert!(big.contains(". . . Person, A."), "{big}");
        let zero = reference(&src(&[]), CitationStyle::Apa);
        assert!(zero.starts_with("CRISPR base editing in vivo. (2024). Nature."), "{zero}");
        let mut ny = src(&["John Smith"]);
        ny.year = None;
        assert!(reference(&ny, CitationStyle::Apa).contains("(n.d.)."));
    }

    #[test]
    fn vancouver_author_counts() {
        let one = reference(&src(&["John Smith"]), CitationStyle::Vancouver);
        assert_eq!(one, "Smith J. CRISPR base editing in vivo. Nature. 2024. doi:10.1038/xyz");
        let three = reference(&src(&["John Smith", "Ann Doe", "Kim Lee"]), CitationStyle::Vancouver);
        assert!(three.starts_with("Smith J, Doe A, Lee K. CRISPR"), "{three}");
        let seven: Vec<String> = (0..7).map(|i| format!("Ann B{i}")).collect();
        let refs: Vec<&str> = seven.iter().map(String::as_str).collect();
        let v = reference(&src(&refs), CitationStyle::Vancouver);
        assert!(v.starts_with("B0 A, B1 A, B2 A, B3 A, B4 A, B5 A, et al. CRISPR"), "{v}");
        let mut ny = src(&[]);
        ny.year = None;
        let z = reference(&ny, CitationStyle::Vancouver);
        assert!(z.starts_with("CRISPR base editing in vivo. Nature. [date unknown]."), "{z}");
    }

    #[test]
    fn news_and_web_sources() {
        let news = Source {
            id: 2,
            kind: SourceKind::News,
            title: "Probe lands on comet".into(),
            authors: vec!["Jane Roe".into()],
            year: None,
            venue: Some("The Guardian".into()),
            url: Some("https://news.example/probe".into()),
            date: Some(d(2026, 11, 20)),
            accessed: Some(d(2026, 11, 25)),
            ..Default::default()
        };
        let h = reference(&news, CitationStyle::Harvard);
        assert_eq!(
            h,
            "Roe, J. (2026) 'Probe lands on comet', The Guardian, 20 November. Available at: https://news.example/probe (Accessed: 25 November 2026)."
        );
        let a = reference(&news, CitationStyle::Apa);
        assert_eq!(a, "Roe, J. (2026, November 20). Probe lands on comet. The Guardian. https://news.example/probe");
        let v = reference(&news, CitationStyle::Vancouver);
        assert!(v.contains("The Guardian [Internet]. 2026 Nov 20 [cited 2026 Nov 25]."), "{v}");
        let mut anon = news.clone();
        anon.authors.clear();
        assert!(reference(&anon, CitationStyle::Harvard).starts_with("The Guardian (2026) 'Probe lands on comet', 20 November."));

        let web = Source {
            id: 3,
            kind: SourceKind::Web,
            title: "Comet".into(),
            publisher: Some("Wikipedia".into()),
            year: Some(2025),
            url: Some("https://en.wikipedia.org/wiki/Comet".into()),
            accessed: Some(d(2026, 11, 25)),
            ..Default::default()
        };
        assert_eq!(
            reference(&web, CitationStyle::Harvard),
            "Wikipedia (2025) Comet. Available at: https://en.wikipedia.org/wiki/Comet (Accessed: 25 November 2026)."
        );
        assert_eq!(reference(&web, CitationStyle::Apa), "Wikipedia. (2025). Comet. https://en.wikipedia.org/wiki/Comet");
        assert!(reference(&web, CitationStyle::Vancouver).starts_with("Comet [Internet]. Wikipedia; 2025 [cited 2026 Nov 25]."));
    }

    fn paper(title: &str, authors: &[&str], year: Option<i32>) -> Paper {
        Paper {
            title: title.into(),
            authors: authors.iter().map(|s| s.to_string()).collect(),
            year,
            venue: Some("Cell & Molecular Biology".into()),
            doi: Some("10.1/abc_def".into()),
            ..Default::default()
        }
    }

    #[test]
    fn bibtex_keys_and_escaping() {
        let ps = vec![
            paper("CRISPR screens at 100% scale", &["John Smith", "Ann Doe"], Some(2024)),
            paper("The CRISPR toolbox", &["Smith, Jane"], Some(2024)),
            paper("On Müller cells", &["Björn Müller"], None),
        ];
        let keys = cite_keys(&ps);
        assert_eq!(keys, vec!["smith2024crispra", "smith2024crisprb", "mullerndmuller"]);
        let b = bibtex(&ps);
        assert!(b.contains("@article{smith2024crispra,"));
        assert!(b.contains("author = {Smith, John and Doe, Ann}"));
        assert!(b.contains("title = {CRISPR screens at 100\\% scale}"));
        assert!(b.contains("journal = {Cell \\& Molecular Biology}"));
        assert!(b.contains("doi = {10.1/abc\\_def}"));
        assert_eq!(b.matches('{').count(), b.matches('}').count(), "braces balanced");
        // stable
        assert_eq!(b, bibtex(&ps));
    }

    #[test]
    fn ris_output() {
        let mut news = paper("Probe lands", &["Jane Roe"], Some(2026));
        news.kind = ContentKind::News;
        news.venue = Some("BBC News".into());
        news.doi = None;
        news.url = Some("https://bbc.example/p".into());
        let web = Paper { title: "Some page".into(), url: Some("https://x.example".into()), ..Default::default() };
        let ps = vec![paper("A study", &["John Smith"], Some(2024)), news, web];
        let r = ris(&ps);
        let entries: Vec<&str> = r.split("ER  - ").collect();
        assert_eq!(entries.len(), 4);
        assert!(entries[0].starts_with("TY  - JOUR\r\n"));
        assert!(entries[0].contains("AU  - Smith, John\r\n"));
        assert!(entries[0].contains("DO  - 10.1/abc_def\r\n"));
        assert!(entries[1].contains("TY  - NEWS\r\n"));
        assert!(entries[2].contains("TY  - ELEC\r\n"));
        assert!(entries[2].contains("UR  - https://x.example\r\n"));
    }

    #[test]
    fn in_text_citations() {
        assert_eq!(in_text(&src(&["John Smith"]), CitationStyle::Harvard), "(Smith, 2024)");
        assert_eq!(in_text(&src(&["John Smith", "Ann Doe"]), CitationStyle::Apa), "(Smith & Doe, 2024)");
        assert_eq!(in_text(&src(&["A B", "C D", "E F"]), CitationStyle::Harvard), "(B et al., 2024)");
        assert_eq!(in_text(&src(&[]), CitationStyle::Vancouver), "[1]");
    }
}
