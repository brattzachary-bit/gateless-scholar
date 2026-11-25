//! Shared widgets: icon buttons, badges, feed/search cards, the document actions menu, and the
//! origin-marked paragraph renderer used by the reader, the ask panel and notes.

use std::collections::{HashMap, HashSet};

use craft_ui::buttons::{ButtonVisuals, IconButton, IconButtonStyle};
use egui::text::{LayoutJob, TextFormat, TextWrapping};
use egui::{Align, Color32, CornerRadius, Rect, Response, Sense, Stroke, Ui, Vec2, pos2, vec2};

use super::icons;
use super::theme::{self, Palette};
use crate::app::{Action, ImgState, View};
use crate::model::*;

pub fn icon_button(ui: &mut Ui, pal: &Palette, icon: &str, tip: &str, selected: bool) -> Response {
    icon_button_sized(ui, pal, icon, tip, selected, 30.0, None)
}

pub fn icon_button_sized(ui: &mut Ui, pal: &Palette, icon: &str, tip: &str, selected: bool, size: f32, tint: Option<Color32>) -> Response {
    let normal = ButtonVisuals { fill: Color32::TRANSPARENT, stroke: Stroke::NONE, icon: tint.unwrap_or(pal.text_weak) };
    let style = IconButtonStyle {
        normal,
        hovered: ButtonVisuals { fill: pal.card_hover, icon: tint.unwrap_or(pal.text), ..normal },
        pressed: None,
        selected: ButtonVisuals { fill: pal.accent_soft, icon: tint.unwrap_or(pal.accent), ..normal },
        corner_radius: CornerRadius::same(7),
        focus_stroke: Stroke::new(1.0, pal.accent),
    };
    let icon_size = (size * 0.56).round();
    IconButton::new(tip, Vec2::splat(size), &style)
        .selected(selected)
        .show(ui, |ui, rect, tint| {
            icons::icon(icon, icon_size, tint).paint_at(ui, Rect::from_center_size(rect.center(), Vec2::splat(icon_size)));
        })
        .on_hover_text(tip)
}

pub fn badge(ui: &mut Ui, text: &str, color: Color32) -> Response {
    let font = theme::regular(11.5);
    let galley = ui.painter().layout_no_wrap(text.to_string(), font, color);
    let size = galley.size() + vec2(14.0, 6.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    ui.painter().rect(rect, 9.0, color.gamma_multiply(0.14), Stroke::new(1.0, color.gamma_multiply(0.5)), egui::StrokeKind::Inside);
    ui.painter().galley(rect.center() - galley.size() / 2.0, galley, color);
    resp
}

pub fn kind_tip(kind: ContentKind) -> &'static str {
    match kind {
        ContentKind::FreeFullText => "Free full text — a legal free copy exists (open access, preprint or author version).",
        ContentKind::AbstractOnly => "Abstract only — only the abstract is free. Opening it writes an AI interpretation built from real sources, not the original paper.",
        ContentKind::AiArticle => "AI article — written entirely by the AI from the papers it lists. Not a real paper.",
        ContentKind::News => "News — one combined story built from many outlets, with references to each.",
    }
}

pub fn kind_badge(ui: &mut Ui, pal: &Palette, kind: ContentKind) -> Response {
    badge(ui, kind.badge(), pal.badge(kind)).on_hover_text(kind_tip(kind))
}

pub fn kind_icon(kind: ContentKind) -> &'static str {
    match kind {
        ContentKind::FreeFullText => "book-open",
        ContentKind::AbstractOnly => "file",
        ContentKind::AiArticle => "sparkle",
        ContentKind::News => "newspaper",
    }
}

/// A small round icon "dot" that shows its full label on hover (keeps the reader uncluttered).
pub fn icon_dot(ui: &mut Ui, icon: &str, color: Color32, tip: &str) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(24.0), Sense::click());
    let hovered = resp.hovered();
    ui.painter().circle(rect.center(), 11.5, color.gamma_multiply(if hovered { 0.28 } else { 0.16 }), Stroke::new(1.0, color.gamma_multiply(0.55)));
    icons::icon(icon, 13.0, color).paint_at(ui, Rect::from_center_size(rect.center(), Vec2::splat(13.0)));
    resp.on_hover_text(tip).on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Call at the end of a scroll area's contents: if the text field being typed in is hidden
/// (e.g. under the on-screen keyboard), scroll it into view.
pub fn keep_focused_visible(ui: &mut Ui) {
    if let Some(id) = ui.ctx().memory(|m| m.focused())
        && let Some(r) = ui.ctx().read_response(id)
    {
        let clip = ui.clip_rect();
        if r.rect.bottom() > clip.bottom() - 8.0 && r.rect.top() >= clip.top() - 400.0 && ui.ctx().egui_wants_keyboard_input() {
            ui.scroll_to_rect(r.rect, Some(Align::Center));
        }
    }
}

pub fn heading(ui: &mut Ui, pal: &Palette, text: &str, sub: Option<&str>) {
    ui.add_space(6.0);
    ui.label(egui::RichText::new(text).font(theme::bold(24.0)).color(pal.text));
    if let Some(s) = sub {
        ui.label(egui::RichText::new(s).color(pal.text_weak));
    }
    ui.add_space(10.0);
}

/// Page title with actions on the right, and its explanation on the line below.
pub fn title_row(ui: &mut Ui, pal: &Palette, title: &str, sub: Option<&str>, right: impl FnOnce(&mut Ui)) {
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(title).font(theme::bold(24.0)).color(pal.text));
        ui.with_layout(egui::Layout::right_to_left(Align::Center), right);
    });
    if let Some(s) = sub {
        ui.add(egui::Label::new(egui::RichText::new(s).color(pal.text_weak)).wrap());
    }
    ui.add_space(10.0);
}

pub fn empty_state(ui: &mut Ui, pal: &Palette, icon: &str, title: &str, body: &str) {
    ui.add_space(40.0);
    ui.vertical_centered(|ui| {
        ui.add(icons::icon(icon, 40.0, pal.text_faint));
        ui.add_space(8.0);
        ui.label(egui::RichText::new(title).size(17.0).color(pal.text));
        ui.add(egui::Label::new(egui::RichText::new(body).color(pal.text_weak)).wrap());
    });
    ui.add_space(40.0);
}

/// Centered column of at most `max` points wide that keeps the full available height (so a
/// scroll area inside it reaches the bottom of the window).
pub fn column<R>(ui: &mut Ui, max: f32, add: impl FnOnce(&mut Ui) -> R) -> R {
    let avail = ui.available_rect_before_wrap();
    let w = avail.width().min(max);
    let x = avail.left() + ((avail.width() - w) / 2.0).max(0.0);
    let rect = Rect::from_min_max(pos2(x, avail.top()), pos2(x + w, avail.bottom()));
    ui.scope_builder(egui::UiBuilder::new().max_rect(rect).layout(egui::Layout::top_down(Align::Min)), |ui| {
        ui.set_width(w);
        add(ui)
    })
    .inner
}

pub fn legend(ui: &mut Ui, pal: &Palette) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 14.0;
        ui.label(egui::RichText::new("Original text").color(pal.text)).on_hover_text("Verbatim from the source");
        let r = ui.label(egui::RichText::new("AI-reworded original").color(pal.text)).on_hover_text("The original, lightly reworded for flow. Hover it in the text to see the original wording.");
        ui.painter().line_segment([r.rect.left_bottom() + vec2(0.0, -1.0), r.rect.right_bottom() + vec2(0.0, -1.0)], Stroke::new(1.0, pal.reworded.gamma_multiply(0.4)));
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let (r, _) = ui.allocate_exact_size(vec2(3.0, 16.0), Sense::hover());
            ui.painter().rect_filled(r, 1.0, pal.ai);
            ui.label(egui::RichText::new("AI-written").color(pal.ai));
        })
        .response
        .on_hover_text("Written by the AI from the cited sources. Marked by colour and a margin bar.");
        ui.label(egui::RichText::new("[1] link to a real source").color(pal.link));
    });
}

// ---------------------------------------------------------------------------------------------
// Cards
// ---------------------------------------------------------------------------------------------

/// Read-only context for drawing cards.
pub struct CardEnv<'a> {
    pub pal: &'a Palette,
    pub images: &'a HashMap<String, ImgState>,
    /// Photos found for documents that had none.
    pub found_images: &'a HashMap<String, Option<ImageRef>>,
    pub brand: &'a str,
    pub favourites: &'a HashSet<String>,
    pub read_later: &'a HashSet<String>,
    pub written: &'a HashSet<String>,
    pub shelves: &'a [Bookshelf],
    pub projects: &'a [Project],
    pub notes: &'a [Note],
}

impl CardEnv<'_> {
    /// The document's own image, or the one found for it. Asks for one when there is neither.
    pub fn image_for(&self, doc: &Doc, acts: &mut Vec<Action>) -> Option<ImageRef> {
        if let Some(i) = doc.image() {
            return Some(i.clone());
        }
        match self.found_images.get(&doc.key()) {
            Some(found) => found.clone(),
            None => {
                acts.push(Action::FindImage(doc.clone()));
                None
            }
        }
    }
}

const CARD_IMAGE: Vec2 = Vec2::new(150.0, 108.0);

/// A Discover / search card: image left; badges, title, 1–2 lines of abstract with "Show more",
/// authors, and actions on the right.
pub fn doc_card(ui: &mut Ui, env: &CardEnv, doc: &Doc, expanded: &mut bool, acts: &mut Vec<Action>) -> Response {
    let pal = env.pal;
    let key = doc.key();
    let id = ui.make_persistent_id(("card", &key));
    let hovered_before = ui.ctx().data(|d| d.get_temp::<bool>(id).unwrap_or(false));
    let frame = egui::Frame::new()
        .fill(if hovered_before { pal.card_hover } else { pal.card })
        .stroke(Stroke::new(1.0, pal.border))
        .corner_radius(12)
        .inner_margin(12);
    let resp = frame
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_top(|ui| {
                // Phones: a smaller picture so the text keeps most of the width.
                let compact = ui.available_width() < 560.0;
                card_image(ui, env, doc, if compact { vec2(92.0, 76.0) } else { CARD_IMAGE }, acts);
                ui.add_space(4.0);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 4.0;
                    ui.horizontal_wrapped(|ui| {
                        kind_badge(ui, pal, doc.kind());
                        if env.written.contains(&key) {
                            badge(ui, "Article ready", pal.link).on_hover_text("The full article has been written and saved.");
                        }
                        if let Doc::Feed(f) = doc
                            && f.summary_ai
                        {
                            badge(ui, "AI summary", pal.ai).on_hover_text("The headline and summary on this card were written by the AI.");
                        }
                        if let Some(p) = doc.paper() {
                            // Metadata sometimes carries impossible years (e.g. 2121): hide those.
                            let this_year = chrono::Datelike::year(&chrono::Utc::now());
                            let meta: Vec<String> = [p.venue.clone(), p.year.filter(|y| *y <= this_year).map(|y| y.to_string()), p.cited_by_count.filter(|c| *c > 0).map(|c| format!("{c} citations"))]
                                .into_iter()
                                .flatten()
                                .collect();
                            if !meta.is_empty() {
                                ui.label(egui::RichText::new(meta.join(" · ")).size(12.0).color(pal.text_faint));
                            }
                        }
                    });
                    let title_size = if ui.available_width() < 420.0 { 15.0 } else { 16.5 };
                    let title = ui.add(egui::Label::new(egui::RichText::new(doc.title()).font(theme::bold(title_size)).color(pal.text)).wrap().sense(Sense::click()));
                    if title.clicked() {
                        acts.push(Action::Open(doc.clone()));
                    }
                    title.on_hover_cursor(egui::CursorIcon::PointingHand);
                    let abs = doc.abstract_text();
                    if !abs.is_empty() {
                        let color = if matches!(doc, Doc::Feed(f) if f.summary_ai) { pal.ai } else { pal.text_weak };
                        let mut job = LayoutJob::single_section(abs.clone(), TextFormat { font_id: theme::regular(13.5), color, ..Default::default() });
                        job.wrap = TextWrapping { max_width: ui.available_width(), max_rows: if *expanded { usize::MAX } else { 2 }, break_anywhere: false, overflow_character: Some('…') };
                        let galley = ui.fonts_mut(|f| f.layout_job(job));
                        let truncated = galley.elided;
                        let r = ui.add(egui::Label::new(galley).sense(Sense::click()));
                        if r.clicked() {
                            acts.push(Action::Open(doc.clone()));
                        }
                        if truncated || *expanded {
                            let label = if *expanded { "Show less" } else { "Show more" };
                            if ui.add(egui::Label::new(egui::RichText::new(label).size(12.5).color(pal.accent)).sense(Sense::click())).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                                *expanded = !*expanded;
                            }
                        }
                    }
                    ui.horizontal(|ui| {
                        let authors = match doc {
                            Doc::Paper(p) => p.author_line(4),
                            Doc::Feed(f) => match (f.kind, &f.paper) {
                                (ContentKind::News, _) => {
                                    let outlets = outlet_names(&f.basis, 4);
                                    if outlets.is_empty() { format!("Gateless Scholar + {}", env.brand) } else { format!("Gateless Scholar + {} · from {}", env.brand, outlets.join(", ")) }
                                }
                                _ => f.author_line(env.brand),
                            },
                        };
                        ui.add(icons::icon(if matches!(doc.kind(), ContentKind::AiArticle | ContentKind::News) { "sparkle" } else { "user" }, 13.0, pal.text_faint));
                        ui.add(egui::Label::new(egui::RichText::new(authors).size(12.5).color(pal.text_faint)).truncate());
                    });
                    ui.horizontal(|ui| {
                        doc_action_buttons(ui, env, doc, acts);
                    });
                });
            });
        })
        .response;
    let resp = resp.interact(Sense::click());
    ui.ctx().data_mut(|d| d.insert_temp(id, resp.hovered()));
    if resp.clicked() {
        acts.push(Action::Open(doc.clone()));
    }
    resp.context_menu(|ui| doc_menu(ui, env, doc, acts));
    resp
}

/// Distinct outlet names of news articles, in order (syndicated copies count once).
pub fn outlet_names(basis: &[Paper], max: usize) -> Vec<String> {
    let mut seen = HashSet::new();
    basis
        .iter()
        .filter_map(|b| b.venue.clone().or(b.publisher.clone()))
        .map(|v| v.trim_start_matches("www.").to_string())
        .filter(|v| seen.insert(v.to_lowercase()))
        .take(max)
        .collect()
}

fn card_image(ui: &mut Ui, env: &CardEnv, doc: &Doc, size: Vec2, acts: &mut Vec<Action>) {
    let pal = env.pal;
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let radius = CornerRadius::same(9);
    match env.image_for(doc, acts) {
        Some(img) if img.embeddable => {
            let url = img.thumb_url.clone().unwrap_or_else(|| img.url.clone());
            match env.images.get(&url) {
                Some(ImgState::Ready(tex)) => cover_image(ui, tex, rect, radius),
                Some(ImgState::Failed) => placeholder(ui, pal, rect, doc.kind()),
                Some(ImgState::Loading) => {
                    ui.painter().rect_filled(rect, radius, pal.panel);
                }
                None => {
                    ui.painter().rect_filled(rect, radius, pal.panel);
                    acts.push(Action::LoadImage(url));
                }
            }
            let tip = format!("{}\n{} · {}{}\nClick to view full screen", img.title, img.credit, img.licence, if img.from_paper { "" } else { "\nMay not be from the paper" });
            if resp.on_hover_text(tip).clicked() {
                acts.push(Action::ShowImage(img.clone()));
            }
        }
        Some(img) => {
            // Copyrighted: link only, never embedded.
            placeholder(ui, pal, rect, doc.kind());
            ui.painter().text(rect.center_bottom() - vec2(0.0, 12.0), egui::Align2::CENTER_CENTER, "Image (link) ↗", theme::regular(11.0), pal.text_weak);
            if resp.on_hover_text(format!("{}\nCopyrighted image — opens at the source", img.title)).clicked() {
                acts.push(Action::OpenUrl(img.page_url.clone()));
            }
        }
        None => placeholder(ui, pal, rect, doc.kind()),
    }
}

/// Paint a texture cover-fitted (cropped to fill) into `rect`.
pub fn cover_image(ui: &Ui, tex: &egui::TextureHandle, rect: Rect, radius: CornerRadius) {
    let ts = tex.size_vec2();
    let scale = (rect.width() / ts.x).max(rect.height() / ts.y);
    let shown = vec2(rect.width() / (ts.x * scale), rect.height() / (ts.y * scale));
    let uv = Rect::from_center_size(pos2(0.5, 0.5), shown);
    egui::Image::new((tex.id(), rect.size())).uv(uv).corner_radius(radius).paint_at(ui, rect);
}

fn placeholder(ui: &Ui, pal: &Palette, rect: Rect, kind: ContentKind) {
    let c = pal.badge(kind);
    ui.painter().rect_filled(rect, 9.0, c.gamma_multiply(0.12));
    let icon = match kind {
        ContentKind::News => "newspaper",
        ContentKind::AiArticle => "sparkle",
        _ => "flask",
    };
    let s = (rect.height() * 0.32).clamp(20.0, 34.0);
    icons::icon(icon, s, c.gamma_multiply(0.7)).paint_at(ui, Rect::from_center_size(rect.center(), Vec2::splat(s)));
}

pub fn doc_action_buttons(ui: &mut Ui, env: &CardEnv, doc: &Doc, acts: &mut Vec<Action>) {
    let pal = env.pal;
    let key = doc.key();
    let fav = env.favourites.contains(&key);
    if icon_button_sized(ui, pal, if fav { "heart-fill" } else { "heart" }, if fav { "Remove from Favourites" } else { "Favourite" }, false, 28.0, fav.then_some(pal.danger)).clicked() {
        acts.push(Action::ToggleSave(SavedList::Favourites, doc.clone()));
    }
    let later = env.read_later.contains(&key);
    if icon_button_sized(ui, pal, if later { "clock-fill" } else { "clock" }, if later { "Remove from Read Later" } else { "Read later (queues the full article)" }, later, 28.0, None).clicked() {
        acts.push(Action::ReadLater(doc.clone()));
    }
    let add = icon_button_sized(ui, pal, "plus", "Add to bookshelf, project or note", false, 28.0, None);
    egui::Popup::menu(&add).show(|ui| add_menu(ui, env, doc, acts));
    let share = icon_button_sized(ui, pal, "share", "Share", false, 28.0, None);
    egui::Popup::menu(&share).show(|ui| share_menu(ui, doc, acts));
    if let Some(url) = doc.paper().and_then(|p| p.url.clone().or(p.doi.as_ref().map(|d| format!("https://doi.org/{d}")))) {
        let label = if doc.kind() == ContentKind::News { "Open the original story" } else { "Open the original paper" };
        if icon_button_sized(ui, pal, "external", label, false, 28.0, None).clicked() {
            acts.push(Action::OpenUrl(url));
        }
    }
}

fn add_menu(ui: &mut Ui, env: &CardEnv, doc: &Doc, acts: &mut Vec<Action>) {
    ui.set_min_width(200.0);
    ui.menu_button("Add to bookshelf", |ui| {
        for s in env.shelves {
            if ui.button(&s.name).clicked() {
                acts.push(Action::ToggleSave(SavedList::Bookshelf(s.id), doc.clone()));
            }
        }
        if !env.shelves.is_empty() {
            ui.separator();
        }
        if ui.button("New bookshelf…").clicked() {
            acts.push(Action::NewBookshelfWith(doc.clone()));
        }
    });
    ui.menu_button("Add to project", |ui| {
        for p in env.projects {
            if ui.button(&p.name).clicked() {
                acts.push(Action::ToggleSave(SavedList::Project(p.id), doc.clone()));
            }
        }
        if !env.projects.is_empty() {
            ui.separator();
        }
        if ui.button("New project…").clicked() {
            acts.push(Action::NewProjectWith(doc.clone()));
        }
    });
    ui.menu_button("Link in note", |ui| {
        for n in env.notes.iter().take(15) {
            if ui.button(crate::app::truncate(&n.title, 40)).clicked() {
                acts.push(Action::LinkInNote(doc.clone(), Some(n.id)));
            }
        }
        if !env.notes.is_empty() {
            ui.separator();
        }
        if ui.button("New note…").clicked() {
            acts.push(Action::LinkInNote(doc.clone(), None));
        }
    });
}

fn share_menu(ui: &mut Ui, doc: &Doc, acts: &mut Vec<Action>) {
    ui.set_min_width(200.0);
    if ui.button("Open to share the article…").on_hover_text("Open the article; export options are at its end").clicked() {
        acts.push(Action::Open(doc.clone()));
    }
    if let Some(p) = doc.paper() {
        if let Some(doi) = &p.doi
            && ui.button("Copy DOI link").clicked()
        {
            acts.push(Action::Copy(format!("https://doi.org/{doi}")));
        }
        if let Some(url) = &p.url
            && ui.button("Copy link to original").clicked()
        {
            acts.push(Action::Copy(url.clone()));
        }
    }
    if ui.button("Copy title").clicked() {
        acts.push(Action::Copy(doc.title().to_string()));
    }
}

/// Right-click menu for any document.
pub fn doc_menu(ui: &mut Ui, env: &CardEnv, doc: &Doc, acts: &mut Vec<Action>) {
    ui.set_min_width(210.0);
    let key = doc.key();
    if ui.button("Open").clicked() {
        acts.push(Action::Open(doc.clone()));
    }
    ui.separator();
    let fav = env.favourites.contains(&key);
    if ui.button(if fav { "Remove from Favourites" } else { "Favourite" }).clicked() {
        acts.push(Action::ToggleSave(SavedList::Favourites, doc.clone()));
    }
    let later = env.read_later.contains(&key);
    if ui.button(if later { "Remove from Read Later" } else { "Read later" }).clicked() {
        acts.push(Action::ReadLater(doc.clone()));
    }
    add_menu(ui, env, doc, acts);
    ui.separator();
    share_menu(ui, doc, acts);
    if let Some(url) = doc.paper().and_then(|p| p.url.clone())
        && ui.button("Open original in browser").clicked()
    {
        acts.push(Action::OpenUrl(url));
    }
    if let Some(oa) = doc.paper().and_then(|p| p.oa_url.clone())
        && ui.button("Open free full text in browser").clicked()
    {
        acts.push(Action::OpenUrl(oa));
    }
}

// ---------------------------------------------------------------------------------------------
// Origin-marked paragraphs
// ---------------------------------------------------------------------------------------------

pub struct ParagraphOut {
    pub response: Response,
    /// Index of the passage under the pointer.
    pub hovered: Option<usize>,
}

/// Draw one paragraph of passages with origin marking:
/// original = normal; reworded = a soft underline (hover shows the original wording);
/// AI-written = purple + a margin bar beside every line that holds AI text;
/// citations = green `[n]` links; unverified = an orange mark.
pub fn paragraph(ui: &mut Ui, pal: &Palette, passages: &[Passage], sources: &[Source], size: f32, acts: &mut Vec<Action>) -> ParagraphOut {
    let bar_space = 14.0;
    let font = theme::regular(size);
    let small = theme::regular(size * 0.78);
    let mut job = LayoutJob::default();
    let mut ranges: Vec<(usize, usize)> = vec![]; // char ranges per passage (text only)
    let mut cites: Vec<(usize, usize, u32)> = vec![];
    let mut unverified: Vec<(usize, usize, usize)> = vec![];
    let mut chars = 0usize;
    let line_height = Some(size * 1.55);
    // Translucent underline so reworded text doesn't distract from reading.
    let underline = Stroke::new(1.0, pal.reworded.gamma_multiply(0.4));
    for (i, p) in passages.iter().enumerate() {
        let text = p.text.trim();
        if text.is_empty() {
            ranges.push((chars, chars));
            continue;
        }
        let fmt = match p.origin {
            Origin::Original => TextFormat { font_id: font.clone(), color: pal.text, line_height, ..Default::default() },
            Origin::Reworded => TextFormat { font_id: font.clone(), color: pal.text, underline, line_height, ..Default::default() },
            Origin::AiWritten => TextFormat { font_id: font.clone(), color: pal.ai, line_height, ..Default::default() },
        };
        let start = chars;
        job.append(text, 0.0, fmt);
        chars += text.chars().count();
        ranges.push((start, chars));
        for c in &p.citations {
            let t = format!("[{c}]");
            let s = chars;
            job.append(&t, 2.0, TextFormat { font_id: small.clone(), color: pal.link, valign: Align::TOP, line_height, ..Default::default() });
            chars += t.chars().count();
            cites.push((s, chars, *c));
        }
        if p.unverified {
            let t = " unverified";
            let s = chars;
            job.append(t, 0.0, TextFormat { font_id: small.clone(), color: pal.warn, italics: true, valign: Align::TOP, line_height, ..Default::default() });
            chars += t.chars().count();
            unverified.push((s, chars, i));
        }
        // A passage ending in a blank line closes a paragraph (Ask answers use this).
        let sep = if p.text.ends_with("\n\n") && i + 1 < passages.len() { "\n\n" } else { " " };
        job.append(sep, 0.0, TextFormat { font_id: font.clone(), line_height, ..Default::default() });
        chars += sep.chars().count();
    }
    job.wrap.max_width = (ui.available_width() - bar_space).max(80.0);
    let galley = ui.fonts_mut(|f| f.layout_job(job));
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), galley.size().y), Sense::click());
    let text_pos = rect.min + vec2(bar_space, 0.0);

    // Margin bar beside lines containing AI-written text.
    let ai_ranges: Vec<(usize, usize)> = passages.iter().zip(&ranges).filter(|(p, _)| p.origin == Origin::AiWritten).map(|(_, r)| *r).collect();
    let mut row_start = 0usize;
    for row in &galley.rows {
        let n = row.char_count_including_newline().0;
        let (a, b) = (row_start, row_start + n);
        if ai_ranges.iter().any(|(s, e)| *s < b && *e > a) {
            let r = row.rect().translate(text_pos.to_vec2());
            ui.painter().rect_filled(Rect::from_min_max(pos2(rect.left() + 2.0, r.top() + 2.0), pos2(rect.left() + 5.0, r.bottom() - 1.0)), 1.0, pal.ai);
        }
        row_start = b;
    }

    // Hover: which passage / citation is under the pointer.
    let mut hovered = None;
    let mut hovered_cite = None;
    if let Some(pos) = resp.hover_pos() {
        let local = pos - text_pos;
        if galley.rect.expand(2.0).contains(pos2(local.x, local.y)) {
            let idx = galley.cursor_from_pos(local).index.0;
            hovered_cite = cites.iter().find(|(s, e, _)| idx >= *s && idx <= *e).map(|c| c.2);
            if hovered_cite.is_none() {
                hovered = ranges.iter().position(|(s, e)| idx >= *s && idx < *e);
                if hovered.is_none() {
                    hovered = unverified.iter().find(|(s, e, _)| idx >= *s && idx <= *e).map(|u| u.2);
                }
            }
        }
    }
    // Light highlight of the hovered passage.
    if let Some(i) = hovered
        && let Some((s, e)) = ranges.get(i).copied()
        && passages[i].origin != Origin::Original
    {
        let mut row_start = 0usize;
        for row in &galley.rows {
            let n = row.char_count_including_newline().0;
            let (a, b) = (row_start, row_start + n);
            if s < b && e > a {
                let x0 = if s > a { galley.pos_from_cursor(egui::text::CCursor::new(s)).left() } else { row.rect().left() };
                let x1 = if e < b { galley.pos_from_cursor(egui::text::CCursor::new(e)).left() } else { row.rect().right() };
                let r = Rect::from_min_max(pos2(x0, row.rect().top()), pos2(x1, row.rect().bottom())).translate(text_pos.to_vec2());
                let c = if passages[i].origin == Origin::AiWritten { pal.ai_soft } else { pal.reworded.gamma_multiply(0.12) };
                ui.painter().rect_filled(r, 2.0, c);
            }
            row_start = b;
        }
    }
    ui.painter().galley(text_pos, galley, pal.text);

    let mut resp = resp;
    if let Some(c) = hovered_cite {
        resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
        if let Some(src) = sources.iter().find(|s| s.id == c) {
            resp = resp.on_hover_ui(|ui| {
                ui.set_max_width(380.0);
                ui.label(egui::RichText::new(format!("[{}] {}", src.id, src.title)).strong());
                let meta: Vec<String> = [Some(short_authors(&src.authors)), src.venue.clone(), src.year.map(|y| y.to_string())].into_iter().flatten().filter(|s| !s.is_empty()).collect();
                ui.label(egui::RichText::new(meta.join(" · ")).color(pal.text_weak));
                ui.label(egui::RichText::new(if src.paper_key.is_some() { "Click to open in Gateless Scholar · right-click for the original" } else { "Click to open the source" }).size(12.0).color(pal.link));
            });
            if resp.clicked() {
                match &src.paper_key {
                    Some(k) => acts.push(Action::OpenKey(k.clone())),
                    None => {
                        if let Some(u) = src.link() {
                            acts.push(Action::OpenUrl(u));
                        }
                    }
                }
            }
            if resp.secondary_clicked()
                && let Some(u) = src.link()
            {
                acts.push(Action::OpenUrl(u));
            }
        }
    } else if let Some(i) = hovered {
        let p = &passages[i];
        resp = match p.origin {
            Origin::Reworded => resp.on_hover_ui(|ui| {
                ui.set_max_width(420.0);
                ui.label(egui::RichText::new("AI-reworded original — the original wording:").size(12.0).color(pal.reworded));
                ui.label(p.original.as_deref().unwrap_or("(original not stored)"));
            }),
            Origin::AiWritten if p.unverified => resp.on_hover_text("AI-written. Unverified: no retrieved source supports this sentence."),
            Origin::AiWritten => resp.on_hover_text(if p.citations.is_empty() {
                "AI-written".to_string()
            } else {
                format!("AI-written from source{} {}", if p.citations.len() > 1 { "s" } else { "" }, p.citations.iter().map(|c| format!("[{c}]")).collect::<Vec<_>>().join(" "))
            }),
            Origin::Original => resp,
        };
    }
    ParagraphOut { response: resp, hovered }
}

pub fn short_authors(a: &[String]) -> String {
    match a.len() {
        0 => String::new(),
        1 => a[0].clone(),
        2 => format!("{} and {}", a[0], a[1]),
        _ => format!("{} et al.", a[0]),
    }
}

pub fn nav_link(ui: &mut Ui, pal: &Palette, text: &str, view: View, acts: &mut Vec<Action>) {
    if ui.add(egui::Label::new(egui::RichText::new(text).color(pal.accent)).sense(Sense::click())).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
        acts.push(Action::Navigate(view));
    }
}
