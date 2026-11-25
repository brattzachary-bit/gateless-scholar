//! .docx export: hand-written WordprocessingML in a zip.
//!
//! Marking: original text plain; AI-reworded original underlined with the original wording
//! as a footnote; AI-written text purple, with a purple left paragraph border (margin bar) on
//! every paragraph holding AI text, and a light tint behind AI runs in mixed paragraphs so the
//! difference survives greyscale printing. Citations "[n]" are green links to the reference
//! list; URLs are green external hyperlinks.

use std::io::Write as _;

use anyhow::{Context, Result};
use zip::write::SimpleFileOptions;

use super::*;

pub(super) const PURPLE: &str = "7B3FBF";
pub(super) const GREEN: &str = "1E8449";
const AI_TINT: &str = "EFE6FA";
const GREY: &str = "5F6368";
/// A4 text width in twentieths of a point (11906 - 1418 - 1134).
const TEXT_W: u32 = 9354;

const W_NS: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
const R_NS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

pub(super) fn render(a: &Article, style: CitationStyle) -> Result<Vec<u8>> {
    let mut d = DocBuilder::default();
    d.article(a, style);
    d.finish(a)
}

pub(super) fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            // Characters not allowed in XML 1.0.
            c if (c as u32) < 0x20 && !matches!(c, '\t' | '\n' | '\r') => {}
            '\u{FFFE}' | '\u{FFFF}' => {}
            _ => out.push(c),
        }
    }
    out
}

#[derive(Default, Clone)]
struct RunProps {
    bold: bool,
    italic: bool,
    underline: bool,
    color: Option<&'static str>,
    shade: Option<&'static str>,
    /// Half-points.
    size: Option<u32>,
    superscript: bool,
}

impl RunProps {
    fn xml(&self) -> String {
        let mut s = String::new();
        if self.bold {
            s.push_str("<w:b/>");
        }
        if self.italic {
            s.push_str("<w:i/>");
        }
        if let Some(c) = self.color {
            s.push_str(&format!("<w:color w:val=\"{c}\"/>"));
        }
        if let Some(sz) = self.size {
            s.push_str(&format!("<w:sz w:val=\"{sz}\"/><w:szCs w:val=\"{sz}\"/>"));
        }
        if self.underline {
            s.push_str("<w:u w:val=\"single\"/>");
        }
        if let Some(f) = self.shade {
            s.push_str(&format!("<w:shd w:val=\"clear\" w:color=\"auto\" w:fill=\"{f}\"/>"));
        }
        if self.superscript {
            s.push_str("<w:vertAlign w:val=\"superscript\"/>");
        }
        if s.is_empty() { s } else { format!("<w:rPr>{s}</w:rPr>") }
    }
}

fn run(text: &str, p: &RunProps) -> String {
    if text.is_empty() {
        return String::new();
    }
    format!("<w:r>{}<w:t xml:space=\"preserve\">{}</w:t></w:r>", p.xml(), xml_escape(text))
}

fn plain() -> RunProps {
    RunProps::default()
}

#[derive(Default)]
struct DocBuilder {
    body: String,
    /// External hyperlink targets; relationship ids are rIdLink{n}.
    links: Vec<String>,
    /// Footnote bodies (ids start at 1).
    footnotes: Vec<String>,
    bookmark_id: u32,
}

impl DocBuilder {
    fn link_id(&mut self, url: &str) -> String {
        let i = match self.links.iter().position(|u| u == url) {
            Some(i) => i,
            None => {
                self.links.push(url.to_string());
                self.links.len() - 1
            }
        };
        format!("rIdLink{}", i + 1)
    }

    fn hyperlink(&mut self, text: &str, url: &str, extra: &RunProps) -> String {
        let id = self.link_id(url);
        let mut p = extra.clone();
        p.color = Some(GREEN);
        p.underline = true;
        format!("<w:hyperlink r:id=\"{id}\" w:history=\"1\">{}</w:hyperlink>", run(text, &p))
    }

    /// Text that may contain the given URL: the URL part becomes a green hyperlink.
    fn text_with_link(&mut self, text: &str, url: Option<&str>, p: &RunProps) -> String {
        if let Some(u) = url
            && !u.is_empty()
            && let Some(i) = text.find(u)
        {
            let (before, rest) = text.split_at(i);
            let after = &rest[u.len()..];
            let link = self.hyperlink(u, u, p);
            return format!("{}{}{}", run(before, p), link, run(after, p));
        }
        run(text, p)
    }

    fn citations(&mut self, c: &[u32], base: &RunProps) -> String {
        if c.is_empty() {
            return String::new();
        }
        let mut p = base.clone();
        p.color = Some(GREEN);
        p.underline = false;
        p.shade = None;
        let mut s = run(" [", &p);
        for (i, n) in c.iter().enumerate() {
            if i > 0 {
                s.push_str(&run(", ", &p));
            }
            s.push_str(&format!("<w:hyperlink w:anchor=\"ref_{n}\" w:history=\"1\">{}</w:hyperlink>", run(&n.to_string(), &p)));
        }
        s.push_str(&run("]", &p));
        s
    }

    fn para(&mut self, ppr: &str, content: &str) {
        self.body.push_str(&format!("<w:p><w:pPr>{ppr}</w:pPr>{content}</w:p>"));
    }

    fn styled(&mut self, style: &str, content: &str) {
        self.para(&format!("<w:pStyle w:val=\"{style}\"/>"), content);
    }

    fn ai_bar() -> String {
        format!("<w:pBdr><w:left w:val=\"single\" w:sz=\"24\" w:space=\"8\" w:color=\"{PURPLE}\"/></w:pBdr>")
    }

    fn footnote(&mut self, text: &str) -> String {
        self.footnotes.push(text.to_string());
        let id = self.footnotes.len();
        format!("<w:r><w:rPr><w:vertAlign w:val=\"superscript\"/></w:rPr><w:footnoteReference w:id=\"{id}\"/></w:r>")
    }

    fn article(&mut self, a: &Article, style: CitationStyle) {
        let f = front(a);
        let fill = if a.kind == ArticleKind::FullText { "37474F" } else { "5B2A86" };
        let banner = run(
            &format!("  {}  ", f.label),
            &RunProps { bold: true, color: Some("FFFFFF"), size: Some(22), ..plain() },
        );
        self.para(
            &format!("<w:shd w:val=\"clear\" w:color=\"auto\" w:fill=\"{fill}\"/><w:spacing w:before=\"0\" w:after=\"240\"/>"),
            &banner,
        );
        self.styled("Title", &run(&f.title, &plain()));
        if let Some(s) = &f.standfirst {
            self.styled("Subtitle", &run(s, &plain()));
        }
        let meta = RunProps { color: Some(GREY), ..plain() };
        self.styled("Meta", &run(&f.byline, &RunProps { bold: true, ..meta.clone() }));
        self.styled("Meta", &run(&f.generated, &meta));
        if let Some((t, url)) = &f.original {
            let mut c = run(t, &meta);
            if let Some(u) = url {
                c.push_str(&run(" — ", &meta));
                c.push_str(&self.hyperlink(u, u, &plain()));
            }
            self.styled("Meta", &c);
        }
        if let Some(img) = &f.hero {
            self.image(img);
        }

        // Legend, each line in its own marking style.
        self.styled("LegendHeading", &run("How the text is marked", &plain()));
        let small = RunProps { size: Some(19), ..plain() };
        self.styled(
            "Legend",
            &format!("{}{}", run("Original text", &RunProps { bold: true, ..small.clone() }), run(LEGEND[0].1.strip_prefix("Original text").unwrap_or(""), &small)),
        );
        self.styled(
            "Legend",
            &format!(
                "{}{}{}",
                run("AI-reworded original", &RunProps { bold: true, underline: true, ..small.clone() }),
                run(LEGEND[1].1.strip_prefix("AI-reworded original").unwrap_or(""), &RunProps { underline: true, ..small.clone() }),
                run(" Underlined; the original wording is in the footnotes.", &small)
            ),
        );
        let ai = RunProps { color: Some(PURPLE), ..small.clone() };
        self.para(
            &format!("<w:pStyle w:val=\"Legend\"/>{}", Self::ai_bar()),
            &format!(
                "{}{}{}",
                run("AI-written", &RunProps { bold: true, ..ai.clone() }),
                run(LEGEND[2].1.strip_prefix("AI-written").unwrap_or(""), &ai),
                run(" Purple, with a bar in the left margin.", &ai)
            ),
        );
        let green = RunProps { color: Some(GREEN), ..small.clone() };
        self.styled(
            "Legend",
            &format!(
                "{}{}{}{}",
                run("[n]", &green),
                run(" links to source n in the references; links to real sources are green. ", &small),
                run("[unverified]", &RunProps { italic: true, ..small.clone() }),
                run(" marks a claim with no supporting source.", &small)
            ),
        );

        let mut fig = 0;
        for sec in &a.sections {
            if !sec.heading.trim().is_empty() {
                self.styled("Heading1", &run(sec.heading.trim(), &plain()));
            }
            for b in &sec.blocks {
                match b {
                    Block::Heading(h) => self.styled("Heading2", &run(h.trim(), &plain())),
                    Block::Paragraph(ps) => self.paragraph(ps),
                    Block::Chart(c) => {
                        fig += 1;
                        self.chart(c, fig);
                    }
                    Block::Image(img) => self.image(img),
                }
            }
        }

        let refs = reference_entries(a, style);
        if !refs.is_empty() {
            self.styled("Heading1", &run(&references_heading(style), &plain()));
            for r in refs {
                self.bookmark_id += 1;
                let bid = self.bookmark_id;
                let mut c = format!("<w:bookmarkStart w:id=\"{bid}\" w:name=\"ref_{}\"/>", r.n);
                c.push_str(&run(&format!("[{}] ", r.n), &RunProps { bold: true, ..plain() }));
                c.push_str(&format!("<w:bookmarkEnd w:id=\"{bid}\"/>"));
                c.push_str(&self.text_with_link(&r.text, r.link.as_deref(), &plain()));
                self.styled("Reference", &c);
            }
        }
        let notes = validation_lines(a);
        if !notes.is_empty() {
            self.styled("Heading2", &run("Validation notes", &plain()));
            for n in notes {
                self.styled("Reference", &run(&format!("• {n}"), &plain()));
            }
        }
        self.styled(
            "Meta",
            &run(
                &format!("{}. Exported from Gateless Scholar. Cite the original sources listed above, not this text.", f.label),
                &RunProps { italic: true, color: Some(GREY), size: Some(18), ..plain() },
            ),
        );
    }

    fn paragraph(&mut self, ps: &[Passage]) {
        if ps.is_empty() {
            return;
        }
        let has_ai = ps.iter().any(|p| p.origin == Origin::AiWritten);
        let mixed = has_ai && ps.iter().any(|p| p.origin != Origin::AiWritten);
        let mut c = String::new();
        for (i, p) in ps.iter().enumerate() {
            let sep = if i > 0 { " " } else { "" };
            let props = match p.origin {
                Origin::Original => plain(),
                Origin::Reworded => RunProps { underline: true, ..plain() },
                Origin::AiWritten => RunProps { color: Some(PURPLE), shade: mixed.then_some(AI_TINT), ..plain() },
            };
            let sep_props = if p.origin == Origin::AiWritten { RunProps { underline: false, ..props.clone() } } else { plain() };
            c.push_str(&run(sep, &sep_props));
            c.push_str(&run(p.text.trim(), &props));
            if p.origin == Origin::Reworded
                && let Some(o) = p.original.as_ref().filter(|o| !o.trim().is_empty())
            {
                c.push_str(&self.footnote(&format!("Original wording: “{}”", o.trim())));
            }
            if p.unverified {
                c.push_str(&run(" [unverified]", &RunProps { italic: true, underline: false, ..props.clone() }));
            }
            let base = RunProps { underline: false, ..props.clone() };
            c.push_str(&self.citations(&p.citations, &base));
        }
        let ppr = if has_ai { Self::ai_bar() } else { String::new() };
        self.para(&ppr, &c);
    }

    fn image(&mut self, img: &ImageRef) {
        let cap = RunProps { italic: true, color: Some(GREY), size: Some(18), ..plain() };
        let mut c = run("Image: ", &RunProps { bold: true, ..cap.clone() });
        c.push_str(&run(&image_caption(img), &cap));
        if let Some(l) = image_link(img) {
            c.push_str(&run(" Source: ", &cap));
            c.push_str(&self.hyperlink(&l, &l, &RunProps { size: Some(18), ..plain() }));
        }
        if let Some(l) = &img.licence_url {
            c.push_str(&run(" Licence: ", &cap));
            c.push_str(&self.hyperlink(l, l, &RunProps { size: Some(18), ..plain() }));
        }
        self.styled("Caption", &c);
    }

    fn chart(&mut self, ch: &ChartSpec, n: usize) {
        self.styled("Caption", &run(&format!("Figure {n}: {}", ch.title.trim()), &RunProps { bold: true, ..plain() }));
        let (header, rows) = chart_table(ch);
        let cols = header.len().max(1) as u32;
        let cw = TEXT_W / cols;
        let mut t = String::from(
            "<w:tbl><w:tblPr><w:tblW w:w=\"0\" w:type=\"auto\"/><w:tblBorders>\
             <w:top w:val=\"single\" w:sz=\"4\" w:color=\"BFBFBF\"/><w:left w:val=\"single\" w:sz=\"4\" w:color=\"BFBFBF\"/>\
             <w:bottom w:val=\"single\" w:sz=\"4\" w:color=\"BFBFBF\"/><w:right w:val=\"single\" w:sz=\"4\" w:color=\"BFBFBF\"/>\
             <w:insideH w:val=\"single\" w:sz=\"4\" w:color=\"BFBFBF\"/><w:insideV w:val=\"single\" w:sz=\"4\" w:color=\"BFBFBF\"/>\
             </w:tblBorders><w:tblLook w:val=\"04A0\" w:firstRow=\"1\"/></w:tblPr><w:tblGrid>",
        );
        for _ in 0..cols {
            t.push_str(&format!("<w:gridCol w:w=\"{cw}\"/>"));
        }
        t.push_str("</w:tblGrid>");
        let cell = |text: &str, head: bool| {
            let shade = if head { "<w:shd w:val=\"clear\" w:color=\"auto\" w:fill=\"EDE7F6\"/>" } else { "" };
            format!(
                "<w:tc><w:tcPr><w:tcW w:w=\"{cw}\" w:type=\"dxa\"/>{shade}</w:tcPr><w:p><w:pPr><w:pStyle w:val=\"TableText\"/></w:pPr>{}</w:p></w:tc>",
                run(text, &RunProps { bold: head, ..plain() })
            )
        };
        t.push_str("<w:tr><w:trPr><w:tblHeader/></w:trPr>");
        for h in &header {
            t.push_str(&cell(h, true));
        }
        t.push_str("</w:tr>");
        for r in &rows {
            t.push_str("<w:tr>");
            for i in 0..header.len() {
                t.push_str(&cell(r.get(i).map(String::as_str).unwrap_or(""), false));
            }
            t.push_str("</w:tr>");
        }
        t.push_str("</w:tbl>");
        self.body.push_str(&t);
        let mut cap = String::new();
        let mut axes = Vec::new();
        if let Some(x) = ch.x_label.as_ref().filter(|s| !s.trim().is_empty()) {
            axes.push(format!("x: {x}"));
        }
        if let Some(y) = ch.y_label.as_ref().filter(|s| !s.trim().is_empty()) {
            axes.push(format!("y: {y}"));
        }
        let capp = RunProps { italic: true, color: Some(GREY), size: Some(18), ..plain() };
        if !axes.is_empty() {
            cap.push_str(&run(&format!("({}) ", axes.join("; ")), &capp));
        }
        let text = chart_caption(ch);
        // Make the data-source numbers links to the references.
        match text.find("data from [") {
            Some(i) => {
                cap.push_str(&run(&text[..i + "data from".len()], &capp));
                cap.push_str(&self.citations(&ch.citations, &capp));
                cap.push_str(&run(".", &capp));
            }
            None => cap.push_str(&run(&text, &capp)),
        }
        self.styled("Caption", &cap);
    }

    fn finish(self, a: &Article) -> Result<Vec<u8>> {
        let document = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
             <w:document xmlns:w=\"{W_NS}\" xmlns:r=\"{R_NS}\"><w:body>{}\
             <w:sectPr><w:footerReference w:type=\"default\" r:id=\"rIdFooter\"/>\
             <w:pgSz w:w=\"11906\" w:h=\"16838\"/>\
             <w:pgMar w:top=\"1134\" w:right=\"1134\" w:bottom=\"1134\" w:left=\"1418\" w:header=\"567\" w:footer=\"567\" w:gutter=\"0\"/>\
             </w:sectPr></w:body></w:document>",
            self.body
        );

        let mut rels = String::from(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
             <Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
             <Relationship Id=\"rIdStyles\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles\" Target=\"styles.xml\"/>\
             <Relationship Id=\"rIdSettings\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/settings\" Target=\"settings.xml\"/>\
             <Relationship Id=\"rIdFootnotes\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/footnotes\" Target=\"footnotes.xml\"/>\
             <Relationship Id=\"rIdFooter\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/footer\" Target=\"footer1.xml\"/>",
        );
        for (i, url) in self.links.iter().enumerate() {
            rels.push_str(&format!(
                "<Relationship Id=\"rIdLink{}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink\" Target=\"{}\" TargetMode=\"External\"/>",
                i + 1,
                xml_escape(url)
            ));
        }
        rels.push_str("</Relationships>");

        let mut footnotes = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<w:footnotes xmlns:w=\"{W_NS}\" xmlns:r=\"{R_NS}\">\
             <w:footnote w:type=\"separator\" w:id=\"-1\"><w:p><w:r><w:separator/></w:r></w:p></w:footnote>\
             <w:footnote w:type=\"continuationSeparator\" w:id=\"0\"><w:p><w:r><w:continuationSeparator/></w:r></w:p></w:footnote>"
        );
        for (i, text) in self.footnotes.iter().enumerate() {
            footnotes.push_str(&format!(
                "<w:footnote w:id=\"{}\"><w:p><w:pPr><w:pStyle w:val=\"FootnoteText\"/></w:pPr>\
                 <w:r><w:rPr><w:vertAlign w:val=\"superscript\"/></w:rPr><w:footnoteRef/></w:r>{}</w:p></w:footnote>",
                i + 1,
                run(&format!(" {text}"), &plain())
            ));
        }
        footnotes.push_str("</w:footnotes>");

        let label = xml_escape(a.kind.label());
        let footer = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<w:ftr xmlns:w=\"{W_NS}\" xmlns:r=\"{R_NS}\">\
             <w:p><w:pPr><w:pStyle w:val=\"Footer\"/><w:tabs><w:tab w:val=\"right\" w:pos=\"{TEXT_W}\"/></w:tabs></w:pPr>\
             <w:r><w:t xml:space=\"preserve\">{label} · Gateless Scholar</w:t></w:r><w:r><w:tab/></w:r>\
             <w:r><w:t xml:space=\"preserve\">Page </w:t></w:r>\
             <w:r><w:fldChar w:fldCharType=\"begin\"/></w:r><w:r><w:instrText xml:space=\"preserve\"> PAGE </w:instrText></w:r>\
             <w:r><w:fldChar w:fldCharType=\"separate\"/></w:r><w:r><w:t>1</w:t></w:r><w:r><w:fldChar w:fldCharType=\"end\"/></w:r>\
             <w:r><w:t xml:space=\"preserve\"> of </w:t></w:r>\
             <w:r><w:fldChar w:fldCharType=\"begin\"/></w:r><w:r><w:instrText xml:space=\"preserve\"> NUMPAGES </w:instrText></w:r>\
             <w:r><w:fldChar w:fldCharType=\"separate\"/></w:r><w:r><w:t>1</w:t></w:r><w:r><w:fldChar w:fldCharType=\"end\"/></w:r>\
             </w:p></w:ftr>"
        );

        let core = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
             <cp:coreProperties xmlns:cp=\"http://schemas.openxmlformats.org/package/2006/metadata/core-properties\" \
             xmlns:dc=\"http://purl.org/dc/elements/1.1/\" xmlns:dcterms=\"http://purl.org/dc/terms/\" \
             xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\">\
             <dc:title>{}</dc:title><dc:creator>{}</dc:creator><dc:description>{label}</dc:description>\
             <dcterms:created xsi:type=\"dcterms:W3CDTF\">{}</dcterms:created></cp:coreProperties>",
            xml_escape(&a.title),
            xml_escape(if a.byline.trim().is_empty() { "Gateless Scholar" } else { a.byline.trim() }),
            a.created.format("%Y-%m-%dT%H:%M:%SZ")
        );

        let files: Vec<(&str, String)> = vec![
            ("[Content_Types].xml", CONTENT_TYPES.to_string()),
            ("_rels/.rels", ROOT_RELS.to_string()),
            ("docProps/core.xml", core),
            ("word/document.xml", document),
            ("word/_rels/document.xml.rels", rels),
            ("word/styles.xml", styles_xml()),
            ("word/settings.xml", SETTINGS.replace("{W_NS}", W_NS)),
            ("word/footnotes.xml", footnotes),
            ("word/footer1.xml", footer),
        ];
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut buf);
            let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
            for (name, content) in files {
                zip.start_file(name, opts).with_context(|| format!("zip entry {name}"))?;
                zip.write_all(content.as_bytes()).with_context(|| format!("zip entry {name}"))?;
            }
            zip.finish().context("finishing the .docx zip")?;
        }
        Ok(buf.into_inner())
    }
}

const CONTENT_TYPES: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\
<Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\
<Default Extension=\"xml\" ContentType=\"application/xml\"/>\
<Override PartName=\"/word/document.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml\"/>\
<Override PartName=\"/word/styles.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml\"/>\
<Override PartName=\"/word/settings.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.settings+xml\"/>\
<Override PartName=\"/word/footnotes.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.footnotes+xml\"/>\
<Override PartName=\"/word/footer1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.footer+xml\"/>\
<Override PartName=\"/docProps/core.xml\" ContentType=\"application/vnd.openxmlformats-package.core-properties+xml\"/>\
</Types>";

const ROOT_RELS: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
<Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"word/document.xml\"/>\
<Relationship Id=\"rId2\" Type=\"http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties\" Target=\"docProps/core.xml\"/>\
</Relationships>";

const SETTINGS: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
<w:settings xmlns:w=\"{W_NS}\"><w:defaultTabStop w:val=\"720\"/>\
<w:footnotePr><w:footnote w:id=\"-1\"/><w:footnote w:id=\"0\"/></w:footnotePr>\
<w:compat><w:compatSetting w:name=\"compatibilityMode\" w:uri=\"http://schemas.microsoft.com/office/word\" w:val=\"15\"/></w:compat>\
</w:settings>";

fn styles_xml() -> String {
    let font = "<w:rFonts w:ascii=\"Arial\" w:hAnsi=\"Arial\" w:eastAsia=\"Arial\" w:cs=\"Arial\"/>";
    let para = |id: &str, name: &str, ppr: &str, rpr: &str| {
        format!(
            "<w:style w:type=\"paragraph\" w:customStyle=\"1\" w:styleId=\"{id}\"><w:name w:val=\"{name}\"/><w:basedOn w:val=\"Normal\"/>\
             <w:qFormat/><w:pPr>{ppr}</w:pPr><w:rPr>{rpr}</w:rPr></w:style>"
        )
    };
    let mut s = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<w:styles xmlns:w=\"{W_NS}\">\
         <w:docDefaults><w:rPrDefault><w:rPr>{font}<w:sz w:val=\"22\"/><w:szCs w:val=\"22\"/><w:lang w:val=\"en-GB\"/></w:rPr></w:rPrDefault>\
         <w:pPrDefault><w:pPr><w:spacing w:after=\"160\" w:line=\"300\" w:lineRule=\"auto\"/></w:pPr></w:pPrDefault></w:docDefaults>\
         <w:style w:type=\"paragraph\" w:default=\"1\" w:styleId=\"Normal\"><w:name w:val=\"Normal\"/><w:qFormat/><w:rPr>{font}</w:rPr></w:style>\
         <w:style w:type=\"character\" w:default=\"1\" w:styleId=\"DefaultParagraphFont\"><w:name w:val=\"Default Paragraph Font\"/><w:uiPriority w:val=\"1\"/><w:semiHidden/></w:style>"
    );
    s.push_str(&para("Title", "Title", "<w:spacing w:before=\"120\" w:after=\"120\" w:line=\"240\" w:lineRule=\"auto\"/>", "<w:b/><w:sz w:val=\"40\"/><w:szCs w:val=\"40\"/><w:color w:val=\"1F1F1F\"/>"));
    s.push_str(&para("Subtitle", "Subtitle", "<w:spacing w:after=\"160\"/>", "<w:sz w:val=\"24\"/><w:color w:val=\"444444\"/>"));
    s.push_str(&para("Meta", "Meta", "<w:spacing w:after=\"40\"/>", "<w:sz w:val=\"19\"/>"));
    s.push_str(&para(
        "Heading1",
        "heading 1",
        "<w:keepNext/><w:spacing w:before=\"360\" w:after=\"120\"/><w:outlineLvl w:val=\"0\"/>",
        "<w:b/><w:sz w:val=\"30\"/><w:szCs w:val=\"30\"/><w:color w:val=\"1F1F1F\"/>",
    ));
    s.push_str(&para(
        "Heading2",
        "heading 2",
        "<w:keepNext/><w:spacing w:before=\"240\" w:after=\"80\"/><w:outlineLvl w:val=\"1\"/>",
        "<w:b/><w:sz w:val=\"24\"/><w:szCs w:val=\"24\"/><w:color w:val=\"333333\"/>",
    ));
    s.push_str(&para(
        "LegendHeading",
        "Legend Heading",
        "<w:keepNext/><w:pBdr><w:top w:val=\"single\" w:sz=\"4\" w:space=\"6\" w:color=\"BFBFBF\"/></w:pBdr><w:spacing w:before=\"240\" w:after=\"60\"/>",
        "<w:b/><w:sz w:val=\"19\"/><w:color w:val=\"5F6368\"/>",
    ));
    s.push_str(&para("Legend", "Legend", "<w:spacing w:after=\"40\"/>", "<w:sz w:val=\"19\"/>"));
    s.push_str(&para("Caption", "caption", "<w:spacing w:before=\"80\" w:after=\"160\"/>", "<w:sz w:val=\"18\"/>"));
    s.push_str(&para("TableText", "Table Text", "<w:spacing w:after=\"0\" w:line=\"240\" w:lineRule=\"auto\"/>", "<w:sz w:val=\"18\"/>"));
    s.push_str(&para("Reference", "Reference", "<w:spacing w:after=\"80\"/><w:ind w:left=\"567\" w:hanging=\"567\"/>", "<w:sz w:val=\"19\"/>"));
    s.push_str(&para("FootnoteText", "footnote text", "<w:spacing w:after=\"0\" w:line=\"240\" w:lineRule=\"auto\"/>", "<w:sz w:val=\"18\"/>"));
    s.push_str(&para("Footer", "footer", "<w:spacing w:after=\"0\"/>", "<w:sz w:val=\"16\"/><w:color w:val=\"5F6368\"/>"));
    s.push_str("</w:styles>");
    s
}
