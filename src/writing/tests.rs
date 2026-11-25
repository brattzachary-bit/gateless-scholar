//! Offline tests of the whole pipeline with a fake provider, plus `#[ignore]` live checks.

use std::sync::{Arc, Mutex};

use futures::future::BoxFuture;
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};

use super::*;
use crate::providers::{CompletionRequest, ModelProvider};

/// Returns canned text chosen by a closure; streams it in small deltas.
struct FakeProvider {
    small: bool,
    reply: Box<dyn Fn(&CompletionRequest) -> String + Send + Sync>,
    calls: Mutex<Vec<CompletionRequest>>,
}

impl FakeProvider {
    fn new(small: bool, reply: impl Fn(&CompletionRequest) -> String + Send + Sync + 'static) -> Arc<Self> {
        Arc::new(FakeProvider { small, reply: Box::new(reply), calls: Mutex::new(vec![]) })
    }
}

impl ModelProvider for FakeProvider {
    fn brand(&self) -> String {
        "Fake".into()
    }
    fn describe(&self) -> String {
        "Fake provider (test)".into()
    }
    fn is_small(&self) -> bool {
        self.small
    }
    fn complete<'a>(&'a self, req: &'a CompletionRequest, deltas: Option<UnboundedSender<String>>) -> BoxFuture<'a, anyhow::Result<String>> {
        Box::pin(async move {
            self.calls.lock().unwrap().push(req.clone());
            let text = (self.reply)(req);
            if let Some(tx) = deltas {
                let chars: Vec<char> = text.chars().collect();
                for c in chars.chunks(7) {
                    let _ = tx.send(c.iter().collect());
                    tokio::task::yield_now().await;
                }
            }
            Ok(text)
        })
    }
}

fn target() -> Paper {
    Paper {
        key: "doi:10.1/xyz".into(),
        doi: Some("10.1/xyz".into()),
        title: "Cryogenic electron tomography of bacterial flagellar motors".into(),
        abstract_text: Some("We imaged 120 flagellar motors in situ using cryo-electron tomography and found a ring of 34 subunits.".into()),
        authors: vec!["A. Author".into(), "B. Author".into()],
        year: Some(2024),
        venue: Some("Journal of Motors".into()),
        url: Some("https://example.org/paper".into()),
        ..Default::default()
    }
}

fn pack() -> SourcePack {
    let t = target();
    let mut related = t.clone();
    related.key = "doi:10.1/rel".into();
    related.title = "Flagellar motor structure review".into();
    let mut citing = t.clone();
    citing.key = "doi:10.1/cit".into();
    citing.title = "A paper that cites the motor study".into();
    SourcePack {
        sources: vec![Source::from_paper(1, SourceKind::Target, &t), Source::from_paper(2, SourceKind::Paper, &related), Source::from_paper(3, SourceKind::Citing, &citing)],
        chunks: vec![
            Chunk { source_id: 1, text: t.abstract_text.clone().unwrap(), section: Some("Abstract".into()), score: 3.0 },
            Chunk { source_id: 2, text: "Motors in different species have between 26 and 34 subunits in the C-ring.".into(), section: None, score: 2.0 },
            Chunk { source_id: 3, text: "Author et al. showed the motor's ring has 34 subunits, which explains its torque.".into(), section: None, score: 1.0 },
        ],
        full_text: None,
        images: vec![
            ImageRef { url: "https://commons.wikimedia.org/a.jpg".into(), title: "Motor".into(), licence: "CC BY 4.0".into(), embeddable: true, ..Default::default() },
            ImageRef { url: "https://commons.wikimedia.org/b.jpg".into(), title: "Bacterium".into(), licence: "CC BY 4.0".into(), embeddable: true, ..Default::default() },
        ],
    }
}

const STRONG_ARTICLE: &str = "Here is the article.\n\n## Overview\n\nBacteria swim with a rotary motor [1]. The motor is tiny but powerful [9].\n\n\
## Background\n\nMotors vary between species, with 26 to 34 subunits [2].\n\n```chart\n{\"kind\":\"bar\",\"title\":\"Subunits\",\"series\":[{\"name\":\"n\",\"points\":[[\"min\",26],[\"max\",34]]}],\"citations\":[2]}\n```\n\n\
## What the paper reports\n\nThe authors imaged 120 motors and report a ring of 34 subunits [1]. The motor spins at 1,000 revolutions per second.\n\n\
## References\n\n[1] Author, A. et al. (2024) Cryogenic electron tomography. Journal of Motors.\n[9] Made Up, B. (2020) Invented paper, Fake Journal.\n";

#[tokio::test]
async fn strong_interpretation_streams_validated_sections() {
    let provider = FakeProvider::new(false, |req| {
        if req.json {
            r#"{"title": "How bacteria build their rotary motors", "standfirst": "A look at motor structure.", "focus": ["intro"]}"#.to_string()
        } else {
            STRONG_ARTICLE.to_string()
        }
    });
    let (tx, mut rx) = unbounded_channel();
    let req = ArticleRequest { kind: ArticleKind::Interpretation, target: Some(target()), topic: target().title, brief: None, length: Length::Journal, allow_tools: false };
    let article = write_article(provider.clone(), req, pack(), tx).await.expect("article");

    assert_eq!(article.kind, ArticleKind::Interpretation);
    assert_eq!(article.title, "AI interpretation: How bacteria build their rotary motors");
    assert_eq!(article.original_title.as_deref(), Some(target().title.as_str()));
    assert_eq!(article.original_authors, target().authors);
    assert_eq!(article.byline, "Gateless Scholar + Fake");
    assert_eq!(article.hero_image.as_ref().unwrap().title, "Motor");
    let headings: Vec<&str> = article.sections.iter().map(|s| s.heading.as_str()).collect();
    assert_eq!(headings, vec!["Overview", "Background", "What the paper reports"], "reference list must be dropped");
    assert!(article.passages().all(|p| p.origin == Origin::AiWritten));
    assert!(article.passages().all(|p| p.citations.iter().all(|c| [1, 2, 3].contains(c))));
    assert_eq!(article.validation.invalid_citations_removed, 1);
    // "The motor is tiny but powerful [9]." lost its only citation; "spins at 1,000 …" is uncited.
    assert_eq!(article.validation.unverified_sentences, 1 + 1);
    assert_eq!(article.validation.charts_rejected, 0);
    assert!(article.sections[1].blocks.iter().any(|b| matches!(b, Block::Chart(_))));
    assert!(article.sections[1].blocks.iter().any(|b| matches!(b, Block::Image(i) if i.title == "Bacterium")));
    assert_eq!(article.sources.len(), 3);
    assert!(!article.plain_text().contains("Invented"));

    // Events: stages, deltas, and sections streamed in order before completion.
    let mut sections = vec![];
    let mut deltas = 0;
    let mut stages = vec![];
    while let Ok(ev) = rx.try_recv() {
        match ev {
            WriteEvent::Section(s) => sections.push(s.heading),
            WriteEvent::Delta(_) => deltas += 1,
            WriteEvent::Stage { text, .. } => stages.push(text),
        }
    }
    assert_eq!(sections, vec!["Overview", "Background", "What the paper reports"]);
    assert!(deltas > 10);
    assert_eq!(stages.first().map(String::as_str), Some("Planning sections…"));
    assert!(stages.iter().any(|s| s == "Checking citations…"));
    assert_eq!(provider.calls.lock().unwrap().len(), 2, "outline + one article call");
}

#[tokio::test]
async fn small_model_writes_section_by_section() {
    let provider = FakeProvider::new(true, |req| {
        if req.json {
            r#"{"title": "Motors", "standfirst": "S.", "sections": [{"heading": "What's new", "focus": "x"}, {"heading": "Key findings", "focus": "y"}, {"heading": "References", "focus": "z"}]}"#.to_string()
        } else if req.prompt.contains("\"Key findings\"") {
            "## Key findings\n\nThe ring has 34 subunits [1, 3].".to_string()
        } else {
            "What's new:\nMotors were imaged in situ [1].".to_string()
        }
    });
    let (tx, _rx) = unbounded_channel();
    let req = ArticleRequest { kind: ArticleKind::AiOnly, target: None, topic: "Bacterial motors".into(), brief: Some("Brief.".into()), length: Length::OnePage, allow_tools: false };
    let article = write_article(provider.clone(), req, pack(), tx).await.expect("article");
    assert_eq!(article.title, "Motors");
    assert_eq!(article.kind, ArticleKind::AiOnly);
    assert_eq!(article.sections.len(), 2);
    assert_eq!(article.sections[1].heading, "Key findings");
    let ps: Vec<&Passage> = article.passages().collect();
    assert_eq!(ps[0].text.trim(), "Motors were imaged in situ.");
    assert_eq!(ps[1].citations, vec![1, 3]);
    // outline + 2 sections (the references section from the outline is dropped)
    assert_eq!(provider.calls.lock().unwrap().len(), 3);
}

#[tokio::test]
async fn full_text_rewording_marks_origins() {
    let provider = FakeProvider::new(false, |req| {
        // Lightly reword the first paragraph, mangle the second.
        let _ = req;
        "@@1@@\nThe motors were imaged at 4 K using cryo-electron tomography in intact cells. This revealed the full ring.\n\n@@2@@\nSomething completely different that has nothing to do with the original text at all.".to_string()
    });
    let mut p = pack();
    p.full_text = Some(FullText {
        url: "https://example.org/oa.pdf".into(),
        licence: Some("CC BY".into()),
        version: Some("publishedVersion".into()),
        sections: vec![FullTextSection {
            heading: "Results".into(),
            paragraphs: vec![
                "The motors were imaged at 4 K with cryo-electron tomography in intact cells. This revealed the full ring.".into(),
                "Each ring contained 34 subunits arranged symmetrically around the central rod of the motor complex.".into(),
                "Short one.".into(),
            ],
        }],
    });
    let (tx, _rx) = unbounded_channel();
    let req = ArticleRequest { kind: ArticleKind::FullText, target: Some(target()), topic: target().title, brief: None, length: Length::Journal, allow_tools: false };
    let a = write_article(provider, req, p, tx).await.expect("article");
    assert_eq!(a.kind, ArticleKind::FullText);
    assert_eq!(a.title, target().title);
    assert_eq!(a.sections[0].heading, "Results");
    let paras: Vec<&Vec<Passage>> = a.sections[0].blocks.iter().filter_map(|b| if let Block::Paragraph(p) = b { Some(p) } else { None }).collect();
    assert_eq!(paras.len(), 3);
    assert_eq!(paras[0][0].origin, Origin::Reworded);
    assert_eq!(paras[0][0].original.as_deref(), Some("The motors were imaged at 4 K with cryo-electron tomography in intact cells."));
    assert_eq!(paras[0][1].origin, Origin::Original);
    assert!(paras[1].iter().all(|p| p.origin == Origin::Original));
    assert_eq!(paras[1][0].text, "Each ring contained 34 subunits arranged symmetrically around the central rod of the motor complex.");
    assert_eq!(paras[2][0].origin, Origin::Original);
    assert_eq!(a.validation.rewordings_reverted, 1);
}

#[tokio::test]
async fn full_text_without_text_falls_back_to_interpretation() {
    let provider = FakeProvider::new(false, |req| if req.json { "{}".into() } else { "## Overview\nMotors are made of rings [1].".into() });
    let (tx, _rx) = unbounded_channel();
    let req = ArticleRequest { kind: ArticleKind::FullText, target: Some(target()), topic: target().title, brief: None, length: Length::OnePage, allow_tools: false };
    let a = write_article(provider, req, pack(), tx).await.unwrap();
    assert_eq!(a.kind, ArticleKind::Interpretation);
    assert!(a.title.starts_with("AI interpretation: "));
    assert_ne!(a.title, format!("AI interpretation: {}", target().title).replace("the topic of ", "x"));
}

#[tokio::test]
async fn garbage_model_output_does_not_panic() {
    let provider = FakeProvider::new(false, |_| "```\n{{{[[[ ]]] @@@ ## \n```chart\n{bad json".into());
    let (tx, _rx) = unbounded_channel();
    let req = ArticleRequest { kind: ArticleKind::News, target: None, topic: "Event".into(), brief: None, length: Length::OnePage, allow_tools: false };
    let r = write_article(provider, req, pack(), tx).await;
    assert!(r.is_err(), "no usable text must be an error, not a panic or an empty article");
}

#[tokio::test]
async fn structured_helpers_parse_replies() {
    let papers: Vec<Paper> = (1..=5)
        .map(|i| Paper { key: format!("k{i}"), title: format!("Paper {i}"), abstract_text: Some(format!("Abstract {i}.")), ..Default::default() })
        .collect();
    let p = FakeProvider::new(false, |req| {
        if req.prompt.contains("article ideas") {
            r#"```json
{"articles": [{"headline": "Idea one", "summary": "Two papers agree [1].", "papers": [1, 3, 99]}, {"headline": "Lonely", "summary": "x", "papers": [2]}]}
```"#
                .into()
        } else if req.prompt.contains("Plan the search") {
            r#"{"queries": ["perovskite stability", "solar cell degradation"], "from_year": 2023, "to_year": null, "sort": "newest", "subject": null}"#.into()
        } else if req.prompt.contains("Papers found") {
            r#"{"top": [4, 2], "overview": "Paper four reports a record efficiency [4]. Paper five disagrees [5]. Invented claim [42]."}"#.into()
        } else if req.prompt.contains("same news event") {
            r#"{"headline": "Bridge reopens", "summary": "The bridge reopened on Monday."}"#.into()
        } else {
            r#"{"queries": ["solar", "Solar Cells", "photovoltaics"]}"#.into()
        }
    });
    let props = propose_ai_articles(p.clone(), "Tech", &papers, 3).await.unwrap();
    assert_eq!(props.len(), 1);
    assert_eq!(props[0].basis_keys, vec!["k1", "k3"]);
    assert_eq!(props[0].summary, "Two papers agree.");

    let plan = plan_search(p.clone(), "recent perovskite work").await.unwrap();
    assert_eq!(plan.queries.len(), 2);
    assert_eq!(plan.filters.from_year, Some(2023));
    assert_eq!(plan.filters.sort, SortBy::Newest);
    assert!(!plan.explanation.is_empty());

    let cur = curate_results(p.clone(), "perovskites", &papers).await.unwrap();
    assert_eq!(cur.top_keys, vec!["k4", "k2"]);
    assert_eq!(cur.sources.iter().map(|s| s.paper_key.clone().unwrap()).collect::<Vec<_>>(), vec!["k4", "k2", "k5"]);
    assert_eq!(cur.overview[0].citations, vec![1]);
    assert_eq!(cur.overview[1].citations, vec![3]);
    assert!(cur.overview[2].citations.is_empty());

    let (h, s) = news_headline(p.clone(), &papers).await.unwrap();
    assert_eq!((h.as_str(), s.as_str()), ("Bridge reopens", "The bridge reopened on Monday."));

    let q = expand_query(p.clone(), "solar").await;
    assert_eq!(q, vec!["Solar Cells", "photovoltaics"]);
}

#[tokio::test]
async fn answer_question_is_cited_and_validated() {
    let p = FakeProvider::new(false, |_| "The ring has 34 subunits [1]. A new source agrees [4].\n\nIt might also matter for drug design [7].".into());
    let mut article = Article { title: "AI interpretation: motors".into(), sources: pack().sources, ..Default::default() };
    article.sections.push(Section { heading: "Overview".into(), blocks: vec![Block::Paragraph(vec![Passage::ai("Motors spin.", vec![1])])] });
    let extra = SourcePack { sources: vec![Source { id: 4, title: "New".into(), ..Default::default() }], ..Default::default() };
    let a = answer_question(p, &article, "How many subunits?", extra, None).await.unwrap();
    assert_eq!(a.sources.len(), 4);
    assert_eq!(a.passages.len(), 3);
    assert_eq!(a.passages[1].citations, vec![4]);
    assert!(a.passages[1].text.ends_with("\n\n"));
    assert!(a.passages[2].citations.is_empty());
    assert!(a.passages.iter().all(|p| p.origin == Origin::AiWritten));
}

#[test]
fn marked_paragraph_parsing() {
    let v = parse_marked_paragraphs("@@1@@\nOne\ntwo.\n\n@@ 2 @@\nThree.");
    assert_eq!(v, vec![(1, "One two.".to_string()), (2, "Three.".to_string())]);
}

// ---------------------------------------------------------------------------------------------
// Live checks: `cargo test writing::tests::live -- --ignored --nocapture`
// ---------------------------------------------------------------------------------------------

fn live_pack() -> (Paper, SourcePack) {
    let t = Paper {
        key: "doi:10.1038/s41586-020-2649-2".into(),
        doi: Some("10.1038/s41586-020-2649-2".into()),
        title: "Array programming with NumPy".into(),
        abstract_text: Some(
            "Array programming provides a powerful, compact and expressive syntax for accessing, manipulating and operating on data in vectors, \
             matrices and higher-dimensional arrays. NumPy is the primary array programming library for the Python language. It has an essential \
             role in research analysis pipelines in fields as diverse as physics, chemistry, astronomy, geoscience, biology, psychology, materials \
             science, engineering, finance and economics. For example, in astronomy, NumPy was an important part of the software stack used in the \
             discovery of gravitational waves and in the first imaging of a black hole."
                .into(),
        ),
        authors: vec!["Charles R. Harris".into(), "K. Jarrod Millman".into(), "Stéfan J. van der Walt".into()],
        year: Some(2020),
        venue: Some("Nature".into()),
        ..Default::default()
    };
    let mk = |key: &str, title: &str, venue: &str, year: i32| Paper { key: key.into(), title: title.into(), venue: Some(venue.into()), year: Some(year), authors: vec!["A. Researcher".into()], ..Default::default() };
    let s2 = mk("k2", "SciPy 1.0: fundamental algorithms for scientific computing in Python", "Nature Methods", 2020);
    let s3 = mk("k3", "The NumPy array: a structure for efficient numerical computation", "Computing in Science & Engineering", 2011);
    let s4 = mk("k4", "Reproducible data analysis in astronomy with Python", "Astronomy and Computing", 2022);
    let pack = SourcePack {
        sources: vec![
            Source::from_paper(1, SourceKind::Target, &t),
            Source::from_paper(2, SourceKind::Paper, &s2),
            Source::from_paper(3, SourceKind::Paper, &s3),
            Source::from_paper(4, SourceKind::Citing, &s4),
        ],
        chunks: vec![
            Chunk { source_id: 1, text: t.abstract_text.clone().unwrap(), section: Some("Abstract".into()), score: 5.0 },
            Chunk { source_id: 2, text: "SciPy is an open-source scientific computing library for Python. Since its initial release in 2001, SciPy has become a de facto standard for leveraging scientific algorithms in Python, with over 600 unique code contributors and thousands of dependent packages.".into(), section: Some("Abstract".into()), score: 4.0 },
            Chunk { source_id: 3, text: "In the Python world, NumPy arrays are the standard representation for numerical data and enable efficient implementation of numerical computations in a high-level language. Performance can be improved by vectorizing calculations, avoiding copying data in memory, and minimizing operation counts.".into(), section: Some("Abstract".into()), score: 3.0 },
            Chunk { source_id: 4, text: "Harris et al. (2020) describe NumPy as the foundation of the scientific Python ecosystem; astronomy pipelines such as ours rely on its arrays for image reduction and catalogue cross-matching.".into(), section: None, score: 2.0 },
        ],
        full_text: None,
        images: vec![],
    };
    (t, pack)
}

async fn run_live(provider: SharedProvider) {
    let (t, pack) = live_pack();
    let (tx, mut rx) = unbounded_channel();
    let printer = tokio::spawn(async move {
        while let Some(ev) = rx.recv().await {
            match ev {
                WriteEvent::Stage { text, fraction } => println!("[stage {:.0}%] {text}", fraction * 100.0),
                WriteEvent::Section(s) => println!("[section] {}", s.heading),
                WriteEvent::Delta(_) => {}
            }
        }
    });
    let req = ArticleRequest { kind: ArticleKind::Interpretation, target: Some(t.clone()), topic: t.title.clone(), brief: None, length: Length::OnePage, allow_tools: false };
    let started = std::time::Instant::now();
    let a = write_article(provider, req, pack, tx).await.expect("live article");
    let _ = printer.await;
    println!("\n=== {} ({:.0}s) ===\n{}\n", a.title, started.elapsed().as_secs_f32(), a.standfirst.clone().unwrap_or_default());
    for s in &a.sections {
        println!("## {}", s.heading);
        for b in &s.blocks {
            match b {
                Block::Paragraph(ps) => {
                    let line: String = ps
                        .iter()
                        .map(|p| format!("{}{}{}", p.text.trim_end(), if p.citations.is_empty() { String::new() } else { format!(" {:?}", p.citations) }, if p.unverified { " (UNVERIFIED)" } else { "" }))
                        .collect::<Vec<_>>()
                        .join(" ");
                    println!("{line}\n");
                }
                other => println!("{other:?}\n"),
            }
        }
    }
    println!("validation: {:#?}", a.validation);
    let words: usize = a.plain_text().split_whitespace().count();
    println!("words: {words}");
    assert!(a.title.starts_with("AI interpretation: "));
    assert!(!a.sections.is_empty());
    assert!(a.passages().any(|p| !p.citations.is_empty()), "the model should cite at least once");
}

#[tokio::test]
#[ignore]
async fn live_write_article_ollama_mistral_nemo() {
    let mut cfg = crate::providers::ProviderConfig::new(crate::providers::ProviderKind::Ollama);
    cfg.model = "mistral-nemo:latest".into();
    cfg.small_model = true;
    run_live(Arc::new(crate::providers::ollama::OllamaProvider::new(cfg))).await;
}

#[tokio::test]
#[ignore]
async fn live_write_article_ollama_strong_path() {
    // The one-call (strong model) path, exercised with a local model.
    let mut cfg = crate::providers::ProviderConfig::new(crate::providers::ProviderKind::Ollama);
    cfg.model = std::env::var("GATELESS_LIVE_MODEL").unwrap_or_else(|_| "mistral-nemo:latest".into());
    cfg.small_model = false;
    run_live(Arc::new(crate::providers::ollama::OllamaProvider::new(cfg))).await;
}
