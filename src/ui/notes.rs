//! Lightweight notes: short notes with links to articles (clicking opens them; linking saves
//! them permanently) and quotes that keep their origin marking.

use std::time::{Duration, Instant};

use egui::{Align, Layout, Sense, Stroke, Ui, vec2};

use super::icons;
use super::theme;
use super::widgets::{self, icon_button};
use crate::app::{Action, App, View};
use crate::model::*;

impl App {
    pub fn notes_view(&mut self, ui: &mut Ui, project: Option<i64>) {
        let pal = self.pal;
        let notes = self.db.notes(project);
        let projects = self.db.projects();
        let mut acts = vec![];

        // Autosave a short moment after typing stops.
        if self.notes.dirty && self.notes.last_edit.is_some_and(|t| t.elapsed() > Duration::from_millis(800)) {
            self.flush_note();
        }
        if self.notes.dirty {
            ui.ctx().request_repaint_after(Duration::from_millis(300));
        }

        // Phones: the list on its own, or the open note on its own.
        let mobile = self.mobile;
        let show_list = !mobile || self.notes.selected.is_none();
        let list_w = if mobile { ui.available_width() } else { 250.0 };
        if show_list {
        egui::Panel::left("notes_list").exact_size(list_w).frame(egui::Frame::new().fill(pal.bg).inner_margin(10).stroke(Stroke::new(1.0, pal.border))).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Notes").font(theme::bold(18.0)));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if icon_button(ui, &pal, "plus", "New note (Ctrl+N)", false).clicked() {
                        self.flush_note();
                        if let Ok(id) = self.db.add_note(project, "Untitled note") {
                            self.notes.selected = Some(id);
                            self.notes.editing = None;
                        }
                    }
                });
            });
            egui::ComboBox::from_id_salt("notes_project")
                .width(list_w - 24.0)
                .selected_text(project.and_then(|id| projects.iter().find(|p| p.id == id)).map(|p| p.name.clone()).unwrap_or_else(|| "All notes".into()))
                .show_ui(ui, |ui| {
                    if ui.selectable_label(project.is_none(), "All notes").clicked() {
                        acts.push(Action::Navigate(View::Notes(None)));
                    }
                    for p in &projects {
                        if ui.selectable_label(project == Some(p.id), &p.name).clicked() {
                            acts.push(Action::Navigate(View::Notes(Some(p.id))));
                        }
                    }
                });
            ui.add_space(6.0);
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                if notes.is_empty() {
                    ui.label(egui::RichText::new("No notes yet. Quote or link articles from the reader, or press +.").size(12.5).color(pal.text_weak));
                }
                for n in &notes {
                    let sel = self.notes.selected == Some(n.id);
                    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 46.0), Sense::click());
                    if sel {
                        ui.painter().rect_filled(rect, 7.0, pal.accent_soft);
                    } else if resp.hovered() {
                        ui.painter().rect_filled(rect, 7.0, pal.card_hover);
                    }
                    ui.painter().text(rect.left_top() + vec2(8.0, 6.0), egui::Align2::LEFT_TOP, crate::app::truncate(&n.title, 30), theme::regular(14.0), pal.text);
                    let preview = n.body.lines().find(|l| !l.trim().is_empty()).unwrap_or(if n.quotes.is_empty() { "" } else { "Quote" });
                    ui.painter().text(rect.left_top() + vec2(8.0, 25.0), egui::Align2::LEFT_TOP, crate::app::truncate(preview, 34), theme::regular(11.5), pal.text_faint);
                    if resp.clicked() {
                        self.flush_note();
                        self.notes.selected = Some(n.id);
                        self.notes.editing = None;
                    }
                    resp.context_menu(|ui| {
                        if ui.button("Delete note").clicked() {
                            let _ = self.db.delete_note(n.id);
                            if self.notes.selected == Some(n.id) {
                                self.notes.selected = None;
                                self.notes.editing = None;
                            }
                        }
                    });
                }
            });
        });
        }
        if mobile && show_list {
            self.actions.extend(acts);
            return;
        }

        egui::CentralPanel::default().frame(egui::Frame::new().fill(pal.bg).inner_margin(16)).show(ui, |ui| {
            if mobile && ui.add(egui::Button::image_and_text(icons::icon("chevron-left", 15.0, pal.text_weak), "All notes").frame(false)).clicked() {
                self.flush_note();
                self.notes.selected = None;
                self.notes.editing = None;
                return;
            }
            let sel = self.notes.selected.or(if mobile { None } else { notes.first().map(|n| n.id) });
            if self.notes.editing.as_ref().map(|n| n.id) != sel {
                self.flush_note();
                self.notes.editing = sel.and_then(|id| self.db.notes(None).into_iter().find(|n| n.id == id));
                self.notes.selected = sel;
            }
            let Some(note) = self.notes.editing.as_mut() else {
                widgets::empty_state(ui, &pal, "note", "Notes", "Keep notes short: this app is for research, not writing up. Link articles with “+ › Link in note”, and right-click any paragraph to quote it.");
                return;
            };
            let mut changed = false;
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                widgets::column(ui, 760.0, |ui| {
                    changed |= ui.add(egui::TextEdit::singleline(&mut note.title).font(theme::bold(22.0)).frame(egui::Frame::NONE).desired_width(f32::INFINITY)).changed();
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(format!("Edited {}", note.updated.format("%e %b %Y %H:%M"))).size(12.0).color(pal.text_faint));
                        ui.label(egui::RichText::new("·").color(pal.text_faint));
                        egui::ComboBox::from_id_salt("note_project")
                            .selected_text(note.project_id.and_then(|id| projects.iter().find(|p| p.id == id)).map(|p| p.name.clone()).unwrap_or_else(|| "No project".into()))
                            .show_ui(ui, |ui| {
                                changed |= ui.selectable_value(&mut note.project_id, None, "No project").changed();
                                for p in &projects {
                                    changed |= ui.selectable_value(&mut note.project_id, Some(p.id), &p.name).changed();
                                }
                            });
                    });
                    ui.add_space(8.0);
                    let r = ui.add(
                        egui::TextEdit::multiline(&mut note.body)
                            .hint_text("Short notes. Links look like [[paper:<key>|Title]] — add them with “+ › Link in note” on any card.")
                            .desired_rows(10)
                            .desired_width(f32::INFINITY)
                            .font(theme::regular(14.5)),
                    );
                    changed |= r.changed();

                    // Linked articles: click to open.
                    let links = parse_links(&note.body);
                    if !links.is_empty() {
                        ui.add_space(8.0);
                        ui.label(egui::RichText::new("LINKED ARTICLES (kept permanently)").size(11.5).strong().color(pal.text_faint));
                        ui.horizontal_wrapped(|ui| {
                            for (key, title) in &links {
                                if ui.add(egui::Button::image_and_text(icons::icon("link", 14.0, pal.link), egui::RichText::new(crate::app::truncate(title, 60)).color(pal.link))).on_hover_text("Open").clicked() {
                                    acts.push(Action::OpenKey(key.clone()));
                                }
                            }
                        });
                    }

                    // Quotes, with their origin marking.
                    let mut remove = None;
                    for (qi, q) in note.quotes.iter().enumerate() {
                        ui.add_space(12.0);
                        egui::Frame::new().fill(pal.card).stroke(Stroke::new(1.0, pal.border)).corner_radius(10).inner_margin(12).show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal(|ui| {
                                ui.add(icons::icon("quote", 15.0, pal.text_faint));
                                let any_ai = q.passages.iter().any(|p| p.origin == Origin::AiWritten);
                                let any_real = q.passages.iter().any(|p| p.origin != Origin::AiWritten);
                                let label = match (any_ai, any_real) {
                                    (true, true) => "Mixed quote: parts are from the real paper, parts are AI-written",
                                    (true, false) => "AI-written quote",
                                    _ => "Quote from the original",
                                };
                                ui.label(egui::RichText::new(label).size(12.0).color(if any_ai { pal.ai } else { pal.text_faint }));
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    if icon_button(ui, &pal, "trash", "Remove quote", false).clicked() {
                                        remove = Some(qi);
                                    }
                                    if let Some(k) = q.article_id.clone().or(q.paper_key.clone())
                                        && icon_button(ui, &pal, "external", "Open the article", false).clicked()
                                    {
                                        acts.push(Action::OpenKey(k));
                                    }
                                });
                            });
                            widgets::paragraph(ui, &pal, &q.passages, &q.references, 14.5, &mut acts);
                            let all_ai = q.passages.iter().all(|p| p.origin == Origin::AiWritten);
                            if all_ai {
                                egui::Frame::new().fill(pal.ai_soft).corner_radius(8).inner_margin(10).show(ui, |ui| {
                                    ui.set_width(ui.available_width());
                                    ui.label(egui::RichText::new("Rewrite this in your own words from your own research").strong().color(pal.ai));
                                    ui.label(egui::RichText::new("This text was written by the AI. Read the sources it used and write your own version — cite them, not the AI:").size(12.5).color(pal.text_weak));
                                    for s in &q.references {
                                        let r = ui.add(egui::Label::new(egui::RichText::new(format!("[{}] {}", s.id, crate::export::cite::reference(s, CitationStyle::Harvard))).size(12.5).color(pal.link)).wrap().sense(Sense::click()));
                                        if r.clicked() {
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
                                    if q.references.is_empty() {
                                        ui.label(egui::RichText::new("(The quoted sentences cite no sources — treat them as unverified.)").size(12.0).color(pal.warn));
                                    }
                                });
                            }
                        });
                    }
                    if let Some(i) = remove {
                        note.quotes.remove(i);
                        changed = true;
                    }
                    widgets::keep_focused_visible(ui);
                });
            });
            if changed {
                self.notes.dirty = true;
                self.notes.last_edit = Some(Instant::now());
            }
        });
        self.actions.extend(acts);
    }

    pub fn flush_note(&mut self) {
        if self.notes.dirty
            && let Some(n) = &self.notes.editing
        {
            if let Err(e) = self.db.save_note(n) {
                log::warn!("saving note: {e}");
            }
            self.notes.dirty = false;
        }
    }
}

fn parse_links(body: &str) -> Vec<(String, String)> {
    let mut out = vec![];
    let mut rest = body;
    while let Some(start) = rest.find("[[") {
        let after = &rest[start + 2..];
        let Some(end) = after.find("]]") else { break };
        let inner = &after[..end];
        let mut parts = inner.splitn(2, '|');
        let target = parts.next().unwrap_or("");
        let title = parts.next().unwrap_or(target).to_string();
        if let Some(k) = target.strip_prefix("article:").or_else(|| target.strip_prefix("paper:")) {
            out.push((k.to_string(), title));
        }
        rest = &after[end + 2..];
    }
    out
}
