# Gateless Scholar – Claude Code Build Brief

## Overview

Build **Gateless Scholar** ("research, ungated"; short name/binary/repo: `gateless`), an open-source native Rust desktop app for Linux (developed on Nobara/KDE). It gives people without university logins a way into research: a Google Scholar-style search, a Perplexity Discover-style daily feed of research and news, and AI-written, fully cited articles. Its key focus is papers that only have a free abstract: the app writes its own clearly labelled interpretation, built from real sources the reader can click through. It must always be honest about what is original and what is AI-generated. Everything described here is version one; build it in stages (see Milestones) so each part works before the next.

## Platform and stack

- Rust. GUI: use the **Artcraft** project's egui-based GUI library directly (open source on GitHub; check the exact crate name). Otherwise egui/eframe.
- Artcraft's WordCraft, PDF editor and mail client are **references only**: borrow applicable code where it fits, but build our own features. Check licences; if any GPL code is used, license Gateless Scholar under GPL.
- Linux only for now. Prefer cross-platform crates over Linux-specific ones, and leave `// TODO(windows):` comments wherever Windows support will need work. Do not implement Windows yet.
- Must look and feel like native desktop software, not a website: custom title bar, right-click menus, keyboard shortcuts, resizable/collapsible panes.
- Modern, clean, Perplexity-like look. Sans-serif fonts. Light and dark mode (dark default).
- Storage: SQLite (rusqlite or sqlx). HTTP: reqwest + tokio. Secrets in the system keyring. Packaging: AppImage and/or Flatpak.
- No background service or launch on startup.

## Layout

- Top-left: logo. Top: search bar ("What do you want to research?").
- Under the search bar: topic tabs (e.g. Tech, Space, Chemistry, Biology, Astrophysics, Computer Vision). Users can add, remove and reorder tabs.
- Two discover views: **Discover Research** (default home) and **Discover News**.
- Hideable left sidebar: Favourites, Read Later, Bookshelves, Projects, Notes, History, Generation Queue, Notifications, Bibliography, Settings. Keep it uncluttered (group items, collapse sections).

## Feed cards

- Image on the left; title on the right; one or two lines of abstract with "Show more" to expand the full abstract; authors below.
- Images: real openly licensed images (Wikimedia Commons, open-access figures). If the paper has none, find the closest relevant image. Clicking opens it full screen with its source and licence shown, noting it may not be from the paper. Copyrighted photos (e.g. National Geographic) are link-only, not embedded.
- Badges: "Free full text", "Abstract only – full text AI-assisted", or "AI article".
- Authors: real papers show the original authors. Fully AI-written articles show "Gateless Scholar + \<user's AI provider, e.g. Claude>".
- Actions: favourite, read later (also queues full generation), add to bookshelf/project, share.
- Infinite scroll per tab. Discover mixes real papers and AI-only articles, clearly labelled, with an optional toggle to hide AI-only articles.

## Daily Discover (research and news)

- When the app is opened each day, generate that day's Discover Research and Discover News **headlines and short abstracts only**, in a background task with a loading bar; cards appear as they are written. Search and everything else stays usable immediately.
- Full articles are only generated when clicked or added to Read Later.
- Keep past days' headlines (a week or more, size-limited) so older items can be opened and generated later.

## Search

- Two modes: **Scholar mode** (list of matching papers, like Google Scholar) and **AI mode** (AI curates the newest and most relevant research at the top, then the normal scrolling list below).
- Search journals and news separately (scope selector).
- Filters for Scholar mode and lists: date range, free full text only, content type (real / abstract-only / AI article / news), subject, sort by newest, most cited or relevance. AI mode chooses these automatically.

## Article reader

- Opening a paper shows the abstract immediately, plus a progress indicator with an estimated time (e.g. "Finding sources… reading 8 papers… writing"). Once writing starts, stream it in section by section.
- If a legal free full text exists (open access, preprint, author version): show it, with limited AI rewording for flow.
- If only the abstract exists: the AI writes its own journal-length interpretation of the topic from the abstract, citing papers, related open sources and how citing papers describe the work. Label it "AI interpretation – not the original paper", always linking to the real paper. Never present it under the original title as the original authors' work.
- Lengths: research articles = full journal-article length; news stories = about one page (Perplexity Discover length) with images and references.
- Generated articles are saved, so they read the same every time.
- Ask panel: Perplexity-style follow-up questions about the article or news story, with referenced answers that can link to other articles in the app.
- Share button at the end: email as text, export as .docx, .txt or PDF, or print. Exports keep the AI labels, text-origin marking and references.

## Text origin marking (must-have)

Store the origin of every passage in the data model, and drive all of the following from it:

1. **Original text** (verbatim from the source): normal style.
2. **AI-reworded original**: underlined; hover/click shows the original wording. AI rewording of originals is limited to improving flow.
3. **AI-written**: distinct colour (e.g. purple), **plus** a margin bar or icon so it works for colourblind users and printouts. Links to real sources are styled distinctly (e.g. green).

- Every factual claim in AI text needs a citation to a retrieved source; unsourced claims are dropped or marked "unverified".
- References in Harvard style at the end of each article (other styles optional in settings).

## Research layer (code does the research, not the model)

So even small local models only have to write from sources provided:

- Research sources (free APIs): OpenAlex, Semantic Scholar, arXiv, Crossref, Unpaywall (legal free full-text links), CORE.
- News sources: GDELT and outlet RSS feeds. Each news story is one combined article on the event, built from 10–20 sources across outlets, written as neutrally as possible whatever the slant of individual sources, with inline references to every original article.
- Pipeline: query expansion → multi-API search → deduplicate by DOI/URL → find legal full text → fetch citing papers and extract the sentences describing the target → chunk → rank (BM25 first, optional local embeddings later) → pass the top chunks with source IDs to the model.
- Cache results, respect rate limits, and send a polite user-agent and contact email.
- Only use legally available content. No bypassing logins, no shadow libraries.

## Model providers

`ModelProvider` trait; the user picks per task (e.g. small model for headlines, strong model for full articles):

- **Local AI CLIs using the user's own login**: Claude Code (`claude -p`, JSON output), Gemini CLI, Codex. Detect which are installed. These may also do extra research themselves, but always receive the research layer's sources.
- **API keys**: Anthropic, Google Gemini (including its free tier), any OpenAI-compatible endpoint.
- **Local models**: Ollama (default localhost:11434) with a picker for installed models.
- Later: an in-app downloader for a recommended local model that works well with the app.

## Writing pipeline

- The model writes only from the numbered sources it is given, citing inline.
- Validate afterwards: every citation must map to a real source; reject invented references; flag uncited factual sentences.
- Weaker models generate section by section, with sources per section.
- Illustrations: real images as above, plus AI-specified charts and diagrams (bar, line, Venn, stock-style graphs, flow diagrams). The AI outputs a small chart spec (type, data, labels) and the app renders it in the app's own style. Chart numbers must come from cited sources.

## Notes, projects and bibliography

- Notes stay lightweight (the app is for research, not writing up): short notes with links to articles; clicking a link opens the article; linking an article in a note saves it permanently.
- Quoting into notes keeps the origin marking. Fully AI text gets a prompt: "Rewrite this in your own words from your own research", listing the references the AI used. Mixed quotes show which parts are from the real paper and which are AI.
- Projects group notes, bookshelves and articles under one research topic.
- Bibliography builder: collect references used in a project and export them (Harvard style, BibTeX/RIS for Zotero). Cite real sources, never the AI article itself.
- Notifications: follow topics or authors and get notified in-app about new research when the app is open.

## Storage and cache

- Kept permanently: favourites, read later, bookshelves, projects, and articles linked in notes.
- History: recently read articles plus past feed headlines, limited by disk size (configurable), not by count.
- "Clear cache" only clears history, never saved items, and warns that it deletes AI-written articles, which would have to be regenerated from scratch.

## Milestones

1. App shell with the Artcraft GUI library: layout, sidebar, tabs, light/dark themes, settings, SQLite.
2. Research layer and Scholar-mode search with filters; paper view with legal full-text links.
3. Model providers (CLIs, APIs, Ollama) and the writing pipeline with citation validation and origin marking.
4. Article reader: abstract-first, progress, streaming, hover-originals, ask panel.
5. Discover Research and Discover News with daily generation, history, and the news pipeline.
6. AI search mode, favourites/read later/bookshelves, generation queue.
7. Notes, projects, bibliography export, notifications.
8. Share/export (email, docx, txt, PDF, print), charts/diagrams, packaging.

## Out of scope for v1

- Windows build (only leave TODO comments), cloud hosting, user accounts, a full writing editor, the in-app model downloader.
