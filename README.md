# Gateless Scholar

**Research, ungated.** A native Linux desktop app that gives people without university logins a way into research:

- **Search** like Google Scholar (Scholar mode) or let an AI curate the newest and most relevant work first (AI mode). Journals and news are searched separately, with filters for date, free full text, content type, subject and sort order.
- **Discover Research** and **Discover News**: each day the app gathers headlines and short abstracts for your topic tabs. It writes full articles only when you open one or add it to Read Later.
- **Articles you can trust about what they are.** If a legal free full text exists, you read the original, lightly reworded for flow. If only the abstract is free, the AI writes its own interpretation, labelled *"AI interpretation – not the original paper"*, built from real sources (citing papers, related open work), and it always links to the real paper.

## Text origin marking

The data model stores the origin of every passage, and the reader, notes and every export are drawn from it:

| Origin | How it looks |
|---|---|
| Original text (verbatim) | normal |
| AI-reworded original | underlined. Hover to see the original wording. Rewording is checked in code and reverted if it drifts too far. |
| AI-written | purple **and** a margin bar, so it still reads for colourblind readers and on printouts |
| Links to real sources | green `[n]` |

Every factual claim in AI text must cite a retrieved source. Citations to sources that don't exist are removed, and sentences without a source are marked *unverified*. Chart numbers must appear in the cited sources. References come at the end in Harvard style (APA 7 and Vancouver are optional).

## How it works

The **code does the research, not the model.** The pipeline runs query expansion → OpenAlex, Semantic Scholar, arXiv, Crossref, CORE and Europe PMC → deduplication by DOI/URL → legal full text via Unpaywall, arXiv and Europe PMC → sentences from citing papers that describe the work → chunking → BM25 ranking. The top passages go to the model as numbered sources, so even a small local model only has to write from what it's given. News stories combine 10–20 outlets found through GDELT and RSS into one neutral article, with inline references to each.

Only legally available content is used. The app bypasses no logins and uses no shadow libraries.

## Models

You choose a model for each task (headlines, articles, news, the Ask panel, AI search):

- **Local AI CLIs using your own login:** Claude Code (`claude -p`), Gemini CLI, Codex. The app detects which ones are installed.
- **API keys** (stored in the system keyring): Anthropic, Google Gemini (the free tier works), or any OpenAI-compatible endpoint.
- **Local models:** Ollama on `localhost:11434`, with a picker for the models you have installed.

Without any model, search and real papers still work.

## Download

Get the AppImage from the [latest release](https://github.com/brattzachary-bit/gateless-scholar/releases/latest), then:

```sh
chmod +x Gateless_Scholar-*-x86_64.AppImage
./Gateless_Scholar-*-x86_64.AppImage
```

It runs on practically any x86-64 Linux from 2018 onwards (glibc 2.28+: Ubuntu 18.10+, Debian 10+, Fedora 29+, Mint 19+), on Wayland or X11. If your system lacks FUSE, run it with `--appimage-extract-and-run`.

## Building

```sh
cargo build --release
./target/release/gateless
```

Requirements: Rust ≥ 1.97 and a Linux desktop (Wayland or X11). There are no system library dependencies beyond the usual GL/Wayland/X11 stack. `fc-match` (fontconfig) is used to pick your desktop's sans-serif font.

Packaging:

- AppImage: `packaging/appimage/build-appimage.sh` (needs `appimagetool`, plus `cargo-zigbuild` and `zig` for the portable glibc 2.28 build, or `NATIVE=1`)
- Flatpak: `flatpak-builder --user --install build-dir packaging/flatpak/org.gatelessscholar.Gateless.yml`

Data lives in `~/.local/share/gateless/`. Set `XDG_DATA_HOME` to use a different location.

## Keyboard

Press `F1` for the full list. The main shortcuts:

| Keys | Action |
|---|---|
| `Ctrl+K` or `/` | search |
| `Ctrl+M` | switch between Scholar and AI mode |
| `Ctrl+1…9` | change topic tab |
| `Tab` | switch between Research and News |
| `Ctrl+R` | refresh Discover |
| `Ctrl+B` | sidebar |
| `Ctrl+J` | Ask panel |
| `Alt+←` / `Alt+→` | back and forward |
| `Ctrl+Shift+L` | light/dark |

## Code map

| Path | What |
|---|---|
| `src/model.rs` | shared types: `Paper`, `Article`, `Passage { origin }`, `Source`, `ChartSpec`, … |
| `src/db.rs` | SQLite storage: saved lists, history with a size limit, queue, notes, cache |
| `src/research/` | research layer (APIs, full text, citing contexts, BM25, images, news) |
| `src/providers/` | `ModelProvider` trait and the CLI, API and Ollama providers |
| `src/writing/` | writing pipeline, citation validation, origin marking, charts |
| `src/export/` | txt, docx, PDF, print, email; Harvard, APA and Vancouver, BibTeX, RIS |
| `src/jobs.rs` | background orchestration (daily Discover, generation, search, ask) |
| `src/ui/` | egui UI built on the ArtCraft Craft suite's `craft-ui` and `craft-window` |

Windows isn't supported yet. Places that need work are marked `// TODO(windows):`.

## Licence

MIT OR Apache-2.0, at your option. The GUI uses [`craft-ui` and `craft-window`](https://github.com/storytold/craft-libs) from the ArtCraft project (MIT/Apache-2.0) on its egui fork.
