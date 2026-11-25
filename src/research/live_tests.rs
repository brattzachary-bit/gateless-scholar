//! Live tests against the real APIs. Run with `cargo test research -- --ignored --nocapture`.
//! They use a throwaway database in a temp directory.

use tokio::sync::mpsc::unbounded_channel;

use super::*;

fn research() -> Research {
    let _ = env_logger::builder().is_test(true).try_init();
    let dir = std::env::temp_dir().join(format!("gateless-live-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let db = Db::open(&dir).expect("open db");
    Research::new(db, ResearchConfig { news_feeds: crate::settings::default_news_feeds(), ..Default::default() })
}

fn progress_printer() -> UnboundedSender<Progress> {
    let (tx, mut rx) = unbounded_channel::<Progress>();
    tokio::spawn(async move {
        while let Some(p) = rx.recv().await {
            println!("  [{:>3.0}%] {}", p.fraction * 100.0, p.text);
        }
    });
    tx
}

fn show_pack(pack: &SourcePack) {
    println!("sources: {}", pack.sources.len());
    for s in &pack.sources {
        println!("  [{}] {:?} {} ({:?}) {}", s.id, s.kind, s.title, s.venue, s.link().unwrap_or_default());
    }
    println!("chunks: {}", pack.chunks.len());
    for c in pack.chunks.iter().take(5) {
        println!("  <{}> {:.2} [{}] {}…", c.source_id, c.score, c.section.as_deref().unwrap_or("-"), c.text.chars().take(120).collect::<String>());
    }
    if let Some(ft) = &pack.full_text {
        println!("full text: {} ({} sections, {} words, licence {:?})", ft.url, ft.sections.len(), text::word_count(&ft.sections), ft.licence);
        for s in ft.sections.iter().take(6) {
            println!("  § {} ({} paras)", s.heading, s.paragraphs.len());
        }
    }
    for i in &pack.images {
        println!("image: {} [{}] embeddable={} from_paper={} {}", i.title, i.licence, i.embeddable, i.from_paper, i.url);
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn research_live_search() {
    let r = research();
    let t = std::time::Instant::now();
    let q = SearchQuery { text: "CRISPR base editing".into(), per_page: 10, ..Default::default() };
    let res = r.search(&q).await.expect("search");
    println!("{} results in {:?}", res.len(), t.elapsed());
    for p in res.iter().take(12) {
        println!("  {:?} {} ({:?}) {:?} cites={:?} {:?} abs={}", p.kind, p.title, p.year, p.found_in, p.cited_by_count, p.oa_url, p.abstract_text.is_some());
    }
    assert!(res.len() >= 5);
    let q2 = SearchQuery {
        text: "graph neural networks".into(),
        per_page: 10,
        filters: SearchFilters { from_year: Some(2023), free_full_text_only: true, sort: SortBy::MostCited, ..Default::default() },
        ..Default::default()
    };
    let res2 = r.search(&q2).await.expect("search2");
    println!("filtered: {}", res2.len());
    for p in res2.iter().take(5) {
        println!("  {:?} {} {:?} cites={:?}", p.kind, p.title, p.year, p.cited_by_count);
    }
    assert!(res2.iter().all(|p| p.kind == ContentKind::FreeFullText && p.year.unwrap_or(0) >= 2023));
    let news = r.search(&SearchQuery { text: "climate summit".into(), scope: SearchScope::News, ..Default::default() }).await.expect("news search");
    println!("news: {}", news.len());
    for p in news.iter().take(5) {
        println!("  {} — {:?} {:?}", p.title, p.venue, p.date);
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn research_live_gather_paper() {
    let r = research();
    let t = std::time::Instant::now();
    // "Attention Is All You Need" (arXiv, many citing papers).
    let mut p = Paper { title: "Attention Is All You Need".into(), arxiv_id: Some("1706.03762".into()), ..Default::default() };
    dedupe::finalize(&mut p);
    let pack = r.gather_for_paper(&p, &[], progress_printer()).await.expect("gather");
    println!("gather_for_paper in {:?}", t.elapsed());
    show_pack(&pack);
    assert_eq!(pack.sources[0].kind, SourceKind::Target);
    assert!(pack.sources.iter().any(|s| s.kind == SourceKind::Citing) || pack.sources.len() > 3);
    assert!(pack.chunks.len() >= 5);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn research_live_gather_paper_doi() {
    let r = research();
    let t = std::time::Instant::now();
    // A PLOS paper (open access, in PMC).
    let mut p = Paper { title: "Why Most Published Research Findings Are False".into(), doi: Some("10.1371/journal.pmed.0020124".into()), ..Default::default() };
    dedupe::finalize(&mut p);
    let pack = r.gather_for_paper(&p, &["replication crisis".into()], progress_printer()).await.expect("gather");
    println!("gather_for_paper (doi) in {:?}", t.elapsed());
    show_pack(&pack);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn research_live_news() {
    let r = research();
    let t = std::time::Instant::now();
    let clusters = r.news_clusters("Space", today() - chrono::Duration::days(3), 6).await.expect("clusters");
    println!("{} clusters in {:?}", clusters.len(), t.elapsed());
    for c in &clusters {
        println!("  {} ({} articles; image {:?})", c.title, c.articles.len(), c.image.as_ref().map(|i| (&i.licence, i.embeddable)));
        for a in c.articles.iter().take(3) {
            println!("     - {} [{:?}]", a.title, a.venue);
        }
    }
    let Some(c) = clusters.first() else { panic!("no clusters") };
    let t = std::time::Instant::now();
    let pack = r.gather_news(&c.title, &c.articles, progress_printer()).await.expect("gather_news");
    println!("gather_news in {:?}", t.elapsed());
    show_pack(&pack);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn research_live_misc() {
    let r = research();
    let img = r.find_image("black hole", None).await.expect("image");
    println!("image: {img:#?}");
    let img = img.expect("an image");
    let bytes = r.image_bytes(img.thumb_url.as_deref().unwrap_or(&img.url)).await.expect("bytes");
    println!("{} bytes", bytes.len());
    let t = std::time::Instant::now();
    let recent = r.recent_papers("Astrophysics", today() - chrono::Duration::days(7), 10).await.expect("recent");
    println!("recent ({:?}):", t.elapsed());
    for p in &recent {
        println!("  {:?} {} [{:?}]", p.date, p.title, p.kind);
    }
    let foryou = r.recent_papers("For You", today() - chrono::Duration::days(7), 10).await.expect("recent");
    println!("for you: {}", foryou.len());
    for p in foryou.iter().take(5) {
        println!("  {:?} {}", p.date, p.title);
    }
    let follow = r.new_for_follow(&Follow::Author("Jennifer Doudna".into()), today() - chrono::Duration::days(120)).await.expect("follow");
    println!("follow: {}", follow.len());
    for p in follow.iter().take(3) {
        println!("  {:?} {}", p.date, p.title);
    }
    let q = r
        .gather_for_question("How does the transformer's self-attention scale with sequence length?", &[Source { id: 1, title: "Attention Is All You Need".into(), ..Default::default() }])
        .await
        .expect("question");
    show_pack(&q);
    assert!(q.sources.iter().all(|s| s.id >= 2));
    let mut ps = vec![Paper { doi: Some("10.1038/s41586-020-2649-2".into()), title: "Array programming with NumPy".into(), ..Default::default() }];
    r.enrich_open_access(&mut ps).await;
    println!("enriched: {:?} {:?} {:?}", ps[0].kind, ps[0].oa_url, ps[0].oa_status);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn research_live_gdelt() {
    let r = research();
    let t = std::time::Instant::now();
    let res = r.gdelt_search(&news::gdelt_query("NASA asteroid crater", 3), Some(today() - chrono::Duration::days(7)), 20).await;
    match &res {
        Ok(v) => println!("{} GDELT articles in {:?}; first: {:?}", v.len(), t.elapsed(), v.first().map(|p| (&p.title, &p.venue))),
        Err(e) => println!("GDELT error after {:?}: {e:#}", t.elapsed()),
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn research_live_image_timing() {
    let r = research();
    let mut p = Paper { title: "Why Most Published Research Findings Are False".into(), doi: Some("10.1371/journal.pmed.0020124".into()), pmcid: Some("PMC1182327".into()), ..Default::default() };
    dedupe::finalize(&mut p);
    let t = std::time::Instant::now();
    let ft = r.europepmc_fulltext("PMC1182327").await;
    let ft = ft.ok().flatten();
    println!("epmc in {:?}: licence {:?} figs {:?}", t.elapsed(), ft.as_ref().map(|f| f.licence.clone()), ft.as_ref().map(|f| f.figures.len()));
    for q in ["Published Research Findings False", "Research Findings", "Published"] {
        let t = std::time::Instant::now();
        let imgs = r.commons_search(q, 8).await;
        println!("commons '{q}' in {:?}: {:?}", t.elapsed(), imgs.map(|v| v.iter().map(|i| (i.title.clone(), i.embeddable)).collect::<Vec<_>>()));
    }
    let t = std::time::Instant::now();
    let imgs = r.find_images(&p.title, Some(&p), 2, true).await;
    println!("find_images in {:?}: {:?}", t.elapsed(), imgs.iter().map(|i| &i.title).collect::<Vec<_>>());
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn research_live_pmc_figures() {
    let r = research();
    let ft = r.europepmc_fulltext("PMC1182327").await.ok().flatten().expect("epmc");
    println!("licence {:?}, {} figures: {:?}", ft.licence, ft.figures.len(), ft.figures.iter().map(|f| (&f.url, f.embeddable)).collect::<Vec<_>>());
    let page = r.get_page("https://pmc.ncbi.nlm.nih.gov/articles/PMC1182327/").await;
    println!("page: {:?}", page.as_ref().map(|b| (b.len(), String::from_utf8_lossy(&b[..b.len().min(200)]).to_string())).map_err(|e| format!("{e:#}")));
    let figs = r.resolve_pmc_figures("PMC1182327", ft.figures).await;
    println!("resolved: {:?}", figs.iter().map(|f| &f.url).collect::<Vec<_>>());
}
