//! Plain-text export.
//!
//! Marking scheme (explained in the legend at the top of the file):
//! - original text: plain;
//! - AI-reworded original: `⟦reworded: …⟧ (original: "…")`;
//! - AI-written: `⟦AI: …⟧`, and every line holding AI text starts with a `▍` margin bar;
//! - unverified claims: `[unverified]`; citations: `[n]` matching the reference list.

use super::*;

const WIDTH: usize = 78;
const BAR: &str = "▍ ";
const NO_BAR: &str = "  ";

pub(super) fn render(a: &Article, style: CitationStyle) -> String {
    let mut out = String::new();
    let f = front(a);

    let banner = format!("[ {} ]", f.label.to_uppercase());
    out.push_str(&banner);
    out.push('\n');
    out.push_str(&"=".repeat(banner.chars().count()));
    out.push_str("\n\n");
    push_wrapped(&mut out, &f.title, "");
    out.push('\n');
    if let Some(s) = &f.standfirst {
        push_wrapped(&mut out, s, "");
        out.push('\n');
    }
    out.push_str(&f.byline);
    out.push('\n');
    out.push_str(&f.generated);
    out.push('\n');
    if let Some((t, url)) = &f.original {
        push_wrapped(&mut out, t, "");
        if let Some(u) = url {
            out.push_str(&format!("  {u}\n"));
        }
    }
    if let Some(img) = &f.hero {
        out.push('\n');
        push_image(&mut out, img);
    }

    out.push_str("\nHow the text is marked\n");
    out.push_str("----------------------\n");
    out.push_str(&format!("{NO_BAR}Plain text             = {}\n", LEGEND[0].1));
    push_wrapped(
        &mut out,
        &format!("⟦reworded: …⟧ (original: \"…\") = {}", LEGEND[1].1),
        NO_BAR,
    );
    push_wrapped(&mut out, &format!("⟦AI: …⟧ = {} Lines with AI text start with ▍.", LEGEND[2].1), BAR);
    out.push_str(&format!("{NO_BAR}[n] = source n in the references; [unverified] = no supporting source found.\n"));

    let mut fig = 0;
    for sec in &a.sections {
        if !sec.heading.trim().is_empty() {
            out.push('\n');
            out.push_str(sec.heading.trim());
            out.push('\n');
            out.push_str(&"-".repeat(sec.heading.trim().chars().count().min(WIDTH)));
            out.push('\n');
        }
        for b in &sec.blocks {
            match b {
                Block::Heading(h) => {
                    out.push('\n');
                    out.push_str(&format!("{NO_BAR}{}\n", h.trim()));
                }
                Block::Paragraph(ps) => {
                    if ps.is_empty() {
                        continue;
                    }
                    out.push('\n');
                    let words = paragraph_words(ps);
                    for (line, ai) in wrap_words(&words, WIDTH - 2) {
                        out.push_str(if ai { BAR } else { NO_BAR });
                        out.push_str(&line);
                        out.push('\n');
                    }
                }
                Block::Chart(c) => {
                    fig += 1;
                    out.push('\n');
                    push_chart(&mut out, c, fig);
                }
                Block::Image(img) => {
                    out.push('\n');
                    push_image(&mut out, img);
                }
            }
        }
    }

    let refs = reference_entries(a, style);
    if !refs.is_empty() {
        let h = references_heading(style);
        out.push_str(&format!("\n{h}\n{}\n", "-".repeat(h.chars().count())));
        for r in refs {
            let prefix = format!("[{}] ", r.n);
            let lines = wrap_plain(&r.text, WIDTH - prefix.chars().count());
            for (i, l) in lines.iter().enumerate() {
                if i == 0 {
                    out.push_str(&prefix);
                } else {
                    out.push_str(&" ".repeat(prefix.chars().count()));
                }
                out.push_str(l);
                out.push('\n');
            }
        }
    }
    let notes = validation_lines(a);
    if !notes.is_empty() {
        out.push_str("\nValidation notes\n----------------\n");
        for n in notes {
            push_wrapped(&mut out, &format!("- {n}"), "");
        }
    }
    out.push('\n');
    push_wrapped(
        &mut out,
        &format!("{}. Exported from Gateless Scholar. Cite the original sources listed above, not this text.", f.label),
        "",
    );
    out
}

/// A word and whether it belongs to AI-written text.
type Word = (String, bool);

fn paragraph_words(ps: &[Passage]) -> Vec<Word> {
    let mut words: Vec<Word> = Vec::new();
    let push = |s: &str, ai: bool, words: &mut Vec<Word>| {
        for w in s.split_whitespace() {
            words.push((w.to_string(), ai));
        }
    };
    for run in passage_runs(ps) {
        let origin = run[0].origin;
        let mut text = String::new();
        for p in &run {
            if !text.is_empty() {
                text.push(' ');
            }
            text.push_str(p.text.trim());
            if p.unverified {
                text.push_str(" [unverified]");
            }
            let c = cite_marker(&p.citations);
            if !c.is_empty() {
                text.push(' ');
                text.push_str(&c);
            }
        }
        match origin {
            Origin::Original => push(&text, false, &mut words),
            Origin::Reworded => {
                let mut s = format!("⟦reworded: {text}⟧");
                if let Some(o) = run[0].original.as_ref().filter(|o| !o.trim().is_empty()) {
                    s.push_str(&format!(" (original: \"{}\")", o.trim()));
                }
                push(&s, false, &mut words);
            }
            Origin::AiWritten => push(&format!("⟦AI: {text}⟧"), true, &mut words),
        }
    }
    words
}

/// Greedy word wrap; each line reports whether it holds any AI word.
fn wrap_words(words: &[Word], width: usize) -> Vec<(String, bool)> {
    let mut lines: Vec<(String, bool)> = Vec::new();
    let mut cur = String::new();
    let mut cur_len = 0;
    let mut cur_ai = false;
    for (w, ai) in words {
        let wl = w.chars().count();
        if cur_len > 0 && cur_len + 1 + wl > width {
            lines.push((std::mem::take(&mut cur), cur_ai));
            cur_len = 0;
            cur_ai = false;
        }
        if cur_len > 0 {
            cur.push(' ');
            cur_len += 1;
        }
        cur.push_str(w);
        cur_len += wl;
        cur_ai |= *ai;
    }
    if cur_len > 0 {
        lines.push((cur, cur_ai));
    }
    lines
}

fn wrap_plain(s: &str, width: usize) -> Vec<String> {
    let words: Vec<Word> = s.split_whitespace().map(|w| (w.to_string(), false)).collect();
    wrap_words(&words, width.max(20)).into_iter().map(|(l, _)| l).collect()
}

fn push_wrapped(out: &mut String, s: &str, prefix: &str) {
    for l in wrap_plain(s, WIDTH - prefix.chars().count()) {
        out.push_str(prefix);
        out.push_str(&l);
        out.push('\n');
    }
}

fn push_image(out: &mut String, img: &ImageRef) {
    push_wrapped(out, &format!("[Image] {}", image_caption(img)), NO_BAR);
    if let Some(l) = image_link(img) {
        out.push_str(&format!("{NO_BAR}Source: {l}\n"));
    }
    if let Some(l) = &img.licence_url {
        out.push_str(&format!("{NO_BAR}Licence: {l}\n"));
    }
}

fn push_chart(out: &mut String, c: &ChartSpec, n: usize) {
    push_wrapped(out, &format!("[Figure {n}] {}", c.title.trim()), NO_BAR);
    push_wrapped(out, &chart_caption(c), NO_BAR);
    let mut extra = Vec::new();
    if let Some(x) = c.x_label.as_ref().filter(|s| !s.trim().is_empty()) {
        extra.push(format!("x: {x}"));
    }
    if let Some(y) = c.y_label.as_ref().filter(|s| !s.trim().is_empty()) {
        extra.push(format!("y: {y}"));
    }
    if !extra.is_empty() {
        out.push_str(&format!("{NO_BAR}({})\n", extra.join("; ")));
    }
    let (header, rows) = chart_table(c);
    let cols = header.len();
    let mut widths = vec![0usize; cols];
    for row in std::iter::once(&header).chain(rows.iter()) {
        for (i, cell) in row.iter().enumerate().take(cols) {
            widths[i] = widths[i].max(cell.chars().count()).min(30);
        }
    }
    let fmt_row = |row: &Vec<String>| {
        let cells: Vec<String> = (0..cols)
            .map(|i| {
                let cell = row.get(i).map(String::as_str).unwrap_or("");
                let cell: String = if cell.chars().count() > 30 {
                    format!("{}…", cell.chars().take(29).collect::<String>())
                } else {
                    cell.to_string()
                };
                let pad = widths[i].saturating_sub(cell.chars().count());
                format!("{cell}{}", " ".repeat(pad))
            })
            .collect();
        format!("{NO_BAR}| {} |\n", cells.join(" | "))
    };
    out.push_str(&fmt_row(&header));
    let sep: Vec<String> = widths.iter().map(|w| "-".repeat(*w)).collect();
    out.push_str(&format!("{NO_BAR}|-{}-|\n", sep.join("-|-")));
    for r in &rows {
        out.push_str(&fmt_row(r));
    }
}
