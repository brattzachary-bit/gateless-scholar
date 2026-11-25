//! PDF export: a small hand-written PDF 1.4 writer (no system libraries, no font files) using
//! the built-in Helvetica faces with exact AFM metrics for word wrapping.
//!
//! Marking: original text plain; AI-reworded original underlined, with a superscript number
//! linking to the "Original wording" endnotes; AI-written text purple **and** a purple bar in
//! the left margin beside every line that holds AI text (plus a light tint behind AI words in
//! mixed paragraphs), so it survives greyscale printing. Citations "[n]" are green and link to
//! the reference entry; URLs are green with link annotations. Every page's footer repeats the
//! article label and "Page i of N".

use std::collections::HashMap;
use std::fmt::Write as _;

use anyhow::Result;

use super::metrics::{HELVETICA, HELVETICA_BOLD, fallback, winansi};
use super::*;

// ---------------------------------------------------------------------------------------------
// Page geometry and palette
// ---------------------------------------------------------------------------------------------

const PAGE_W: f32 = 595.28;
const PAGE_H: f32 = 841.89;
const LEFT: f32 = 72.0;
const RIGHT: f32 = PAGE_W - 56.0;
const TOP: f32 = PAGE_H - 56.0;
const BOTTOM: f32 = 64.0;
const TEXT_W: f32 = RIGHT - LEFT;
const BAR_X: f32 = LEFT - 14.0;
const BAR_W: f32 = 3.0;

#[derive(Clone, Copy, Debug, PartialEq)]
struct Rgb(f32, f32, f32);

const BLACK: Rgb = Rgb(0.11, 0.11, 0.13);
const GREY: Rgb = Rgb(0.37, 0.39, 0.41);
const LIGHT: Rgb = Rgb(0.80, 0.80, 0.82);
const PURPLE: Rgb = Rgb(123.0 / 255.0, 63.0 / 255.0, 191.0 / 255.0);
const AI_TINT: Rgb = Rgb(0.937, 0.902, 0.980);
const GREEN: Rgb = Rgb(30.0 / 255.0, 132.0 / 255.0, 73.0 / 255.0);
const WHITE: Rgb = Rgb(1.0, 1.0, 1.0);
const SERIES: [Rgb; 6] = [
    Rgb(0.20, 0.42, 0.80),
    Rgb(0.93, 0.53, 0.13),
    Rgb(0.13, 0.62, 0.53),
    Rgb(0.84, 0.27, 0.36),
    Rgb(0.45, 0.47, 0.52),
    Rgb(0.72, 0.62, 0.10),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Font {
    Regular,
    Bold,
    Italic,
    BoldItalic,
}

impl Font {
    fn name(self) -> &'static str {
        match self {
            Font::Regular => "F1",
            Font::Bold => "F2",
            Font::Italic => "F3",
            Font::BoldItalic => "F4",
        }
    }
    fn widths(self) -> &'static [u16; 256] {
        match self {
            Font::Regular | Font::Italic => &HELVETICA,
            Font::Bold | Font::BoldItalic => &HELVETICA_BOLD,
        }
    }
}

/// Unicode → WinAnsi bytes, spelling out characters Helvetica cannot show.
fn encode(s: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len());
    for c in s.chars() {
        match winansi(c) {
            Some(b) => out.push(b),
            None => {
                for fc in fallback(c).chars() {
                    out.push(winansi(fc).unwrap_or(b'?'));
                }
            }
        }
    }
    out
}

fn width_bytes(b: &[u8], font: Font, size: f32) -> f32 {
    let w = font.widths();
    b.iter().map(|&c| w[c as usize] as f32).sum::<f32>() * size / 1000.0
}

fn text_width(s: &str, font: Font, size: f32) -> f32 {
    width_bytes(&encode(s), font, size)
}

fn pdf_string(b: &[u8]) -> String {
    let mut s = String::with_capacity(b.len() + 2);
    s.push('(');
    for &c in b {
        match c {
            b'(' | b')' | b'\\' => {
                s.push('\\');
                s.push(c as char);
            }
            0x20..=0x7E => s.push(c as char),
            _ => {
                let _ = write!(s, "\\{c:03o}");
            }
        }
    }
    s.push(')');
    s
}

/// Literal string for non-text values (URIs, info dictionary).
/// Text strings outside content streams (document info) are UTF-16BE with a BOM.
fn pdf_text_value(s: &str) -> String {
    let mut out = String::from("<FEFF");
    for u in s.encode_utf16() {
        let _ = write!(out, "{u:04X}");
    }
    out.push('>');
    out
}

fn f(v: f32) -> String {
    let v = if v.is_finite() { v } else { 0.0 };
    let s = format!("{v:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

// ---------------------------------------------------------------------------------------------
// Rich text
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
enum Target {
    Uri(String),
    /// A named destination inside the document (resolved when writing).
    Dest(String),
}

#[derive(Clone, Debug)]
struct Span {
    text: String,
    font: Font,
    size: f32,
    color: Rgb,
    underline: bool,
    link: Option<Target>,
    /// Part of AI-written text: draws the margin bar on its lines.
    ai: bool,
    /// Light tint behind the text (AI words in mixed paragraphs).
    tint: bool,
    /// Baseline shift (superscripts).
    rise: f32,
}

impl Span {
    fn new(text: impl Into<String>, size: f32) -> Self {
        Span {
            text: text.into(),
            font: Font::Regular,
            size,
            color: BLACK,
            underline: false,
            link: None,
            ai: false,
            tint: false,
            rise: 0.0,
        }
    }
    fn font(mut self, f: Font) -> Self {
        self.font = f;
        self
    }
    fn color(mut self, c: Rgb) -> Self {
        self.color = c;
        self
    }
    fn underline(mut self) -> Self {
        self.underline = true;
        self
    }
    fn link(mut self, t: Target) -> Self {
        self.link = Some(t);
        self
    }
    fn ai(mut self, tint: bool) -> Self {
        self.ai = true;
        self.tint = tint;
        self.color = PURPLE;
        self
    }
}

/// A piece of a word in one style.
#[derive(Clone, Debug)]
struct Piece {
    bytes: Vec<u8>,
    span: usize,
    width: f32,
}

type Word = Vec<Piece>;

#[derive(Clone, Copy)]
struct ParaOpts {
    indent: f32,
    line_h: f32,
    space_after: f32,
    /// Keep this many lines together at a page break (widow control for short blocks).
    keep: usize,
}

impl Default for ParaOpts {
    fn default() -> Self {
        ParaOpts { indent: 0.0, line_h: 15.0, space_after: 8.0, keep: 2 }
    }
}

fn words_of(spans: &[Span]) -> Vec<Word> {
    let mut words: Vec<Word> = Vec::new();
    let mut glue = false; // the next piece continues the previous word
    for (i, sp) in spans.iter().enumerate() {
        let text = sp.text.as_str();
        if text.is_empty() {
            continue;
        }
        if text.starts_with(char::is_whitespace) {
            glue = false;
        }
        let parts: Vec<&str> = text.split_whitespace().collect();
        for (k, part) in parts.iter().enumerate() {
            let bytes = encode(part);
            let width = width_bytes(&bytes, sp.font, sp.size);
            let piece = Piece { bytes, span: i, width };
            match words.last_mut() {
                Some(w) if glue && k == 0 => w.push(piece),
                _ => words.push(vec![piece]),
            }
            glue = false;
        }
        glue = !text.ends_with(char::is_whitespace) && !parts.is_empty();
    }
    words
}

// ---------------------------------------------------------------------------------------------
// Layout
// ---------------------------------------------------------------------------------------------

struct Annot {
    rect: [f32; 4],
    target: Target,
}

#[derive(Default)]
struct Page {
    content: String,
    annots: Vec<Annot>,
}

struct Layout {
    pages: Vec<Page>,
    y: f32,
    dests: HashMap<String, (usize, f32)>,
}

impl Layout {
    fn new() -> Self {
        Layout { pages: vec![Page::default()], y: TOP, dests: HashMap::new() }
    }

    fn page(&mut self) -> &mut Page {
        if self.pages.is_empty() {
            self.pages.push(Page::default());
        }
        let n = self.pages.len() - 1;
        &mut self.pages[n]
    }

    fn new_page(&mut self) {
        self.pages.push(Page::default());
        self.y = TOP;
    }

    /// Start a new page unless `h` points of space remain.
    fn ensure(&mut self, h: f32) {
        if self.y - h < BOTTOM && self.y < TOP - 1.0 {
            self.new_page();
        }
    }

    fn dest_here(&mut self, name: String) {
        let p = self.pages.len().saturating_sub(1);
        let y = (self.y + 14.0).min(PAGE_H);
        self.dests.entry(name).or_insert((p, y));
    }

    fn ops(&mut self, s: &str) {
        self.page().content.push_str(s);
    }

    fn rect(&mut self, x: f32, y: f32, w: f32, h: f32, c: Rgb) {
        let s = format!("{} {} {} rg {} {} {} {} re f\n", f(c.0), f(c.1), f(c.2), f(x), f(y), f(w), f(h));
        self.ops(&s);
    }

    fn stroke_rect(&mut self, x: f32, y: f32, w: f32, h: f32, c: Rgb, lw: f32) {
        let s = format!("{} {} {} RG {} w {} {} {} {} re S\n", f(c.0), f(c.1), f(c.2), f(lw), f(x), f(y), f(w), f(h));
        self.ops(&s);
    }

    fn line(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, c: Rgb, lw: f32) {
        let s = format!("{} {} {} RG {} w {} {} m {} {} l S\n", f(c.0), f(c.1), f(c.2), f(lw), f(x1), f(y1), f(x2), f(y2));
        self.ops(&s);
    }

    fn text_at(&mut self, x: f32, y: f32, s: &str, font: Font, size: f32, c: Rgb) {
        let b = encode(s);
        let op = text_op(x, y, &b, font, size, c, 0.0);
        self.ops(&op);
    }

    fn text_center(&mut self, cx: f32, y: f32, s: &str, font: Font, size: f32, c: Rgb) {
        let w = text_width(s, font, size);
        self.text_at(cx - w / 2.0, y, s, font, size, c);
    }

    fn link_rect(&mut self, rect: [f32; 4], target: Target) {
        self.page().annots.push(Annot { rect, target });
    }

    fn gap(&mut self, h: f32) {
        self.y -= h;
    }

    /// Lay out a rich paragraph with word wrapping and page breaks.
    fn paragraph(&mut self, spans: &[Span], o: ParaOpts) {
        let words = words_of(spans);
        if words.is_empty() {
            return;
        }
        let max_w = TEXT_W - o.indent;
        let space = |w: &Word| spans.get(w.first().map(|p| p.span).unwrap_or(0)).map(|s| text_width(" ", s.font, s.size)).unwrap_or(3.0);
        // Break into lines.
        let mut lines: Vec<Vec<Word>> = Vec::new();
        let mut cur: Vec<Word> = Vec::new();
        let mut cur_w = 0.0;
        for w in words {
            let ww: f32 = w.iter().map(|p| p.width).sum();
            if ww > max_w {
                // Overlong word (e.g. a URL): split it into chunks that fit.
                if !cur.is_empty() {
                    lines.push(std::mem::take(&mut cur));
                    cur_w = 0.0;
                }
                for chunk in split_long_word(w, spans, max_w) {
                    lines.push(vec![vec![chunk]]);
                }
                continue;
            }
            let sp = if cur.is_empty() { 0.0 } else { space(&w) };
            if !cur.is_empty() && cur_w + sp + ww > max_w {
                lines.push(std::mem::take(&mut cur));
                cur_w = 0.0;
                cur.push(w);
                cur_w += ww;
            } else {
                cur_w += sp + ww;
                cur.push(w);
            }
        }
        if !cur.is_empty() {
            lines.push(cur);
        }
        let keep = o.keep.min(lines.len()).max(1);
        self.ensure(o.line_h * keep as f32);
        for line in &lines {
            self.ensure(o.line_h);
            let baseline = self.y - o.line_h * 0.78;
            self.render_line(line, spans, LEFT + o.indent, baseline, o.line_h, &space);
            self.y -= o.line_h;
        }
        self.y -= o.space_after;
    }

    fn render_line(&mut self, line: &[Word], spans: &[Span], x0: f32, baseline: f32, lh: f32, space: &dyn Fn(&Word) -> f32) {
        let mut bg = String::new();
        let mut fg = String::new();
        let mut annots: Vec<Annot> = Vec::new();
        let mut x = x0;
        let mut has_ai = false;
        let mut prev: Option<(usize, f32)> = None; // last span index and x end
        for (wi, w) in line.iter().enumerate() {
            if wi > 0 {
                let sp = space(w);
                // Carry underline / tint / link across the space between two pieces of the same span.
                if let (Some((ps, pend)), Some(first)) = (prev, w.first())
                    && ps == first.span
                    && let Some(s) = spans.get(ps)
                {
                    if s.underline {
                        let _ = write!(fg, "{} {} {} RG 0.6 w {} {} m {} {} l S\n", f(s.color.0), f(s.color.1), f(s.color.2), f(pend), f(baseline - 1.6), f(pend + sp), f(baseline - 1.6));
                    }
                    if s.tint {
                        let _ = write!(bg, "{} {} {} rg {} {} {} {} re f\n", f(AI_TINT.0), f(AI_TINT.1), f(AI_TINT.2), f(pend), f(baseline - lh * 0.22), f(sp), f(lh * 0.9));
                    }
                }
                x += sp;
            }
            for p in w {
                let Some(s) = spans.get(p.span) else { continue };
                has_ai |= s.ai;
                if s.tint {
                    let _ = write!(bg, "{} {} {} rg {} {} {} {} re f\n", f(AI_TINT.0), f(AI_TINT.1), f(AI_TINT.2), f(x), f(baseline - lh * 0.22), f(p.width), f(lh * 0.9));
                }
                fg.push_str(&text_op(x, baseline + s.rise, &p.bytes, s.font, s.size, s.color, 0.0));
                if s.underline {
                    let _ = write!(fg, "{} {} {} RG 0.6 w {} {} m {} {} l S\n", f(s.color.0), f(s.color.1), f(s.color.2), f(x), f(baseline - 1.6), f(x + p.width), f(baseline - 1.6));
                }
                if let Some(t) = &s.link {
                    let r = [x, baseline - 3.0, x + p.width, baseline + s.size * 0.8 + s.rise];
                    match annots.last_mut() {
                        Some(a) if a.target == *t && (a.rect[2] - x).abs() < 8.0 => a.rect[2] = r[2],
                        _ => annots.push(Annot { rect: r, target: t.clone() }),
                    }
                }
                x += p.width;
                prev = Some((p.span, x));
            }
        }
        if has_ai {
            let _ = write!(bg, "{} {} {} rg {} {} {} {} re f\n", f(PURPLE.0), f(PURPLE.1), f(PURPLE.2), f(BAR_X), f(baseline - lh * 0.22 - 0.5), f(BAR_W), f(lh + 1.0));
        }
        let page = self.page();
        page.content.push_str(&bg);
        page.content.push_str(&fg);
        page.annots.extend(annots);
    }

    fn heading(&mut self, text: &str, size: f32, space_before: f32) {
        self.ensure(size * 1.4 + 15.0 * 3.0 + space_before);
        if self.y < TOP - 1.0 {
            self.gap(space_before);
        }
        self.paragraph(&[Span::new(text, size).font(Font::Bold)], ParaOpts { line_h: size * 1.3, space_after: size * 0.45, keep: 1, ..Default::default() });
    }
}

fn text_op(x: f32, y: f32, b: &[u8], font: Font, size: f32, c: Rgb, _rise: f32) -> String {
    format!(
        "BT /{} {} Tf {} {} {} rg {} {} Td {} Tj ET\n",
        font.name(),
        f(size),
        f(c.0),
        f(c.1),
        f(c.2),
        f(x),
        f(y),
        pdf_string(b)
    )
}

fn split_long_word(w: Word, spans: &[Span], max_w: f32) -> Vec<Piece> {
    let mut out = Vec::new();
    for p in w {
        let Some(s) = spans.get(p.span) else { continue };
        let widths = s.font.widths();
        let mut cur = Vec::new();
        let mut cw = 0.0;
        for b in p.bytes {
            let bw = widths[b as usize] as f32 * s.size / 1000.0;
            if cw + bw > max_w && !cur.is_empty() {
                out.push(Piece { bytes: std::mem::take(&mut cur), span: p.span, width: cw });
                cw = 0.0;
            }
            cur.push(b);
            cw += bw;
        }
        if !cur.is_empty() {
            out.push(Piece { bytes: cur, span: p.span, width: cw });
        }
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Article
// ---------------------------------------------------------------------------------------------

const BODY: f32 = 10.5;
const SMALL: f32 = 8.8;

pub(super) fn render(a: &Article, style: CitationStyle) -> Result<Vec<u8>> {
    render_with_pages(a, style).map(|(b, _)| b)
}

pub(super) fn render_with_pages(a: &Article, style: CitationStyle) -> Result<(Vec<u8>, usize)> {
    let mut l = Layout::new();
    let fr = front(a);

    // Label banner.
    let fill = if a.kind == ArticleKind::FullText { Rgb(0.216, 0.278, 0.31) } else { Rgb(0.357, 0.165, 0.525) };
    let bh = 22.0;
    l.rect(LEFT, l.y - bh, TEXT_W, bh, fill);
    l.text_at(LEFT + 8.0, l.y - bh + 7.0, &fr.label.to_uppercase(), Font::Bold, 9.5, WHITE);
    l.gap(bh + 14.0);

    l.paragraph(&[Span::new(&fr.title, 20.0).font(Font::Bold)], ParaOpts { line_h: 24.0, space_after: 6.0, ..Default::default() });
    if let Some(s) = &fr.standfirst {
        l.paragraph(&[Span::new(s, 12.0).color(GREY)], ParaOpts { line_h: 16.0, space_after: 8.0, ..Default::default() });
    }
    let meta = ParaOpts { line_h: 12.5, space_after: 1.0, ..Default::default() };
    l.paragraph(&[Span::new(&fr.byline, 9.5).font(Font::Bold).color(GREY)], meta);
    l.paragraph(&[Span::new(&fr.generated, 9.5).color(GREY)], meta);
    if let Some((t, url)) = &fr.original {
        let mut spans = vec![Span::new(t.clone(), 9.5).color(GREY)];
        if let Some(u) = url {
            spans.push(Span::new(" — ", 9.5).color(GREY));
            spans.push(Span::new(u.clone(), 9.5).color(GREEN).link(Target::Uri(u.clone())));
        }
        l.paragraph(&spans, meta);
    }
    if let Some(img) = &fr.hero {
        l.gap(6.0);
        image_caption_block(&mut l, img);
    }

    // Legend box.
    l.gap(10.0);
    legend(&mut l);

    let mut fig = 0;
    let mut endnotes: Vec<String> = Vec::new();
    for sec in &a.sections {
        if !sec.heading.trim().is_empty() {
            l.heading(sec.heading.trim(), 14.0, 8.0);
        }
        for b in &sec.blocks {
            match b {
                Block::Heading(h) => l.heading(h.trim(), 11.5, 4.0),
                Block::Paragraph(ps) => paragraph(&mut l, ps, &mut endnotes),
                Block::Chart(c) => {
                    fig += 1;
                    chart(&mut l, c, fig);
                }
                Block::Image(img) => image_caption_block(&mut l, img),
            }
        }
    }

    if !endnotes.is_empty() {
        l.heading("Original wording of reworded passages", 12.5, 10.0);
        l.paragraph(
            &[Span::new("Underlined passages above were lightly reworded by the AI for flow. This is what the source said:", SMALL).color(GREY)],
            ParaOpts { line_h: 12.0, space_after: 6.0, ..Default::default() },
        );
        for (i, e) in endnotes.iter().enumerate() {
            let n = i + 1;
            l.ensure(12.5 * 2.0);
            l.dest_here(format!("ow_{n}"));
            let y = l.y;
            l.text_at(LEFT, y - 12.5 * 0.78, &format!("{n}."), Font::Bold, 9.5, BLACK);
            l.paragraph(&[Span::new(format!("“{e}”"), 9.5)], ParaOpts { indent: 22.0, line_h: 12.5, space_after: 4.0, keep: 2 });
        }
    }

    let refs = reference_entries(a, style);
    if !refs.is_empty() {
        l.heading(&references_heading(style), 12.5, 10.0);
        for r in refs {
            l.ensure(12.5 * 2.0);
            l.dest_here(format!("ref_{}", r.n));
            let y = l.y;
            l.text_at(LEFT, y - 12.5 * 0.78, &format!("[{}]", r.n), Font::Bold, 9.5, BLACK);
            let spans = text_with_link(&r.text, r.link.as_deref(), 9.5);
            l.paragraph(&spans, ParaOpts { indent: 26.0, line_h: 12.5, space_after: 4.0, keep: 2 });
        }
    }
    let notes = validation_lines(a);
    if !notes.is_empty() {
        l.heading("Validation notes", 11.5, 8.0);
        for n in notes {
            l.paragraph(&[Span::new(format!("•  {n}"), 9.5)], ParaOpts { line_h: 12.5, space_after: 2.0, ..Default::default() });
        }
    }
    l.gap(8.0);
    l.paragraph(
        &[Span::new(format!("{}. Exported from Gateless Scholar. Cite the original sources listed above, not this text.", fr.label), 8.5)
            .font(Font::Italic)
            .color(GREY)],
        ParaOpts { line_h: 11.0, ..Default::default() },
    );

    let pages = l.pages.len();
    let bytes = write_pdf(l, a)?;
    Ok((bytes, pages))
}

fn text_with_link(text: &str, link: Option<&str>, size: f32) -> Vec<Span> {
    if let Some(u) = link
        && !u.is_empty()
        && let Some(i) = text.find(u)
    {
        let (before, rest) = text.split_at(i);
        let after = &rest[u.len()..];
        return vec![
            Span::new(before, size),
            Span::new(u, size).color(GREEN).link(Target::Uri(u.to_string())),
            Span::new(after, size),
        ];
    }
    vec![Span::new(text, size)]
}

fn legend(l: &mut Layout) {
    let lh = 12.5;
    l.ensure(16.0 + lh * 7.0);
    let top = l.y;
    let page = l.pages.len();
    l.gap(6.0);
    l.paragraph(&[Span::new("How the text is marked", 9.0).font(Font::Bold).color(GREY)], ParaOpts { line_h: lh, space_after: 2.0, keep: 1, ..Default::default() });
    let o = ParaOpts { line_h: lh, space_after: 1.0, keep: 1, ..Default::default() };
    let rest = |prefix: &str, s: &str| s.strip_prefix(prefix).unwrap_or(s).to_string();
    l.paragraph(&[Span::new("Original text", SMALL).font(Font::Bold), Span::new(rest("Original text", LEGEND[0].1), SMALL)], o);
    l.paragraph(
        &[
            Span::new("AI-reworded original", SMALL).font(Font::Bold).underline(),
            Span::new(rest("AI-reworded original", LEGEND[1].1), SMALL).underline(),
            Span::new(" Underlined; numbered notes give the original wording.", SMALL),
        ],
        o,
    );
    l.paragraph(
        &[
            Span::new("AI-written", SMALL).font(Font::Bold).ai(false),
            Span::new(rest("AI-written", LEGEND[2].1), SMALL).ai(false),
            Span::new(" Purple, with a bar in the left margin.", SMALL).ai(false),
        ],
        o,
    );
    l.paragraph(
        &[
            Span::new("[n]", SMALL).color(GREEN),
            Span::new(" links to source n in the references; green text links to a real source. ", SMALL),
            Span::new("[unverified]", SMALL).font(Font::Italic),
            Span::new(" marks a claim with no supporting source.", SMALL),
        ],
        o,
    );
    l.gap(3.0);
    if l.pages.len() == page {
        // Frame drawn after the content so its height fits the wrapped legend exactly.
        let y = l.y;
        l.stroke_rect(LEFT - 20.0, y, TEXT_W + 28.0, top - y, LIGHT, 0.6);
    }
    l.gap(14.0);
}

fn paragraph(l: &mut Layout, ps: &[Passage], endnotes: &mut Vec<String>) {
    if ps.is_empty() {
        return;
    }
    let has_ai = ps.iter().any(|p| p.origin == Origin::AiWritten);
    let mixed = has_ai && ps.iter().any(|p| p.origin != Origin::AiWritten);
    let mut spans: Vec<Span> = Vec::new();
    for (i, p) in ps.iter().enumerate() {
        let base = |t: &str| {
            let s = Span::new(t, BODY);
            match p.origin {
                Origin::Original => s,
                Origin::Reworded => s.underline(),
                Origin::AiWritten => s.ai(mixed),
            }
        };
        let lead = if i > 0 { " " } else { "" };
        spans.push(base(&format!("{lead}{}", p.text.trim())));
        if p.origin == Origin::Reworded
            && let Some(o) = p.original.as_ref().filter(|o| !o.trim().is_empty())
        {
            endnotes.push(o.trim().to_string());
            let n = endnotes.len();
            let mut m = Span::new(n.to_string(), 6.8).color(GREEN).link(Target::Dest(format!("ow_{n}")));
            m.rise = 4.0;
            spans.push(m);
        }
        if p.unverified {
            let mut s = base(" [unverified]").font(Font::Italic);
            s.underline = false;
            spans.push(s);
        }
        if !p.citations.is_empty() {
            let mk = |t: &str| {
                let mut s = Span::new(t, BODY).color(GREEN);
                if p.origin == Origin::AiWritten {
                    s.ai = true;
                    s.tint = mixed;
                }
                s
            };
            spans.push(mk(" ["));
            for (k, n) in p.citations.iter().enumerate() {
                if k > 0 {
                    spans.push(mk(", "));
                }
                spans.push(mk(&n.to_string()).link(Target::Dest(format!("ref_{n}"))));
            }
            spans.push(mk("]"));
        }
    }
    l.paragraph(&spans, ParaOpts { line_h: 15.0, space_after: 8.0, keep: 2, indent: 0.0 });
}

fn image_caption_block(l: &mut Layout, img: &ImageRef) {
    let mut spans = vec![Span::new("Image: ", SMALL).font(Font::BoldItalic).color(GREY), Span::new(image_caption(img), SMALL).font(Font::Italic).color(GREY)];
    if let Some(u) = image_link(img) {
        spans.push(Span::new(" Source: ", SMALL).font(Font::Italic).color(GREY));
        spans.push(Span::new(u.clone(), SMALL).color(GREEN).link(Target::Uri(u)));
    }
    if let Some(u) = &img.licence_url {
        spans.push(Span::new(" Licence: ", SMALL).font(Font::Italic).color(GREY));
        spans.push(Span::new(u.clone(), SMALL).color(GREEN).link(Target::Uri(u.clone())));
    }
    l.paragraph(&spans, ParaOpts { indent: 10.0, line_h: 11.5, space_after: 10.0, keep: 3 });
}

// ---------------------------------------------------------------------------------------------
// Charts
// ---------------------------------------------------------------------------------------------

fn chart(l: &mut Layout, c: &ChartSpec, n: usize) {
    let drawable = match c.kind {
        ChartKind::Bar | ChartKind::Line | ChartKind::Stock => c.series.iter().any(|s| !s.points.is_empty()),
        ChartKind::Venn => (1..=3).contains(&c.sets.len()),
        ChartKind::Flow => !c.nodes.is_empty() && c.nodes.len() <= 12,
    };
    let h = match c.kind {
        ChartKind::Flow => (c.nodes.len() as f32 * 40.0 + 30.0).max(120.0),
        _ => 200.0,
    };
    let title_h = 20.0;
    l.ensure(title_h + if drawable { h } else { 60.0 } + 30.0);
    l.gap(4.0);
    l.paragraph(&[Span::new(format!("Figure {n}: {}", c.title.trim()), 10.0).font(Font::Bold)], ParaOpts { line_h: 13.0, space_after: 4.0, keep: 1, ..Default::default() });
    if drawable {
        let top = l.y;
        let bottom = top - h;
        l.stroke_rect(LEFT, bottom, TEXT_W, h, LIGHT, 0.5);
        match c.kind {
            ChartKind::Bar | ChartKind::Line | ChartKind::Stock => xy_chart(l, c, LEFT + 8.0, bottom + 6.0, RIGHT - 10.0, top - 8.0),
            ChartKind::Venn => venn(l, c, bottom, top),
            ChartKind::Flow => flow(l, c, bottom, top),
        }
        l.y = bottom - 4.0;
    } else {
        data_table(l, c);
    }
    let mut spans = Vec::new();
    let mut axes = Vec::new();
    if let Some(x) = c.x_label.as_ref().filter(|s| !s.trim().is_empty()) {
        axes.push(format!("x: {x}"));
    }
    if let Some(y) = c.y_label.as_ref().filter(|s| !s.trim().is_empty()) {
        axes.push(format!("y: {y}"));
    }
    if !axes.is_empty() {
        spans.push(Span::new(format!("({}) ", axes.join("; ")), SMALL).font(Font::Italic).color(GREY));
    }
    let cap = chart_caption(c);
    match cap.find("data from [") {
        Some(i) => {
            spans.push(Span::new(&cap[..i + "data from".len()], SMALL).font(Font::Italic).color(GREY));
            spans.push(Span::new(" [", SMALL).color(GREEN));
            for (k, n) in c.citations.iter().enumerate() {
                if k > 0 {
                    spans.push(Span::new(", ", SMALL).color(GREEN));
                }
                spans.push(Span::new(n.to_string(), SMALL).color(GREEN).link(Target::Dest(format!("ref_{n}"))));
            }
            spans.push(Span::new("].", SMALL).color(GREEN));
        }
        None => spans.push(Span::new(cap, SMALL).font(Font::Italic).color(GREY)),
    }
    l.paragraph(&spans, ParaOpts { line_h: 11.5, space_after: 12.0, keep: 2, ..Default::default() });
}

/// Fallback: the chart's numbers as a simple table.
fn data_table(l: &mut Layout, c: &ChartSpec) {
    let (header, rows) = chart_table(c);
    let cols = header.len().max(1);
    let cw = TEXT_W / cols as f32;
    let rh = 14.0;
    let fit = |s: &str, font: Font| ellipsize(s, font, 8.5, cw - 8.0);
    l.ensure(rh * 2.0);
    let y = l.y;
    l.rect(LEFT, y - rh, TEXT_W, rh, Rgb(0.93, 0.91, 0.96));
    for (i, h) in header.iter().enumerate() {
        l.text_at(LEFT + 4.0 + i as f32 * cw, y - rh + 4.0, &fit(h, Font::Bold), Font::Bold, 8.5, BLACK);
    }
    l.y -= rh;
    for r in &rows {
        l.ensure(rh);
        let y = l.y;
        for i in 0..cols {
            let cell = r.get(i).map(String::as_str).unwrap_or("");
            l.text_at(LEFT + 4.0 + i as f32 * cw, y - rh + 4.0, &fit(cell, Font::Regular), Font::Regular, 8.5, BLACK);
        }
        l.line(LEFT, y - rh, RIGHT, y - rh, LIGHT, 0.4);
        l.y -= rh;
    }
    l.gap(4.0);
}

fn ellipsize(s: &str, font: Font, size: f32, max_w: f32) -> String {
    if text_width(s, font, size) <= max_w {
        return s.to_string();
    }
    let mut out = String::new();
    for c in s.chars() {
        let next = format!("{out}{c}…");
        if text_width(&next, font, size) > max_w {
            break;
        }
        out.push(c);
    }
    format!("{out}…")
}

/// "Nice" axis ticks covering [lo, hi].
fn nice_ticks(lo: f64, hi: f64) -> Vec<f64> {
    let (lo, hi) = if (hi - lo).abs() < 1e-12 { (lo - 1.0, hi + 1.0) } else { (lo, hi) };
    let raw = (hi - lo) / 4.0;
    let mag = 10f64.powf(raw.abs().log10().floor());
    let step = [1.0, 2.0, 2.5, 5.0, 10.0].iter().map(|m| m * mag).find(|s| *s >= raw).unwrap_or(10.0 * mag);
    if !step.is_finite() || step <= 0.0 {
        return vec![lo, hi];
    }
    let start = (lo / step).floor() * step;
    let mut t = Vec::new();
    let mut v = start;
    while v <= hi + step * 0.999 && t.len() < 12 {
        t.push(v);
        v += step;
    }
    t
}

fn xy_chart(l: &mut Layout, c: &ChartSpec, x0: f32, y0: f32, x1: f32, y1: f32) {
    let mut xs: Vec<String> = Vec::new();
    for s in &c.series {
        for (x, _) in &s.points {
            if !xs.contains(x) {
                xs.push(x.clone());
            }
        }
    }
    let vals: Vec<f64> = c.series.iter().flat_map(|s| s.points.iter().map(|p| p.1)).filter(|v| v.is_finite()).collect();
    if xs.is_empty() || vals.is_empty() {
        return;
    }
    let (mut lo, mut hi) = vals.iter().fold((f64::MAX, f64::MIN), |(a, b), v| (a.min(*v), b.max(*v)));
    if c.kind == ChartKind::Bar || (c.kind == ChartKind::Line && lo >= 0.0 && lo < hi * 0.4) {
        lo = lo.min(0.0);
        hi = hi.max(0.0);
    }
    let ticks = nice_ticks(lo, hi);
    let (tlo, thi) = (*ticks.first().unwrap_or(&lo), *ticks.last().unwrap_or(&hi));
    let span = if (thi - tlo).abs() < 1e-12 { 1.0 } else { thi - tlo };

    // Legend row at the top when there are named series.
    let named = c.series.len() > 1 || c.series.iter().any(|s| !s.name.trim().is_empty());
    let mut top = y1;
    if named {
        let mut lx = x0 + 40.0;
        for (i, s) in c.series.iter().enumerate() {
            let col = SERIES[i % SERIES.len()];
            l.rect(lx, top - 8.0, 8.0, 8.0, col);
            let name = if s.name.trim().is_empty() { format!("Series {}", i + 1) } else { s.name.clone() };
            l.text_at(lx + 11.0, top - 7.5, &name, Font::Regular, 8.0, BLACK);
            lx += 20.0 + text_width(&name, Font::Regular, 8.0);
        }
        top -= 16.0;
    }
    let tick_w = ticks.iter().map(|t| text_width(&fmt_num(*t), Font::Regular, 7.5)).fold(0.0f32, f32::max);
    let px0 = x0 + tick_w + 8.0;
    let px1 = x1;
    let py0 = y0 + 16.0;
    let py1 = top - 4.0;
    let ymap = |v: f64| py0 + ((v - tlo) / span) as f32 * (py1 - py0);

    for t in &ticks {
        let y = ymap(*t);
        l.line(px0, y, px1, y, Rgb(0.9, 0.9, 0.92), 0.4);
        let label = fmt_num(*t);
        let w = text_width(&label, Font::Regular, 7.5);
        l.text_at(px0 - 4.0 - w, y - 2.5, &label, Font::Regular, 7.5, GREY);
    }
    l.line(px0, py0, px0, py1, GREY, 0.6);
    let zero = if tlo <= 0.0 && thi >= 0.0 { ymap(0.0) } else { py0 };
    l.line(px0, zero, px1, zero, GREY, 0.6);

    let n = xs.len() as f32;
    let gw = (px1 - px0) / n;
    // X labels (skip some when crowded).
    let max_label = xs.iter().map(|x| text_width(x, Font::Regular, 7.5)).fold(0.0f32, f32::max);
    let every = ((max_label + 6.0) / gw.max(1.0)).ceil().max(1.0) as usize;
    for (i, x) in xs.iter().enumerate() {
        if i % every != 0 {
            continue;
        }
        let cx = px0 + gw * (i as f32 + 0.5);
        let label = ellipsize(x, Font::Regular, 7.5, gw * every as f32 - 4.0);
        l.text_center(cx, y0 + 4.0, &label, Font::Regular, 7.5, GREY);
    }

    match c.kind {
        ChartKind::Bar => {
            let ns = c.series.len().max(1) as f32;
            let inner = gw * 0.72;
            let bw = inner / ns;
            let show_vals = (xs.len() * c.series.len()) <= 24;
            for (si, s) in c.series.iter().enumerate() {
                let col = SERIES[si % SERIES.len()];
                for (x, v) in &s.points {
                    let Some(i) = xs.iter().position(|e| e == x) else { continue };
                    if !v.is_finite() {
                        continue;
                    }
                    let bx = px0 + gw * i as f32 + (gw - inner) / 2.0 + bw * si as f32;
                    let (ya, yb) = (zero, ymap(*v));
                    l.rect(bx, ya.min(yb), (bw - 1.5).max(1.0), (yb - ya).abs(), col);
                    if show_vals {
                        let label = fmt_num(*v);
                        let ly = if *v >= 0.0 { yb + 2.5 } else { yb - 8.5 };
                        l.text_center(bx + bw / 2.0, ly, &label, Font::Regular, 6.8, BLACK);
                    }
                }
            }
        }
        ChartKind::Line | ChartKind::Stock => {
            for (si, s) in c.series.iter().enumerate() {
                let col = SERIES[si % SERIES.len()];
                let pts: Vec<(f32, f32)> = s
                    .points
                    .iter()
                    .filter(|(_, v)| v.is_finite())
                    .filter_map(|(x, v)| xs.iter().position(|e| e == x).map(|i| (px0 + gw * (i as f32 + 0.5), ymap(*v))))
                    .collect();
                if pts.is_empty() {
                    continue;
                }
                if c.kind == ChartKind::Stock && c.series.len() == 1 && pts.len() > 1 {
                    // Light area under a single stock-style series.
                    let mut s = format!("0.86 0.9 0.97 rg {} {} m ", f(pts[0].0), f(py0));
                    for (x, y) in &pts {
                        let _ = write!(s, "{} {} l ", f(*x), f(*y));
                    }
                    let _ = write!(s, "{} {} l h f\n", f(pts[pts.len() - 1].0), f(py0));
                    l.ops(&s);
                }
                let mut s = format!("{} {} {} RG 1.6 w 1 J 1 j ", f(col.0), f(col.1), f(col.2));
                for (k, (x, y)) in pts.iter().enumerate() {
                    let _ = write!(s, "{} {} {} ", f(*x), f(*y), if k == 0 { "m" } else { "l" });
                }
                s.push_str("S 0 J 0 j\n");
                l.ops(&s);
                if pts.len() <= 40 && c.kind == ChartKind::Line {
                    for (x, y) in &pts {
                        l.rect(x - 2.0, y - 2.0, 4.0, 4.0, col);
                    }
                }
            }
            if c.kind == ChartKind::Stock
                && let Some(s) = c.series.first()
                && let Some((_, last)) = s.points.iter().rev().find(|p| p.1.is_finite())
            {
                let label = fmt_num(*last);
                l.text_at(px1 - text_width(&label, Font::Bold, 8.0), py1 - 2.0, &label, Font::Bold, 8.0, SERIES[0]);
            }
        }
        _ => {}
    }
}

fn circle_ops(cx: f32, cy: f32, r: f32) -> String {
    let k = 0.5523 * r;
    format!(
        "{} {} m {} {} {} {} {} {} c {} {} {} {} {} {} c {} {} {} {} {} {} c {} {} {} {} {} {} c ",
        f(cx + r), f(cy),
        f(cx + r), f(cy + k), f(cx + k), f(cy + r), f(cx), f(cy + r),
        f(cx - k), f(cy + r), f(cx - r), f(cy + k), f(cx - r), f(cy),
        f(cx - r), f(cy - k), f(cx - k), f(cy - r), f(cx), f(cy - r),
        f(cx + k), f(cy - r), f(cx + r), f(cy - k), f(cx + r), f(cy),
    )
}

fn venn(l: &mut Layout, c: &ChartSpec, bottom: f32, top: f32) {
    let cx = LEFT + TEXT_W / 2.0;
    let cy = (bottom + top) / 2.0 - 4.0;
    let r = ((top - bottom) / 2.0 - 24.0).min(if c.sets.len() == 3 { 50.0 } else { 66.0 });
    let d = r * 0.95;
    let centers: Vec<(f32, f32)> = match c.sets.len() {
        1 => vec![(cx, cy)],
        2 => vec![(cx - d / 2.0, cy), (cx + d / 2.0, cy)],
        _ => vec![(cx - d / 2.0, cy + d * 0.3), (cx + d / 2.0, cy + d * 0.3), (cx, cy - d * 0.55)],
    };
    for (i, (x, y)) in centers.iter().enumerate() {
        let col = SERIES[i % SERIES.len()];
        let s = format!("{} {} {} RG 1.8 w {}S\n", f(col.0), f(col.1), f(col.2), circle_ops(*x, *y, r));
        l.ops(&s);
    }
    for (i, (s, (x, y))) in c.sets.iter().zip(centers.iter()).enumerate() {
        let col = SERIES[i % SERIES.len()];
        let (dx, dy) = (x - cx, y - cy);
        let len = (dx * dx + dy * dy).sqrt().max(1.0);
        // Size inside the part of the circle away from the others; label outside the circle.
        let (sx, sy) = if c.sets.len() == 1 { (*x, *y) } else { (x + dx / len * r * 0.5, y + dy / len * r * 0.5) };
        l.text_center(sx, sy - 3.0, &fmt_num(s.size), Font::Regular, 9.0, BLACK);
        let below = c.sets.len() == 3 && i == 2;
        let ly = if below { y - r - 11.0 } else { y + r + 5.0 };
        let lx = if c.sets.len() == 1 { *x } else { x + dx / len * r * 0.35 };
        let name = ellipsize(&s.label, Font::Bold, 8.5, TEXT_W / 2.0 - 10.0);
        l.text_center(lx, ly, &name, Font::Bold, 8.5, col);
    }
    // Overlap values at the midpoint between the two centres.
    for (i, s) in c.sets.iter().enumerate() {
        for (j, v) in &s.overlaps {
            if *j <= i {
                continue;
            }
            if let (Some(a), Some(b)) = (centers.get(i), centers.get(*j)) {
                let (mx, my) = ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0);
                l.text_center(mx, my - 3.0, &fmt_num(*v), Font::Bold, 8.5, BLACK);
            }
        }
    }
}

fn arrow_head(l: &mut Layout, x: f32, y: f32, dx: f32, dy: f32, c: Rgb) {
    let len = (dx * dx + dy * dy).sqrt().max(0.001);
    let (ux, uy) = (dx / len, dy / len);
    let (px, py) = (-uy, ux);
    let s = 6.0;
    let w = 3.2;
    let ops = format!(
        "{} {} {} rg {} {} m {} {} l {} {} l h f\n",
        f(c.0), f(c.1), f(c.2),
        f(x), f(y),
        f(x - ux * s + px * w), f(y - uy * s + py * w),
        f(x - ux * s - px * w), f(y - uy * s - py * w)
    );
    l.ops(&ops);
}

fn flow(l: &mut Layout, c: &ChartSpec, bottom: f32, top: f32) {
    let n = c.nodes.len();
    if n == 0 {
        return;
    }
    let bw = 200.0f32;
    let bh = 22.0f32;
    let step = ((top - bottom - 20.0) / n as f32).max(bh + 8.0);
    let cx = LEFT + TEXT_W / 2.0 - 40.0;
    let boxes: Vec<(f32, f32)> = (0..n).map(|i| (cx, top - 14.0 - bh / 2.0 - step * i as f32)).collect();
    let line_col = Rgb(0.45, 0.47, 0.52);
    let mut arc = 0;
    for (from, to, label) in &c.edges {
        let (Some(a), Some(b)) = (boxes.get(*from), boxes.get(*to)) else { continue };
        if from == to {
            continue;
        }
        if *to == from + 1 {
            let (x, y1, y2) = (a.0, a.1 - bh / 2.0, b.1 + bh / 2.0);
            l.line(x, y1, x, y2 + 5.0, line_col, 1.0);
            arrow_head(l, x, y2, 0.0, -1.0, line_col);
            if !label.trim().is_empty() {
                l.text_at(x + 6.0, (y1 + y2) / 2.0 - 3.0, &ellipsize(label, Font::Italic, 7.5, 150.0), Font::Italic, 7.5, GREY);
            }
        } else {
            arc += 1;
            let forward = to > from;
            let side = if forward { 1.0 } else { -1.0 };
            let xe = a.0 + side * bw / 2.0;
            let off = side * (28.0 + 14.0 * arc as f32);
            let (ya, yb) = (a.1, b.1);
            let s = format!(
                "{} {} {} RG 1 w {} {} m {} {} {} {} {} {} c S\n",
                f(line_col.0), f(line_col.1), f(line_col.2),
                f(xe), f(ya), f(xe + off), f(ya), f(xe + off), f(yb), f(xe + 5.0 * side), f(yb)
            );
            l.ops(&s);
            arrow_head(l, xe, yb, -side, 0.0, line_col);
            if !label.trim().is_empty() {
                let lx = if forward { xe + off * 0.75 + 4.0 } else { xe + off * 0.75 - 4.0 - text_width(label, Font::Italic, 7.5).min(110.0) };
                l.text_at(lx, (ya + yb) / 2.0 - 3.0, &ellipsize(label, Font::Italic, 7.5, 110.0), Font::Italic, 7.5, GREY);
            }
        }
    }
    for (i, (x, y)) in boxes.iter().enumerate() {
        let col = SERIES[i % SERIES.len()];
        l.rect(x - bw / 2.0, y - bh / 2.0, bw, bh, Rgb(0.96, 0.97, 0.99));
        l.stroke_rect(x - bw / 2.0, y - bh / 2.0, bw, bh, col, 1.2);
        let name = ellipsize(c.nodes.get(i).map(String::as_str).unwrap_or(""), Font::Regular, 8.5, bw - 10.0);
        l.text_center(*x, y - 3.0, &name, Font::Regular, 8.5, BLACK);
    }
}

// ---------------------------------------------------------------------------------------------
// Writing the file
// ---------------------------------------------------------------------------------------------

fn write_pdf(mut l: Layout, a: &Article) -> Result<Vec<u8>> {
    let total = l.pages.len();
    // Footers: label on the left, page numbers on the right.
    let label = format!("{} · Gateless Scholar", a.kind.label());
    for (i, p) in l.pages.iter_mut().enumerate() {
        let num = format!("Page {} of {}", i + 1, total);
        let lab = ellipsize(&label, Font::Regular, 7.5, TEXT_W - 80.0);
        p.content.push_str(&format!("{} {} {} RG 0.4 w {} {} m {} {} l S\n", f(LIGHT.0), f(LIGHT.1), f(LIGHT.2), f(LEFT), f(BOTTOM - 22.0), f(RIGHT), f(BOTTOM - 22.0)));
        p.content.push_str(&text_op(LEFT, BOTTOM - 34.0, &encode(&lab), Font::Regular, 7.5, GREY, 0.0));
        let w = text_width(&num, Font::Regular, 7.5);
        p.content.push_str(&text_op(RIGHT - w, BOTTOM - 34.0, &encode(&num), Font::Regular, 7.5, GREY, 0.0));
    }

    // Object numbers: 1 catalog, 2 pages, 3-6 fonts, 7 info, then page + content pairs, then annots.
    let page_obj = |i: usize| 8 + i * 2;
    let content_obj = |i: usize| 9 + i * 2;
    let mut next_annot = 8 + total * 2;
    let mut objects: Vec<(usize, Vec<u8>)> = Vec::new();

    objects.push((1, b"<< /Type /Catalog /Pages 2 0 R /PageMode /UseNone >>".to_vec()));
    let kids: Vec<String> = (0..total).map(|i| format!("{} 0 R", page_obj(i))).collect();
    objects.push((2, format!("<< /Type /Pages /Kids [{}] /Count {total} >>", kids.join(" ")).into_bytes()));
    for (id, base) in [(3, "Helvetica"), (4, "Helvetica-Bold"), (5, "Helvetica-Oblique"), (6, "Helvetica-BoldOblique")] {
        objects.push((id, format!("<< /Type /Font /Subtype /Type1 /BaseFont /{base} /Encoding /WinAnsiEncoding >>").into_bytes()));
    }
    objects.push((
        7,
        format!(
            "<< /Title {} /Author {} /Subject {} /Creator (Gateless Scholar) /Producer (Gateless Scholar) /CreationDate (D:{}Z) >>",
            pdf_text_value(&a.title),
            pdf_text_value(if a.byline.trim().is_empty() { "Gateless Scholar" } else { a.byline.trim() }),
            pdf_text_value(a.kind.label()),
            a.created.format("%Y%m%d%H%M%S")
        )
        .into_bytes(),
    ));
    let dests = std::mem::take(&mut l.dests);
    for (i, p) in l.pages.iter().enumerate() {
        let mut annot_ids = Vec::new();
        for an in &p.annots {
            let action = match &an.target {
                Target::Uri(u) => format!("/A << /S /URI /URI {} >>", pdf_string(u.as_bytes())),
                Target::Dest(name) => match dests.get(name) {
                    Some((pg, y)) => format!("/Dest [{} 0 R /XYZ 0 {} null]", page_obj(*pg), f(*y)),
                    None => continue,
                },
            };
            let id = next_annot;
            next_annot += 1;
            annot_ids.push(format!("{id} 0 R"));
            objects.push((
                id,
                format!(
                    "<< /Type /Annot /Subtype /Link /Rect [{} {} {} {}] /Border [0 0 0] {action} >>",
                    f(an.rect[0]),
                    f(an.rect[1]),
                    f(an.rect[2]),
                    f(an.rect[3])
                )
                .into_bytes(),
            ));
        }
        let annots = if annot_ids.is_empty() { String::new() } else { format!(" /Annots [{}]", annot_ids.join(" ")) };
        objects.push((
            page_obj(i),
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {} {}] /Resources << /Font << /F1 3 0 R /F2 4 0 R /F3 5 0 R /F4 6 0 R >> >> /Contents {} 0 R{annots} >>",
                f(PAGE_W),
                f(PAGE_H),
                content_obj(i)
            )
            .into_bytes(),
        ));
        let mut c = format!("<< /Length {} >>\nstream\n", p.content.len()).into_bytes();
        c.extend_from_slice(p.content.as_bytes());
        c.extend_from_slice(b"\nendstream");
        objects.push((content_obj(i), c));
    }
    objects.sort_by_key(|(id, _)| *id);

    let mut out: Vec<u8> = b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n".to_vec();
    let max_id = objects.last().map(|o| o.0).unwrap_or(0);
    let mut offsets = vec![0usize; max_id + 1];
    for (id, body) in &objects {
        if let Some(slot) = offsets.get_mut(*id) {
            *slot = out.len();
        }
        out.extend_from_slice(format!("{id} 0 obj\n").as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    let mut x = format!("xref\n0 {}\n0000000000 65535 f \n", max_id + 1);
    for off in offsets.iter().skip(1) {
        if *off == 0 {
            x.push_str("0000000000 65535 f \n");
        } else {
            let _ = write!(x, "{off:010} 00000 n \n");
        }
    }
    let _ = write!(x, "trailer\n<< /Size {} /Root 1 0 R /Info 7 0 R >>\nstartxref\n{xref}\n%%EOF\n", max_id + 1);
    out.extend_from_slice(x.as_bytes());
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_winansi_and_fallbacks() {
        assert_eq!(encode("a–b"), vec![b'a', 150, b'b']);
        assert_eq!(encode("⟦x⟧"), b"[x]".to_vec());
        assert_eq!(encode("α"), b"alpha".to_vec());
        assert_eq!(encode("Łódź"), vec![b'L', 0xF3, b'd', b'z']);
        assert_eq!(pdf_string(b"(a)\\"), "(\\(a\\)\\\\)");
    }

    #[test]
    fn helvetica_widths() {
        // "Hello" in Helvetica: H 722 e 556 l 222 l 222 o 556 = 2278.
        assert!((text_width("Hello", Font::Regular, 10.0) - 22.78).abs() < 0.01);
    }

    #[test]
    fn ticks_cover_range() {
        let t = nice_ticks(0.0, 87.0);
        assert!(t.first().copied().unwrap_or(1.0) <= 0.0);
        assert!(t.last().copied().unwrap_or(0.0) >= 87.0);
        assert!(nice_ticks(5.0, 5.0).len() >= 2);
    }
}
