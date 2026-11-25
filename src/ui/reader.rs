//! The article reader: the abstract immediately, a progress indicator with an estimated time
//! while sources are found and the article is written, sections streamed in as they finish,
//! origin marking throughout, references, share/export, and the Ask panel.

use egui::{Align, Layout, Sense, Stroke, Ui, vec2};

use super::charts;
use super::icons;
use super::theme::{self, Palette};
use super::widgets::{self, icon_button};
use crate::app::{Action, App, ExportKind, GenState, View};
use crate::model::*;

const READ_WIDTH: f32 = 760.0;

impl App {
    pub fn reader_view(&mut self, ui: &mut Ui, key: &str) {
        let pal = self.pal;
        let Some(reader) = self.readers.get(key) else {
            match self.db.find_doc(key) {
                Some(doc) => self.actions.push(Action::Open(doc)),
                None => {
                    widgets::empty_state(ui, &pal, "file", "Not found", "This item is no longer stored.");
                }
            }
            return;
        };
        let doc = reader.doc.clone();
        let article = reader.article.clone();
        let ask_open = reader.ask_open;
        let learn_mode = reader.learn_mode;
        let mobile = self.mobile;
        let data = self.card_data();
        let env = self.card_env(&data);
        let gen_state = self.gens.get(key);
        let style = self.settings.citation_style;
        let notes = data.notes.clone();
        let mut acts = vec![];
        let mut toggle_ask = false;
        let mut set_learn: Option<bool> = None;
        let learn_article_key = crate::app::learn_key(key, false);
        let learn_abs_key = crate::app::learn_key(key, true);
        let learn_article = self.learn.get(&learn_article_key).cloned();
        let learn_article_gen = self.gens.get(&learn_article_key);
        let learn_abs = self.learn.get(&learn_abs_key).cloned();
        let learn_abs_gen = self.gens.get(&learn_abs_key);
        let hero = article.as_ref().and_then(|a| a.hero_image.clone()).or_else(|| env.image_for(&doc, &mut acts)).filter(|i| i.embeddable);

        // Toolbar: back, a small type dot (details on hover), Learn AI, then the actions.
        egui::Frame::new().inner_margin(egui::Margin::symmetric(12, 6)).show(ui, |ui| {
            ui.horizontal(|ui| {
                if icon_button(ui, &pal, "chevron-left", "Back (Alt+←)", false).clicked() {
                    acts.push(Action::Back);
                }
                widgets::icon_dot(ui, widgets::kind_icon(doc.kind()), pal.badge(doc.kind()), widgets::kind_tip(doc.kind()));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let more = icon_button(ui, &pal, "more", "More", false);
                    egui::Popup::menu(&more).show(|ui| {
                        ui.set_min_width(220.0);
                        widgets::doc_menu(ui, &env, &doc, &mut acts);
                        if article.is_some() {
                            ui.separator();
                            if ui.button("Write the article again…").clicked() {
                                acts.push(Action::ConfirmRegenerate(key.to_string()));
                            }
                        }
                    });
                    if article.is_some() {
                        let share = icon_button(ui, &pal, "download", "Share / export the article (email, Word, PDF, text, print)", false);
                        egui::Popup::menu(&share).show(|ui| share_menu(ui, key, &mut acts));
                    }
                    if icon_button(ui, &pal, "ask", "Ask a follow-up question (Ctrl+J)", ask_open).clicked() {
                        toggle_ask = true;
                    }
                    // Phones: favourite / read later / add / share live in the "more" menu.
                    if !mobile {
                        widgets::doc_action_buttons(ui, &env, &doc, &mut acts);
                    }
                    ui.add_space(6.0);
                    let learn_btn = ui
                        .add(
                            egui::Button::image_and_text(icons::icon("sparkle", 15.0, if learn_mode { egui::Color32::WHITE } else { pal.ai }), egui::RichText::new("Learn AI").color(if learn_mode { egui::Color32::WHITE } else { pal.ai }))
                                .fill(if learn_mode { pal.ai } else { pal.ai_soft })
                                .corner_radius(14),
                        )
                        .on_hover_text("Rewrite this in plain words for a non-specialist: every technical term explained, same sources. Click again to go back.");
                    if learn_btn.clicked() {
                        if learn_mode {
                            set_learn = Some(false);
                        } else if article.is_some() {
                            acts.push(Action::Learn { doc_key: key.to_string(), abstract_only: false });
                        } else {
                            // No article yet: explain the abstract meanwhile.
                            acts.push(Action::Learn { doc_key: key.to_string(), abstract_only: true });
                        }
                    }
                });
            });
        });
        ui.painter().line_segment([ui.min_rect().left_bottom(), ui.min_rect().right_bottom()], Stroke::new(1.0, pal.border));

        egui::ScrollArea::vertical().auto_shrink([false, false]).id_salt(("reader", key)).show(ui, |ui| {
            ui.add_space(18.0);
            widgets::column(ui, READ_WIDTH, |ui| {
                if let Some(img) = &hero {
                    hero_image(ui, &pal, &self.images, img, &mut acts);
                }
                article_header(ui, &pal, &doc, article.as_ref(), gen_state, &mut acts);
                ui.add_space(10.0);
                original_box(ui, &pal, &doc, article.as_ref(), &mut acts);
                ui.add_space(14.0);
                abstract_block(ui, &pal, &doc, learn_abs.as_ref(), learn_abs_gen, key, &mut acts);
                ui.add_space(16.0);

                if learn_mode {
                    learn_view(ui, &pal, &self.images, learn_article.as_ref(), learn_article_gen, &notes, &doc, style, &mut acts, &mut set_learn);
                } else {
                    match (&article, gen_state) {
                        (Some(a), _) => {
                            for s in &a.sections {
                                section(ui, &pal, &self.images, s, &a.sources, &notes, &doc, &mut acts);
                            }
                            references(ui, &pal, &a.sources, style, &mut acts);
                            validation(ui, &pal, &a.validation);
                            ui.add_space(12.0);
                            footer(ui, &pal, a, key, &mut acts);
                        }
                        (None, Some(g)) => {
                            progress(ui, &pal, g, &doc, &mut acts);
                            streaming(ui, &pal, &self.images, g, &notes, &doc, style, &mut acts);
                        }
                        (None, None) => {
                            ui.vertical_centered(|ui| {
                                if ui.button("Write the full article").clicked() {
                                    acts.push(Action::Generate(doc.clone()));
                                }
                            });
                        }
                    }
                }
                ui.add_space(40.0);
            });
        });
        if let Some(r) = self.readers.get_mut(key) {
            if toggle_ask {
                r.ask_open = !r.ask_open;
            }
            if let Some(l) = set_learn {
                r.learn_mode = l;
            }
        }
        self.actions.extend(acts);
    }

    pub fn ask_panel(&mut self, ui: &mut Ui, key: &str) {
        let pal = self.pal;
        // Phones: the Ask panel takes the whole screen (Back closes it).
        let width = if self.mobile { ui.ctx().content_rect().width() } else { self.settings.ask_panel_width.clamp(280.0, 600.0) };
        let mut acts = vec![];
        let panel = egui::Panel::right("ask_panel")
            .exact_size(width)
            .frame(egui::Frame::new().fill(pal.sidebar).inner_margin(12))
            .show(ui, |ui| {
                let Some(r) = self.readers.get_mut(key) else { return };
                ui.horizontal(|ui| {
                    ui.add(icons::icon("ask", 18.0, pal.accent));
                    ui.label(egui::RichText::new("Ask").font(theme::bold(16.0)));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if icon_button(ui, &pal, "x", "Close (Ctrl+J)", false).clicked() {
                            r.ask_open = false;
                        }
                    });
                });
                ui.label(egui::RichText::new("Follow-up questions about this article, answered from referenced sources.").size(12.0).color(pal.text_weak));
                ui.add_space(6.0);
                let input_h = 92.0;
                let list_h = (ui.available_height() - input_h).max(80.0);
                egui::ScrollArea::vertical().max_height(list_h).auto_shrink([false, false]).stick_to_bottom(true).show(ui, |ui| {
                    if r.asks.is_empty() && r.asking.is_none() {
                        ui.add_space(10.0);
                        for q in suggested_questions(&r.doc) {
                            if ui.add(egui::Button::new(egui::RichText::new(&q).size(13.0).color(pal.text_weak)).wrap()).clicked() {
                                acts.push(Action::Ask(key.to_string(), q));
                            }
                        }
                    }
                    for a in &r.asks {
                        ask_entry(ui, &pal, &a.question, &a.answer, &mut acts);
                    }
                    if let Some((q, partial)) = &r.asking {
                        question_bubble(ui, &pal, q);
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label(egui::RichText::new(if partial.is_empty() { "Finding sources and answering…" } else { "Answering…" }).size(12.0).color(pal.text_faint));
                        });
                        if !partial.is_empty() {
                            ui.label(egui::RichText::new(partial).size(13.0).color(pal.ai.gamma_multiply(0.7)));
                        }
                    }
                    if let Some(e) = &r.ask_error {
                        ui.colored_label(pal.danger, e);
                    }
                });
                ui.add_space(6.0);
                let ready = r.article.is_some() && r.asking.is_none();
                egui::Frame::new().fill(pal.card).stroke(Stroke::new(1.0, pal.border)).corner_radius(10).inner_margin(8).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    let hint = if r.article.is_some() { "Ask a follow-up…" } else { "Available once the article is written" };
                    let te = ui.add_enabled(ready, egui::TextEdit::multiline(&mut r.ask_input).hint_text(hint).desired_rows(2).desired_width(ui.available_width() - 36.0).frame(egui::Frame::NONE));
                    let send_key = te.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter) && !i.modifiers.shift);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let send = icon_button(ui, &pal, "send", "Ask (Enter)", false);
                        if (send.clicked() || send_key) && ready {
                            let q = r.ask_input.trim().to_string();
                            if !q.is_empty() {
                                acts.push(Action::Ask(key.to_string(), q));
                            }
                        }
                    });
                });
            });
        // Resize the ask panel with a craft-ui handle on its left edge.
        let edge = panel.response.rect;
        let divider = egui::Rect::from_min_max(egui::pos2(edge.left() - 2.0, edge.top()), egui::pos2(edge.left() + 2.0, edge.bottom()));
        let style = craft_ui::layout::ResizeHandleStyle { hovered: pal.accent.gamma_multiply(0.5), dragged: pal.accent, focused: pal.accent };
        let rr = craft_ui::layout::ResizeHandle::new(egui::Id::unique("ask_resize"), craft_ui::layout::SplitAxis::Horizontal, divider, "Resize ask panel", style).show(ui);
        if rr.delta != 0.0 {
            self.settings.ask_panel_width = (self.settings.ask_panel_width - rr.delta).clamp(280.0, 600.0);
        }
        if rr.response.drag_stopped() {
            acts.push(Action::SaveSettings);
        }
        self.actions.extend(acts);
    }
}

fn suggested_questions(doc: &Doc) -> Vec<String> {
    match doc.kind() {
        ContentKind::News => vec!["What's the background to this story?".into(), "How do different outlets report it?".into(), "What happens next?".into()],
        _ => vec![
            "Explain this in simple terms.".into(),
            "What are the main limitations?".into(),
            "How does this compare with earlier work?".into(),
            "What research has built on this since?".into(),
        ],
    }
}

fn question_bubble(ui: &mut Ui, pal: &Palette, q: &str) {
    egui::Frame::new().fill(pal.accent_soft).corner_radius(10).inner_margin(egui::Margin::symmetric(10, 6)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(egui::RichText::new(q).strong());
    });
    ui.add_space(4.0);
}

fn ask_entry(ui: &mut Ui, pal: &Palette, q: &str, a: &Answer, acts: &mut Vec<Action>) {
    question_bubble(ui, pal, q);
    widgets::paragraph(ui, pal, &a.passages, &a.sources, 13.5, acts);
    let cited: Vec<&Source> = a.sources.iter().filter(|s| a.passages.iter().any(|p| p.citations.contains(&s.id))).collect();
    if !cited.is_empty() {
        ui.add_space(2.0);
        for s in cited {
            ui.horizontal_wrapped(|ui| {
                ui.label(egui::RichText::new(format!("[{}]", s.id)).size(11.5).color(pal.link));
                let label = ui.add(egui::Label::new(egui::RichText::new(crate::app::truncate(&s.title, 70)).size(12.0).color(pal.link)).sense(Sense::click())).on_hover_cursor(egui::CursorIcon::PointingHand);
                if label.clicked() {
                    match &s.paper_key {
                        Some(k) => acts.push(Action::OpenKey(k.clone())),
                        None => {
                            if let Some(u) = s.link() {
                                acts.push(Action::OpenUrl(u));
                            }
                        }
                    }
                }
                if s.paper_key.is_some() {
                    label.on_hover_text("Open this article in Gateless Scholar");
                }
            });
        }
    }
    if !a.generated_by.is_empty() {
        ui.label(egui::RichText::new(format!("AI answer · {}", a.generated_by)).size(11.0).color(pal.text_faint));
    }
    ui.add_space(12.0);
}

fn article_header(ui: &mut Ui, pal: &Palette, doc: &Doc, article: Option<&Article>, gen_state: Option<&GenState>, _acts: &mut [Action]) {
    // What this text is: always visible, but compact.
    let kind_label = match article {
        Some(a) => a.kind.label(),
        None => match doc.kind() {
            ContentKind::AbstractOnly => ArticleKind::Interpretation.label(),
            ContentKind::FreeFullText => "Free full text – the original, lightly reworded for flow where it can be read",
            ContentKind::AiArticle => ArticleKind::AiOnly.label(),
            ContentKind::News => ArticleKind::News.label(),
        },
    };
    let is_ai = article.map(|a| a.kind != ArticleKind::FullText).unwrap_or(!matches!(doc.kind(), ContentKind::FreeFullText));
    let color = if is_ai { pal.ai } else { pal.badge_free };
    // Wrapping row: on a phone the label and the key chip each take a line, and the label wraps
    // (a non-wrapping row would widen the whole column off-screen).
    ui.horizontal_wrapped(|ui| {
        let max = ui.available_width();
        egui::Frame::new().fill(color.gamma_multiply(0.12)).corner_radius(12).inner_margin(egui::Margin::symmetric(10, 4)).show(ui, |ui| {
            ui.set_max_width(max - 20.0);
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 5.0;
                ui.add(icons::icon(if is_ai { "sparkle" } else { "book-open" }, 13.0, color));
                ui.add(egui::Label::new(egui::RichText::new(kind_label).size(12.5).color(color)).wrap());
            });
        });
        // The marking key lives behind a small chip instead of a full-width box.
        let key = ui.add(egui::Button::new(egui::RichText::new("How text is marked").size(12.0).color(pal.text_faint)).frame(false));
        key.on_hover_ui(|ui| {
            ui.set_max_width(420.0);
            widgets::legend(ui, pal);
        });
    });
    ui.add_space(8.0);
    // Title: our own title for AI text; never the original title presented as the authors' work.
    let title = match article {
        Some(a) => a.title.clone(),
        None => match doc {
            Doc::Feed(f) if matches!(f.kind, ContentKind::AiArticle | ContentKind::News) => f.headline.clone(),
            _ => match doc.kind() {
                ContentKind::AbstractOnly => format!("AI interpretation: {}", doc.title()),
                _ => doc.title().to_string(),
            },
        },
    };
    let size = if ui.available_width() < 500.0 { 23.0 } else { 29.0 };
    ui.label(egui::RichText::new(title).font(theme::bold(size)).color(pal.text));
    if let Some(s) = article.and_then(|a| a.standfirst.clone()) {
        ui.add_space(4.0);
        // The standfirst is AI-written: AI colour plus a margin bar.
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(vec2(3.0, 22.0), Sense::hover());
            ui.painter().rect_filled(r, 1.0, pal.ai);
            ui.add(egui::Label::new(egui::RichText::new(s).size(17.0).color(pal.ai)).wrap());
        });
    }
    ui.add_space(6.0);
    ui.horizontal_wrapped(|ui| {
        match article {
            Some(a) if a.kind == ArticleKind::FullText => {
                ui.label(egui::RichText::new(a.original_authors.join(", ")).color(pal.text_weak));
                ui.label(egui::RichText::new("· original authors · light rewording by").color(pal.text_faint));
                ui.label(egui::RichText::new(&a.byline).color(pal.ai));
            }
            Some(a) => {
                ui.add(icons::icon("sparkle", 14.0, pal.ai));
                ui.label(egui::RichText::new(&a.byline).color(pal.ai));
            }
            None => {
                if gen_state.is_some() {
                    ui.label(egui::RichText::new("Being written…").color(pal.text_faint));
                }
            }
        }
        if let Some(a) = article {
            ui.label(egui::RichText::new(format!("· {} · {}", a.created.format("%e %b %Y"), a.generated_by)).size(12.5).color(pal.text_faint));
        }
    });
}

/// A wide photo above the title (cover-fitted); click for full screen with source and licence.
fn hero_image(ui: &mut Ui, pal: &Palette, images: &std::collections::HashMap<String, crate::app::ImgState>, img: &ImageRef, acts: &mut Vec<Action>) {
    let url = img.url.clone();
    let h = if ui.available_width() < 500.0 { 170.0 } else { 230.0 };
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::click());
    match images.get(&url) {
        Some(crate::app::ImgState::Ready(tex)) => widgets::cover_image(ui, tex, rect, egui::CornerRadius::same(12)),
        Some(crate::app::ImgState::Failed) => {
            ui.painter().rect_filled(rect, 12.0, pal.card);
        }
        Some(crate::app::ImgState::Loading) => {
            ui.painter().rect_filled(rect, 12.0, pal.card);
        }
        None => {
            ui.painter().rect_filled(rect, 12.0, pal.card);
            acts.push(Action::LoadImage(url));
        }
    }
    let tip = format!("{}\n{} · {}{}\nClick to view full screen with its source", img.title, img.credit, img.licence, if img.from_paper { "" } else { "\nMay not be from the paper" });
    if resp.on_hover_text(tip).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
        acts.push(Action::ShowImage(img.clone()));
    }
    ui.add_space(14.0);
}

/// The real paper, always linked: title, authors, venue; links as small icon dots (details on hover).
fn original_box(ui: &mut Ui, pal: &Palette, doc: &Doc, article: Option<&Article>, acts: &mut Vec<Action>) {
    match doc {
        Doc::Feed(f) if matches!(f.kind, ContentKind::AiArticle | ContentKind::News) => {
            if f.basis.is_empty() {
                return;
            }
            let news = f.kind == ContentKind::News;
            ui.horizontal_wrapped(|ui| {
                ui.label(egui::RichText::new(if news { "From" } else { "Written from" }).size(12.5).color(pal.text_faint));
                if news {
                    // One chip per outlet (syndicated copies of the same story count once).
                    let mut seen = std::collections::HashSet::new();
                    for p in &f.basis {
                        let outlet = p.venue.clone().or(p.publisher.clone()).unwrap_or_else(|| "source".into()).trim_start_matches("www.").to_string();
                        if !seen.insert(outlet.to_lowercase()) {
                            continue;
                        }
                        let r = ui.add(egui::Button::new(egui::RichText::new(&outlet).size(12.5).color(pal.link)).corner_radius(10)).on_hover_text(format!("{}\nOpen the original story", p.title));
                        if r.clicked()
                            && let Some(u) = &p.url
                        {
                            acts.push(Action::OpenUrl(u.clone()));
                        }
                    }
                } else {
                    for (i, p) in f.basis.iter().take(12).enumerate() {
                        let short: String = crate::app::truncate(&p.title, 48);
                        let r = ui.add(egui::Button::new(egui::RichText::new(format!("{} {short}", i + 1)).size(12.5).color(pal.link)).corner_radius(10)).on_hover_text(format!("{}\n{}", p.title, p.author_line(3)));
                        if r.clicked() {
                            acts.push(Action::Open(Doc::Paper(p.clone())));
                        }
                    }
                }
            });
        }
        _ => {
            let Some(p) = doc.paper() else { return };
            egui::Frame::new().fill(pal.card).stroke(Stroke::new(1.0, pal.border)).corner_radius(10).inner_margin(12).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("THE ORIGINAL PAPER").size(11.5).strong().color(pal.text_faint));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if let Some(doi) = &p.doi
                            && widgets::icon_dot(ui, "copy", pal.text_weak, &format!("Copy DOI link\nhttps://doi.org/{doi}")).clicked()
                        {
                            acts.push(Action::Copy(format!("https://doi.org/{doi}")));
                        }
                        if let Some(oa) = &p.oa_url {
                            let version = match p.oa_version.as_deref() {
                                Some("submittedVersion") => " (preprint)",
                                Some("acceptedVersion") => " (author version)",
                                Some("publishedVersion") => " (published version)",
                                _ => "",
                            };
                            let tip = format!("Free full text{version} — a legal free copy{}\nClick to open it", p.oa_licence.as_ref().map(|l| format!(", licence {l}")).unwrap_or_default());
                            if widgets::icon_dot(ui, "book-open", pal.badge_free, &tip).clicked() {
                                acts.push(Action::OpenUrl(oa.clone()));
                            }
                        }
                        let link = p.doi.as_ref().map(|d| format!("https://doi.org/{d}")).or(p.url.clone()).or(article.and_then(|a| a.original_url.clone()));
                        if let Some(u) = link
                            && widgets::icon_dot(ui, "external", pal.link, "Read the original paper at the publisher").clicked()
                        {
                            acts.push(Action::OpenUrl(u));
                        }
                    });
                });
                ui.label(egui::RichText::new(&p.title).font(theme::bold(15.5)).color(pal.text));
                let meta: Vec<String> = [Some(p.author_line(4)), p.venue.clone(), p.date.map(|d| d.format("%Y").to_string()).or(p.year.map(|y| y.to_string())), p.cited_by_count.filter(|c| *c > 0).map(|c| format!("cited by {c}"))]
                    .into_iter()
                    .flatten()
                    .filter(|s| !s.is_empty())
                    .collect();
                ui.label(egui::RichText::new(meta.join(" · ")).size(12.5).color(pal.text_weak));
            });
        }
    }
}

fn abstract_block(ui: &mut Ui, pal: &Palette, doc: &Doc, learn: Option<&Article>, learn_gen: Option<&GenState>, key: &str, acts: &mut Vec<Action>) {
    let text = doc.abstract_text();
    if text.trim().is_empty() {
        return;
    }
    let (label, passage) = match doc {
        Doc::Feed(f) if f.summary_ai => ("SUMMARY (AI-WRITTEN)", Passage::ai(text, vec![])),
        Doc::Feed(f) if f.kind == ContentKind::News => ("SUMMARY (FROM THE OUTLETS)", Passage::original(text)),
        _ => ("ABSTRACT (ORIGINAL TEXT FROM THE PAPER)", Passage::original(text)),
    };
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(label).size(11.5).strong().color(pal.text_faint));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if learn.is_none()
                && learn_gen.is_none_or(|g| g.error.is_some())
                && ui
                    .add(egui::Button::image_and_text(icons::icon("sparkle", 13.0, pal.ai), egui::RichText::new("Learn AI").size(12.5).color(pal.ai)).fill(pal.ai_soft).corner_radius(12))
                    .on_hover_text("Explain this in plain words, with the technical terms explained")
                    .clicked()
            {
                acts.push(Action::Learn { doc_key: key.to_string(), abstract_only: true });
            }
        });
    });
    ui.add_space(2.0);
    widgets::paragraph(ui, pal, &[passage], &[], 15.0, acts);
    // The plain-language explanation, right under the abstract.
    if learn.is_some() || learn_gen.is_some() {
        ui.add_space(8.0);
        egui::Frame::new().fill(pal.ai_soft).corner_radius(10).inner_margin(12).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.add(icons::icon("sparkle", 15.0, pal.ai));
                ui.label(egui::RichText::new("Learn AI · in plain words").strong().color(pal.ai));
            });
            match (learn, learn_gen) {
                (Some(a), _) => {
                    for s in &a.sections {
                        if s.heading == "Key terms" {
                            ui.add_space(4.0);
                            ui.label(egui::RichText::new("Key terms").strong().color(pal.text));
                        }
                        for b in &s.blocks {
                            if let Block::Paragraph(ps) = b {
                                widgets::paragraph(ui, pal, ps, &a.sources, 14.5, acts);
                                ui.add_space(4.0);
                            }
                        }
                    }
                }
                (None, Some(g)) => {
                    if let Some(e) = &g.error {
                        ui.colored_label(pal.danger, e);
                    } else {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label(egui::RichText::new(&g.text).size(12.5).color(pal.text_weak));
                        });
                        for s in &g.sections {
                            for b in &s.blocks {
                                if let Block::Paragraph(ps) = b {
                                    widgets::paragraph(ui, pal, ps, &g.sources, 14.5, acts);
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
        });
    }
}

/// The Learn AI version of the whole article.
#[allow(clippy::too_many_arguments)]
fn learn_view(
    ui: &mut Ui,
    pal: &Palette,
    images: &std::collections::HashMap<String, crate::app::ImgState>,
    learn: Option<&Article>,
    learn_gen: Option<&GenState>,
    notes: &[Note],
    doc: &Doc,
    style: CitationStyle,
    acts: &mut Vec<Action>,
    set_learn: &mut Option<bool>,
) {
    egui::Frame::new().fill(pal.ai_soft).corner_radius(10).inner_margin(egui::Margin::symmetric(12, 8)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.add(icons::icon("sparkle", 16.0, pal.ai));
            ui.label(egui::RichText::new("Learn AI: this article rewritten in plain words, with technical terms explained. Same sources.").color(pal.ai));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button("Back to the full article").clicked() {
                    *set_learn = Some(false);
                }
            });
        });
    });
    ui.add_space(12.0);
    match (learn, learn_gen) {
        (Some(a), _) => {
            for s in &a.sections {
                section(ui, pal, images, s, &a.sources, notes, doc, acts);
            }
            references(ui, pal, &a.sources, style, acts);
        }
        (None, Some(g)) => {
            progress(ui, pal, g, doc, acts);
            streaming(ui, pal, images, g, notes, doc, style, acts);
        }
        (None, None) => {}
    }
}

/// Sections streamed so far, plus a live preview of the one being written.
#[allow(clippy::too_many_arguments)]
fn streaming(ui: &mut Ui, pal: &Palette, images: &std::collections::HashMap<String, crate::app::ImgState>, g: &GenState, notes: &[Note], doc: &Doc, style: CitationStyle, acts: &mut Vec<Action>) {
    for s in &g.sections {
        section(ui, pal, images, s, &g.sources, notes, doc, acts);
    }
    if !g.delta.is_empty() && g.error.is_none() {
        ui.add_space(6.0);
        egui::Frame::new().stroke(Stroke::new(1.0, pal.border)).corner_radius(8).inner_margin(10).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                ui.spinner();
                ui.add(egui::Label::new(egui::RichText::new("Writing the next section… (preview; citations are checked before it's shown)").size(12.0).color(pal.text_faint)).wrap());
            });
            let tail: String = g.delta.chars().rev().take(700).collect::<Vec<_>>().into_iter().rev().collect();
            ui.label(egui::RichText::new(tail).size(13.0).color(pal.ai.gamma_multiply(0.6)));
        });
    }
    if !g.sources.is_empty() {
        ui.add_space(10.0);
        egui::CollapsingHeader::new(egui::RichText::new(format!("Sources found ({})", g.sources.len())).color(pal.text_weak)).id_salt("found_sources").show(ui, |ui| {
            references(ui, pal, &g.sources, style, acts);
        });
    }
}

fn progress(ui: &mut Ui, pal: &Palette, g: &GenState, doc: &Doc, acts: &mut Vec<Action>) {
    egui::Frame::new().fill(pal.card).stroke(Stroke::new(1.0, pal.border)).corner_radius(10).inner_margin(14).show(ui, |ui| {
        ui.set_width(ui.available_width());
        if let Some(err) = &g.error {
            ui.horizontal(|ui| {
                ui.add(icons::icon("alert", 18.0, pal.warn));
                ui.label(egui::RichText::new("The article couldn't be written").strong().color(pal.warn));
            });
            ui.label(egui::RichText::new(err).color(pal.text_weak));
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.button("Try again").clicked() {
                    acts.push(Action::Generate(doc.clone()));
                }
                if ui.button("Model settings").clicked() {
                    acts.push(Action::Navigate(View::Settings));
                }
                if let Some(u) = doc.paper().and_then(|p| p.oa_url.clone().or(p.url.clone())) {
                    if ui.button("Read the original instead ↗").clicked() {
                        acts.push(Action::OpenUrl(u));
                    }
                }
            });
            return;
        }
        // Wrapping rows throughout: a long status must never widen the reading column.
        ui.horizontal_wrapped(|ui| {
            ui.spinner();
            ui.add(egui::Label::new(egui::RichText::new(&g.text).strong()).wrap());
            ui.label(egui::RichText::new(format!("· {}", eta(g))).size(12.5).color(pal.text_faint));
        });
        ui.add_space(4.0);
        ui.add(egui::ProgressBar::new(g.fraction).desired_height(6.0).fill(pal.accent));
        ui.add_space(4.0);
        let steps = ["Finding sources", "Reading papers", "Writing", "Checking citations"];
        let active = if g.fraction < 0.15 { 0 } else if g.fraction < 0.3 { 1 } else if g.fraction < 0.95 { 2 } else { 3 };
        ui.horizontal_wrapped(|ui| {
            for (i, s) in steps.iter().enumerate() {
                let c = if i < active { pal.link } else if i == active { pal.text } else { pal.text_faint };
                ui.label(egui::RichText::new(if i < active { format!("✓ {s}") } else { s.to_string() }).size(12.0).color(c));
                if i + 1 < steps.len() {
                    ui.label(egui::RichText::new("›").color(pal.text_faint));
                }
            }
        });
        if !g.sources.is_empty() {
            ui.label(egui::RichText::new(format!("{} sources gathered · sections appear below as they're written", g.sources.len())).size(12.0).color(pal.text_faint));
        }
    });
    ui.add_space(14.0);
}

fn eta(g: &GenState) -> String {
    let elapsed = g.started.elapsed().as_secs_f32();
    if g.fraction > 0.08 && elapsed > 3.0 {
        let total = elapsed / g.fraction;
        let left = (total - elapsed).max(5.0);
        if left < 60.0 { format!("about {} s left", (left / 5.0).ceil() as u32 * 5) } else { format!("about {} min left", (left / 60.0).ceil() as u32) }
    } else {
        "usually 1–4 minutes".into()
    }
}

#[allow(clippy::too_many_arguments)]
fn section(ui: &mut Ui, pal: &Palette, images: &std::collections::HashMap<String, crate::app::ImgState>, s: &Section, sources: &[Source], notes: &[Note], doc: &Doc, acts: &mut Vec<Action>) {
    if !s.heading.is_empty() {
        ui.add_space(10.0);
        ui.add(egui::Label::new(egui::RichText::new(&s.heading).font(theme::bold(20.0)).color(pal.text)).wrap());
        ui.add_space(4.0);
    }
    for b in &s.blocks {
        match b {
            Block::Heading(h) => {
                ui.add_space(6.0);
                ui.label(egui::RichText::new(h).font(theme::bold(16.5)).color(pal.text));
            }
            Block::Paragraph(ps) => {
                let out = widgets::paragraph(ui, pal, ps, sources, 15.5, acts);
                out.response.context_menu(|ui| paragraph_menu(ui, ps, sources, notes, doc, out.hovered, acts));
                ui.add_space(8.0);
            }
            Block::Chart(spec) => {
                charts::chart(ui, pal, spec, sources, acts);
                ui.add_space(10.0);
            }
            Block::Image(img) => image_block(ui, pal, images, img, acts),
        }
    }
}

fn paragraph_menu(ui: &mut Ui, ps: &[Passage], sources: &[Source], notes: &[Note], doc: &Doc, hovered: Option<usize>, acts: &mut Vec<Action>) {
    ui.set_min_width(220.0);
    let text: String = ps.iter().map(|p| p.text.trim()).collect::<Vec<_>>().join(" ");
    if ui.button("Copy paragraph").clicked() {
        acts.push(Action::Copy(text.clone()));
    }
    if let Some(i) = hovered
        && let Some(p) = ps.get(i)
    {
        if ui.button("Copy this sentence").clicked() {
            acts.push(Action::Copy(p.text.clone()));
        }
        if let Some(o) = &p.original
            && ui.button("Copy the original wording").clicked()
        {
            acts.push(Action::Copy(o.clone()));
        }
    }
    let cited: Vec<Source> = sources.iter().filter(|s| ps.iter().any(|p| p.citations.contains(&s.id))).cloned().collect();
    let quote = NoteQuote { article_id: Some(doc.key()), paper_key: doc.paper().map(|p| p.key.clone()), passages: ps.to_vec(), references: cited };
    ui.menu_button("Quote into note", |ui| {
        for n in notes.iter().take(15) {
            if ui.button(crate::app::truncate(&n.title, 40)).clicked() {
                acts.push(Action::QuoteToNote { quote: quote.clone(), note: Some(n.id) });
            }
        }
        if !notes.is_empty() {
            ui.separator();
        }
        if ui.button("New note").clicked() {
            acts.push(Action::QuoteToNote { quote: quote.clone(), note: None });
        }
    });
    for s in sources.iter().filter(|s| ps.iter().any(|p| p.citations.contains(&s.id))).take(6) {
        if ui.button(format!("Open source [{}]", s.id)).on_hover_text(&s.title).clicked() {
            match &s.paper_key {
                Some(k) => acts.push(Action::OpenKey(k.clone())),
                None => {
                    if let Some(u) = s.link() {
                        acts.push(Action::OpenUrl(u));
                    }
                }
            }
        }
    }
}

fn image_block(ui: &mut Ui, pal: &Palette, images: &std::collections::HashMap<String, crate::app::ImgState>, img: &ImageRef, acts: &mut Vec<Action>) {
    ui.add_space(6.0);
    if img.embeddable {
        let w = ui.available_width();
        match images.get(&img.url) {
            Some(crate::app::ImgState::Ready(tex)) => {
                let size = tex.size_vec2();
                let scale = (w / size.x).min(420.0 / size.y).min(1.5);
                let r = ui.add(egui::Image::new((tex.id(), size * scale)).corner_radius(10).sense(Sense::click())).on_hover_text("View full screen");
                if r.clicked() {
                    acts.push(Action::ShowImage(img.clone()));
                }
            }
            Some(crate::app::ImgState::Failed) => {}
            Some(crate::app::ImgState::Loading) => {
                let (r, _) = ui.allocate_exact_size(vec2(w, 200.0), Sense::hover());
                ui.painter().rect_filled(r, 10.0, pal.card);
            }
            None => {
                acts.push(Action::LoadImage(img.url.clone()));
                let (r, _) = ui.allocate_exact_size(vec2(w, 200.0), Sense::hover());
                ui.painter().rect_filled(r, 10.0, pal.card);
            }
        }
    } else {
        if ui.add(egui::Button::image_and_text(icons::icon("image", 16.0, pal.link), egui::RichText::new(format!("Image: {} (copyrighted — view at source ↗)", img.title)).color(pal.link))).clicked() {
            acts.push(Action::OpenUrl(img.page_url.clone()));
        }
    }
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        ui.label(egui::RichText::new(&img.title).size(12.0).color(pal.text_weak));
        if !img.credit.is_empty() {
            ui.label(egui::RichText::new(format!("· {}", img.credit)).size(12.0).color(pal.text_faint));
        }
        ui.label(egui::RichText::new(format!("· {}", img.licence)).size(12.0).color(pal.text_faint));
        if ui.add(egui::Label::new(egui::RichText::new("· source ↗").size(12.0).color(pal.link)).sense(Sense::click())).clicked() {
            acts.push(Action::OpenUrl(img.page_url.clone()));
        }
        if !img.from_paper {
            ui.label(egui::RichText::new("· may not be from the paper").size(12.0).italics().color(pal.text_faint));
        }
    });
    ui.add_space(10.0);
}

fn references(ui: &mut Ui, pal: &Palette, sources: &[Source], style: CitationStyle, acts: &mut Vec<Action>) {
    if sources.is_empty() {
        return;
    }
    ui.add_space(14.0);
    ui.label(egui::RichText::new("References").font(theme::bold(20.0)));
    ui.label(egui::RichText::new(format!("{} style · real sources only — the AI article itself is never cited", style.label())).size(12.0).color(pal.text_faint));
    ui.add_space(6.0);
    for s in sources {
        let text = crate::export::cite::reference(s, style);
        let text = if text.trim().is_empty() { fallback_reference(s) } else { text };
        ui.horizontal_top(|ui| {
            ui.label(egui::RichText::new(format!("[{}]", s.id)).color(pal.link).size(13.0));
            let r = ui.add(egui::Label::new(egui::RichText::new(&text).size(13.0).color(pal.text_weak)).wrap().sense(Sense::click())).on_hover_cursor(egui::CursorIcon::PointingHand);
            let r = r.on_hover_text(if s.paper_key.is_some() { "Click: open in Gateless Scholar · right-click: more" } else { "Click: open the source · right-click: more" });
            if r.clicked() {
                match &s.paper_key {
                    Some(k) if s.kind != SourceKind::News => acts.push(Action::OpenKey(k.clone())),
                    _ => {
                        if let Some(u) = s.link() {
                            acts.push(Action::OpenUrl(u));
                        }
                    }
                }
            }
            r.context_menu(|ui| {
                if let Some(u) = s.link()
                    && ui.button("Open original in browser").clicked()
                {
                    acts.push(Action::OpenUrl(u));
                }
                if let Some(k) = &s.paper_key
                    && s.kind != SourceKind::News
                    && ui.button("Open in Gateless Scholar").clicked()
                {
                    acts.push(Action::OpenKey(k.clone()));
                }
                if ui.button("Copy reference").clicked() {
                    acts.push(Action::Copy(text.clone()));
                }
            });
        });
        ui.add_space(2.0);
    }
}

fn fallback_reference(s: &Source) -> String {
    let mut out = String::new();
    if !s.authors.is_empty() {
        out.push_str(&widgets::short_authors(&s.authors));
        out.push(' ');
    }
    out.push_str(&format!("({}) ", s.year.map(|y| y.to_string()).unwrap_or_else(|| "n.d.".into())));
    out.push_str(&format!("'{}'", s.title));
    if let Some(v) = &s.venue {
        out.push_str(&format!(", {v}"));
    }
    if let Some(l) = s.link() {
        out.push_str(&format!(". Available at: {l}"));
    }
    out
}

fn validation(ui: &mut Ui, pal: &Palette, v: &ValidationReport) {
    let any = v.invalid_citations_removed + v.unverified_sentences + v.charts_rejected + v.rewordings_reverted > 0 || !v.notes.is_empty();
    if !any {
        return;
    }
    ui.add_space(10.0);
    egui::CollapsingHeader::new(egui::RichText::new("How this article was checked").color(pal.text_weak)).id_salt("validation").show(ui, |ui| {
        let line = |ui: &mut Ui, n: u32, text: &str| {
            if n > 0 {
                ui.label(egui::RichText::new(format!("• {n} {text}")).size(13.0).color(pal.text_weak));
            }
        };
        line(ui, v.invalid_citations_removed, "citation(s) to sources that don't exist were removed");
        line(ui, v.unverified_sentences, "sentence(s) without a supporting source are marked “unverified”");
        line(ui, v.charts_rejected, "chart(s) were dropped because their numbers couldn't be found in the cited sources");
        line(ui, v.rewordings_reverted, "rewording(s) changed the original too much and were reverted to the original text");
        for n in &v.notes {
            ui.label(egui::RichText::new(format!("• {n}")).size(13.0).color(pal.text_weak));
        }
    });
}

fn footer(ui: &mut Ui, pal: &Palette, a: &Article, key: &str, acts: &mut Vec<Action>) {
    ui.separator();
    ui.add_space(6.0);
    ui.label(egui::RichText::new("Share this article").font(theme::bold(15.0)));
    ui.label(egui::RichText::new("Exports keep the AI label, the text-origin marking and the references.").size(12.0).color(pal.text_faint));
    ui.add_space(4.0);
    ui.horizontal_wrapped(|ui| {
        for (kind, icon, label) in [
            (ExportKind::Email, "mail", "Email as text"),
            (ExportKind::Docx, "file", "Word (.docx)"),
            (ExportKind::Pdf, "file", "PDF"),
            (ExportKind::Txt, "file", "Text (.txt)"),
            (ExportKind::Print, "print", "Print"),
        ] {
            if ui.add(egui::Button::image_and_text(icons::icon(icon, 15.0, pal.text_weak), label).min_size(vec2(0.0, 30.0))).clicked() {
                acts.push(Action::Export(kind, key.to_string()));
            }
        }
    });
    let _ = a;
}

fn share_menu(ui: &mut Ui, key: &str, acts: &mut Vec<Action>) {
    ui.set_min_width(200.0);
    for (kind, label) in [(ExportKind::Email, "Email as text…"), (ExportKind::Docx, "Export as Word (.docx)…"), (ExportKind::Pdf, "Export as PDF…"), (ExportKind::Txt, "Export as text (.txt)…"), (ExportKind::Print, "Print…")] {
        if ui.button(label).clicked() {
            acts.push(Action::Export(kind, key.to_string()));
        }
    }
}
