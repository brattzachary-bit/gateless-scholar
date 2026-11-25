//! Export tests. `sample_exports` writes a sample article as .txt/.docx/.pdf to
//! `$GATELESS_EXPORT_SAMPLES` (or the system temp dir) for eyeballing.

use std::io::Read as _;

use chrono::{NaiveDate, TimeZone, Utc};

use super::*;

fn d(y: i32, m: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, day).unwrap()
}

pub(crate) fn sample_article() -> Article {
    let sources = vec![
        Source {
            id: 1,
            kind: SourceKind::Target,
            title: "Base editing of PCSK9 lowers LDL cholesterol in primates".into(),
            authors: vec!["Musunuru, Kiran".into(), "Alexandra C. Chadwick".into(), "Taiji Mizoguchi".into(), "Sara P. Garcia".into()],
            year: Some(2021),
            venue: Some("Nature".into()),
            doi: Some("10.1038/s41586-021-03534-y".into()),
            accessed: Some(d(2026, 11, 25)),
            ..Default::default()
        },
        Source {
            id: 2,
            kind: SourceKind::Citing,
            title: "In vivo gene editing for cardiovascular disease: a review".into(),
            authors: vec!["Jane Roe".into(), "Ali Khan".into()],
            year: Some(2023),
            venue: Some("Circulation Research".into()),
            doi: Some("10.1161/circresaha.123.000001".into()),
            accessed: Some(d(2026, 11, 25)),
            ..Default::default()
        },
        Source {
            id: 3,
            kind: SourceKind::News,
            title: "Gene-editing trial cuts cholesterol in first patients".into(),
            authors: vec![],
            venue: Some("BBC News".into()),
            date: Some(d(2023, 11, 13)),
            year: Some(2023),
            url: Some("https://www.bbc.co.uk/news/health-0000000".into()),
            accessed: Some(d(2026, 11, 25)),
            ..Default::default()
        },
        Source {
            id: 4,
            kind: SourceKind::Web,
            title: "PCSK9".into(),
            publisher: Some("Wikipedia".into()),
            year: Some(2026),
            url: Some("https://en.wikipedia.org/wiki/PCSK9".into()),
            accessed: Some(d(2026, 11, 25)),
            ..Default::default()
        },
    ];
    let reworded = Passage {
        text: "Lowering PCSK9 levels reduces LDL cholesterol, a major cause of heart disease.".into(),
        origin: Origin::Reworded,
        citations: vec![1],
        original: Some("PCSK9 is a validated target for lowering LDL cholesterol, which is causally linked to coronary heart disease.".into()),
        unverified: false,
    };
    let mut long_ai = Vec::new();
    for i in 0..6 {
        long_ai.push(Passage::ai(
            format!(
                "Sentence {} of a longer AI-written discussion explains how lipid nanoparticles carry the editor's mRNA to liver cells, where the edit switches off the gene for good and keeps working without repeat doses.",
                i + 1
            ),
            vec![1 + (i % 2) as u32],
        ));
    }
    Article {
        id: "sample".into(),
        kind: ArticleKind::Interpretation,
        paper_key: Some("doi:10.1038/s41586-021-03534-y".into()),
        title: "AI interpretation: one-off gene editing to lower cholesterol".into(),
        standfirst: Some("What a single infusion of a base editor did to cholesterol levels in monkeys, and what it may mean for people.".into()),
        original_title: Some("In vivo CRISPR base editing of PCSK9 durably lowers cholesterol in primates".into()),
        original_authors: vec!["Kiran Musunuru".into(), "Alexandra C. Chadwick".into(), "Taiji Mizoguchi".into()],
        original_url: Some("https://doi.org/10.1038/s41586-021-03534-y".into()),
        byline: "Gateless Scholar + Claude".into(),
        generated_by: "Claude Code CLI (sonnet)".into(),
        created: Utc.with_ymd_and_hms(2026, 11, 25, 9, 30, 0).unwrap(),
        sections: vec![
            Section {
                heading: "Background".into(),
                blocks: vec![
                    Block::Paragraph(vec![
                        Passage::original("Heart disease remains the leading cause of death worldwide."),
                        reworded.clone(),
                        Passage::ai("Gene editing offers a way to make that reduction permanent with one treatment.", vec![2]),
                    ]),
                    Block::Paragraph(vec![
                        Passage::ai("The study tested this idea in cynomolgus monkeys, whose liver biology is close to ours.", vec![1]),
                        Passage { unverified: true, ..Passage::ai("Several companies are now racing to bring the approach to clinics.", vec![]) },
                    ]),
                    Block::Image(ImageRef {
                        url: "https://upload.wikimedia.org/wikipedia/commons/x/xx/PCSK9.png".into(),
                        page_url: "https://commons.wikimedia.org/wiki/File:PCSK9.png".into(),
                        title: "Structure of the PCSK9 protein".into(),
                        credit: "Emw".into(),
                        licence: "CC BY-SA 3.0".into(),
                        licence_url: Some("https://creativecommons.org/licenses/by-sa/3.0/".into()),
                        from_paper: false,
                        embeddable: true,
                        ..Default::default()
                    }),
                ],
            },
            Section {
                heading: "Results".into(),
                blocks: vec![
                    Block::Heading("Cholesterol levels".into()),
                    Block::Paragraph(long_ai),
                    Block::Chart(ChartSpec {
                        kind: ChartKind::Bar,
                        title: "Blood levels after one infusion (change from baseline)".into(),
                        x_label: Some("Measure".into()),
                        y_label: Some("Reduction, %".into()),
                        series: vec![
                            ChartSeries { name: "Low dose".into(), points: vec![("PCSK9".into(), 52.0), ("LDL".into(), 34.0)] },
                            ChartSeries { name: "High dose".into(), points: vec![("PCSK9".into(), 90.0), ("LDL".into(), 60.0)] },
                        ],
                        sets: vec![],
                        nodes: vec![],
                        edges: vec![],
                        citations: vec![1],
                    }),
                    Block::Chart(ChartSpec {
                        kind: ChartKind::Line,
                        title: "LDL over time (high dose)".into(),
                        x_label: Some("Weeks".into()),
                        y_label: Some("% of baseline".into()),
                        series: vec![ChartSeries {
                            name: "".into(),
                            points: vec![("0".into(), 100.0), ("2".into(), 55.0), ("4".into(), 42.0), ("8".into(), 40.0), ("24".into(), 41.0)],
                        }],
                        sets: vec![],
                        nodes: vec![],
                        edges: vec![],
                        citations: vec![1, 2],
                    }),
                    Block::Chart(ChartSpec {
                        kind: ChartKind::Venn,
                        title: "Animals by response".into(),
                        x_label: None,
                        y_label: None,
                        series: vec![],
                        sets: vec![
                            VennSet { label: "PCSK9 down >50%".into(), size: 10.0, overlaps: vec![(1, 7.0), (2, 3.0)] },
                            VennSet { label: "LDL down >50%".into(), size: 8.0, overlaps: vec![(2, 2.0)] },
                            VennSet { label: "Mild liver enzyme rise".into(), size: 4.0, overlaps: vec![] },
                        ],
                        nodes: vec![],
                        edges: vec![],
                        citations: vec![1],
                    }),
                    Block::Chart(ChartSpec {
                        kind: ChartKind::Flow,
                        title: "How the treatment works".into(),
                        x_label: None,
                        y_label: None,
                        series: vec![],
                        sets: vec![],
                        nodes: vec!["Infusion of lipid nanoparticles".into(), "Uptake by liver cells".into(), "Base edit in PCSK9 gene".into(), "Less PCSK9 protein".into(), "Lower LDL cholesterol".into()],
                        edges: vec![(0, 1, "blood".into()), (1, 2, "".into()), (2, 3, "".into()), (3, 4, "".into()), (2, 4, "permanent".into())],
                        citations: vec![1, 4],
                    }),
                    Block::Paragraph(vec![
                        Passage::original("The effect lasted for at least eight months after a single dose."),
                        Passage { citations: vec![1], ..reworded.clone() },
                    ]),
                ],
            },
            Section {
                heading: "What it means".into(),
                blocks: vec![Block::Paragraph(vec![Passage::ai(
                    "News coverage of the first human trial reported similar falls in cholesterol, though the trial was small.",
                    vec![3, 4],
                )])],
            },
        ],
        sources,
        hero_image: None,
        validation: ValidationReport {
            invalid_citations_removed: 1,
            unverified_sentences: 1,
            charts_rejected: 0,
            rewordings_reverted: 0,
            notes: vec!["Citation [9] did not match any source and was dropped.".into()],
        },
    }
}

#[test]
fn txt_marks_every_origin() {
    let a = sample_article();
    let t = to_txt(&a, CitationStyle::Harvard);
    assert!(t.starts_with("[ AI INTERPRETATION – NOT THE ORIGINAL PAPER ]"));
    assert!(t.contains("By Gateless Scholar + Claude"));
    assert!(t.contains("Generated by Claude Code CLI (sonnet) on 25 November 2026"));
    assert!(t.contains("Original paper: In vivo CRISPR base editing"));
    assert!(t.contains("https://doi.org/10.1038/s41586-021-03534-y"));
    assert!(t.contains("How the text is marked"));
    // Original plain, reworded wrapped with original, AI wrapped and barred.
    assert!(t.contains("  Heart disease remains the leading cause of death worldwide. ⟦reworded:"));
    assert!(t.contains("(original: \"PCSK9 is a validated target"));
    assert!(t.contains("⟦AI: Gene"));
    assert!(t.lines().any(|l| l.starts_with("▍ ") && l.contains("⟦AI:")));
    assert!(t.contains("[unverified]"));
    assert!(t.contains("[Figure 1] Blood levels"));
    assert!(t.contains("| PCSK9"));
    let flat = t.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(flat.contains("[Image] Structure of the PCSK9 protein. Credit: Emw. Licence: CC BY-SA 3.0. May not be from the paper."));
    assert!(t.contains("References (Harvard)"));
    assert!(t.contains("[1] Musunuru, K. et al. (2021)"));
    assert!(t.contains("Validation notes"));
    // No line (except URLs) longer than the wrap width.
    for l in t.lines() {
        assert!(l.chars().count() <= 80 || l.contains("http"), "too long: {l}");
    }
    // AI words never appear on an unbarred body line.
    for l in t.lines() {
        if l.contains("⟦AI:") {
            assert!(l.starts_with("▍ "), "AI text without margin bar: {l}");
        }
    }
}

fn zip_entries(bytes: &[u8]) -> Vec<(String, String)> {
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes)).expect("valid zip");
    let mut out = Vec::new();
    for i in 0..z.len() {
        let mut f = z.by_index(i).unwrap();
        let mut s = String::new();
        f.read_to_string(&mut s).unwrap();
        out.push((f.name().to_string(), s));
    }
    out
}

fn assert_well_formed(name: &str, xml: &str) {
    let mut r = quick_xml::Reader::from_str(xml);
    let mut depth: i64 = 0;
    loop {
        match r.read_event() {
            Ok(quick_xml::events::Event::Start(_)) => depth += 1,
            Ok(quick_xml::events::Event::End(_)) => depth -= 1,
            Ok(quick_xml::events::Event::Eof) => break,
            Ok(_) => {}
            Err(e) => panic!("{name} is not well-formed: {e}"),
        }
        assert!(depth >= 0, "{name}: unbalanced");
    }
    assert_eq!(depth, 0, "{name}: unclosed elements");
}

#[test]
fn docx_is_valid_zip_with_well_formed_xml() {
    let a = sample_article();
    let bytes = docx_bytes(&a, CitationStyle::Apa).unwrap();
    let entries = zip_entries(&bytes);
    let names: Vec<&str> = entries.iter().map(|(n, _)| n.as_str()).collect();
    for req in ["[Content_Types].xml", "_rels/.rels", "word/document.xml", "word/_rels/document.xml.rels", "word/styles.xml", "word/footnotes.xml"] {
        assert!(names.contains(&req), "missing {req}");
    }
    for (n, x) in &entries {
        assert_well_formed(n, x);
    }
    let doc = &entries.iter().find(|(n, _)| n == "word/document.xml").unwrap().1;
    assert!(doc.contains("AI interpretation – not the original paper"));
    assert!(doc.contains("w:color w:val=\"7B3FBF\""), "AI purple");
    assert!(doc.contains("<w:left w:val=\"single\" w:sz=\"24\""), "AI margin bar");
    assert!(doc.contains("<w:u w:val=\"single\"/>"), "reworded underline");
    assert!(doc.contains("<w:footnoteReference w:id=\"1\"/>"));
    assert!(doc.contains("w:anchor=\"ref_1\""));
    assert!(doc.contains("w:name=\"ref_4\""));
    assert!(doc.contains("[unverified]"));
    assert!(doc.contains("Figure 1: Blood levels"));
    assert!(doc.contains("References (APA 7)"));
    let notes = &entries.iter().find(|(n, _)| n == "word/footnotes.xml").unwrap().1;
    assert!(notes.contains("Original wording: “PCSK9 is a validated target"));
    let rels = &entries.iter().find(|(n, _)| n == "word/_rels/document.xml.rels").unwrap().1;
    assert!(rels.contains("https://doi.org/10.1038/s41586-021-03534-y"));
    // Every r:id used in the document has a relationship.
    let re = regex::Regex::new(r#"r:id="([^"]+)""#).unwrap();
    for cap in re.captures_iter(doc) {
        assert!(rels.contains(&format!("Id=\"{}\"", &cap[1])), "dangling {}", &cap[1]);
    }
}

#[test]
fn pdf_is_written_with_pages() {
    let a = sample_article();
    let (bytes, pages) = pdf_bytes(&a, CitationStyle::Vancouver).unwrap();
    assert!(bytes.starts_with(b"%PDF-1.4"));
    assert!(bytes.ends_with(b"%%EOF\n"));
    assert!((2..=8).contains(&pages), "pages: {pages}");
    let s = String::from_utf8_lossy(&bytes);
    assert_eq!(s.matches("/Type /Page ").count(), pages);
    assert!(s.contains("/URI (https://doi.org/10.1038/s41586-021-03534-y)"));
    assert!(s.contains("/Dest ["), "internal citation links");
    assert!(s.contains(&format!("(Page 1 of {pages})")));
    // A longer article grows the page count.
    let mut big = a.clone();
    for _ in 0..5 {
        big.sections.extend(a.sections.clone());
    }
    let (_, big_pages) = pdf_bytes(&big, CitationStyle::Harvard).unwrap();
    assert!(big_pages > pages * 3, "{big_pages} vs {pages}");
    // Empty article still produces a valid one-page PDF.
    let (e, ep) = pdf_bytes(&Article::default(), CitationStyle::Harvard).unwrap();
    assert!(e.starts_with(b"%PDF") && ep == 1);
}

#[test]
fn mailto_building_and_truncation() {
    let url = mailto_url("AI article: Hi & bye", "Line one\nLine 2 ⟦AI: x⟧", None);
    assert_eq!(url, "mailto:?subject=AI%20article%3A%20Hi%20%26%20bye&body=Line%20one%0D%0ALine%202%20%E2%9F%A6AI%3A%20x%E2%9F%A7");
    let a = sample_article();
    let body = to_txt(&a, CitationStyle::Harvard);
    assert!(body.chars().count() > EMAIL_BODY_LIMIT);
    let p = std::path::Path::new("/tmp/full.txt");
    let truncated = truncate_body(&body, Some(p));
    assert!(truncated.chars().count() <= EMAIL_BODY_LIMIT, "{}", truncated.chars().count());
    assert!(truncated.contains("Full article attached as text file: /tmp/full.txt"));
    assert!(truncated.starts_with("[ AI INTERPRETATION"));
    let draft = prepare_email(&a, CitationStyle::Harvard).unwrap();
    let file = draft.full_text_file.expect("long article saved to a file");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), body);
    assert!(draft.url.starts_with("mailto:?subject=AI%20interpretation%3A"));
    let _ = std::fs::remove_file(file);
    // Short article: no file.
    let short = Article { title: "T".into(), ..Default::default() };
    let draft = prepare_email(&short, CitationStyle::Harvard).unwrap();
    assert!(draft.full_text_file.is_none());
}

#[test]
fn print_pdf_is_written() {
    let p = print_pdf_path(&sample_article(), CitationStyle::Harvard).unwrap();
    assert!(std::fs::metadata(&p).unwrap().len() > 1000);
    let _ = std::fs::remove_file(p);
}

#[test]
fn file_stems() {
    assert_eq!(file_stem("AI interpretation: one-off gene editing!"), "ai-interpretation-one-off-gene-editing");
    assert_eq!(file_stem("???"), "article");
}

#[test]
fn sample_exports() {
    let dir = std::env::var_os("GATELESS_EXPORT_SAMPLES")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("gateless-export-samples"));
    std::fs::create_dir_all(&dir).unwrap();
    let a = sample_article();
    std::fs::write(dir.join("sample.txt"), to_txt(&a, CitationStyle::Harvard)).unwrap();
    to_docx(&a, CitationStyle::Harvard, &dir.join("sample.docx")).unwrap();
    to_pdf(&a, CitationStyle::Harvard, &dir.join("sample.pdf")).unwrap();
    let mut full = a.clone();
    full.kind = ArticleKind::FullText;
    to_pdf(&full, CitationStyle::Apa, &dir.join("sample-fulltext-apa.pdf")).unwrap();
}
