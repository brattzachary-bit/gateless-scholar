//! Favourites, Read Later, bookshelves, projects, history, the generation queue,
//! notifications and the bibliography builder.

use std::collections::HashSet;

use egui::{Align, Layout, Sense, Stroke, Ui, vec2};

use super::icons;
use super::theme;
use super::widgets::{self, icon_button};
use crate::app::{Action, App, Modal, ToastKind, View};
use crate::db::QueueStatus;
use crate::model::*;

impl App {
    fn doc_list(&mut self, ui: &mut Ui, docs: Vec<Doc>, salt: &str, remove_from: Option<SavedList>) {
        let mut expanded = std::mem::take(&mut self.discover.expanded);
        let data = self.card_data();
        let env = self.card_env(&data);
        let mut acts = vec![];
        egui::ScrollArea::vertical().auto_shrink([false, false]).id_salt(salt).show(ui, |ui| {
            widgets::column(ui, 980.0, |ui| {
                for doc in &docs {
                    let key = doc.key();
                    let mut ex = expanded.contains(&key);
                    let before = ex;
                    let r = widgets::doc_card(ui, &env, doc, &mut ex, &mut acts);
                    if let Some(list) = remove_from {
                        r.context_menu(|ui| {
                            if ui.button("Remove from this list").clicked() {
                                acts.push(Action::ToggleSave(list, doc.clone()));
                            }
                        });
                    }
                    if ex != before {
                        if ex {
                            expanded.insert(key);
                        } else {
                            expanded.remove(&key);
                        }
                    }
                    ui.add_space(8.0);
                }
                ui.add_space(20.0);
            });
        });
        drop(env);
        self.discover.expanded = expanded;
        self.actions.extend(acts);
    }

    pub fn saved_view(&mut self, ui: &mut Ui, list: SavedList) {
        let pal = self.pal;
        let (title, sub, icon) = match list {
            SavedList::Favourites => ("Favourites", "Kept permanently.", "heart"),
            SavedList::ReadLater => ("Read Later", "Kept permanently. Adding something here also queues its full article for writing.", "clock"),
            SavedList::NoteLink => ("Linked in notes", "Kept permanently because a note links to them.", "note"),
            _ => ("Saved", "", "books"),
        };
        let docs: Vec<Doc> = self.db.saved(list).into_iter().map(|e| e.doc).collect();
        widgets::column(ui, 980.0, |ui| {
            widgets::heading(ui, &pal, title, Some(sub));
            if docs.is_empty() {
                widgets::empty_state(ui, &pal, icon, "Nothing here yet", "Use the buttons on any card or article to add it.");
            }
        });
        if !docs.is_empty() {
            self.doc_list(ui, docs, &format!("saved-{}", list.key()), Some(list));
        }
    }

    pub fn bookshelves_view(&mut self, ui: &mut Ui) {
        let pal = self.pal;
        let shelves = self.db.bookshelves();
        let projects = self.db.projects();
        let selected = ui.ctx().data(|d| d.get_temp::<i64>(egui::Id::unique("shelf_sel")));
        let mut acts = vec![];
        let mut select = None;
        let mut rename: Option<Bookshelf> = None;
        let mut delete = None;
        widgets::column(ui, 980.0, |ui| {
            widgets::title_row(ui, &pal, "Bookshelves", Some("Your own collections. Kept permanently."), |ui| {
                    if ui.add(egui::Button::image_and_text(icons::icon("plus", 15.0, pal.text), "New bookshelf")).clicked() {
                        self.modal = Some(Modal::NewBookshelf { name: String::new(), doc: None });
                    }
            });
            if shelves.is_empty() {
                widgets::empty_state(ui, &pal, "books", "No bookshelves yet", "Create one, or use “+ › Add to bookshelf” on any card.");
                return;
            }
            ui.horizontal_wrapped(|ui| {
                for s in &shelves {
                    let n = self.db.saved(SavedList::Bookshelf(s.id)).len();
                    let sel = selected == Some(s.id);
                    let r = ui.add(egui::Button::image_and_text(icons::icon("books", 15.0, if sel { pal.accent } else { pal.text_weak }), format!("{}  ({n})", s.name)).selected(sel).min_size(vec2(0.0, 30.0)));
                    if r.clicked() {
                        select = Some(s.id);
                    }
                    r.context_menu(|ui| {
                        ui.menu_button("Move to project", |ui| {
                            if ui.button("No project").clicked() {
                                rename = Some(Bookshelf { project_id: None, ..s.clone() });
                            }
                            for p in &projects {
                                if ui.button(&p.name).clicked() {
                                    rename = Some(Bookshelf { project_id: Some(p.id), ..s.clone() });
                                }
                            }
                        });
                        if ui.button("Delete bookshelf").on_hover_text("Items stay in your other lists").clicked() {
                            delete = Some(s.id);
                        }
                    });
                }
            });
            ui.add_space(8.0);
        });
        if let Some(id) = select {
            ui.ctx().data_mut(|d| d.insert_temp(egui::Id::unique("shelf_sel"), id));
        }
        if let Some(b) = rename {
            let _ = self.db.update_bookshelf(&b);
        }
        if let Some(id) = delete {
            let _ = self.db.delete_bookshelf(id);
        }
        let current = selected.or(shelves.first().map(|s| s.id));
        if let Some(id) = current {
            let docs: Vec<Doc> = self.db.saved(SavedList::Bookshelf(id)).into_iter().map(|e| e.doc).collect();
            if docs.is_empty() {
                widgets::column(ui, 980.0, |ui| widgets::empty_state(ui, &pal, "books", "Empty bookshelf", "Add articles from any card's “+” menu."));
            } else {
                self.doc_list(ui, docs, &format!("shelf-{id}"), Some(SavedList::Bookshelf(id)));
            }
        }
        self.actions.extend(acts.drain(..));
    }

    pub fn projects_view(&mut self, ui: &mut Ui, project: Option<i64>) {
        let pal = self.pal;
        let projects = self.db.projects();
        let mut acts: Vec<Action> = vec![];
        let Some(pid) = project.or(None) else {
            widgets::column(ui, 980.0, |ui| {
                widgets::title_row(ui, &pal, "Projects", Some("Group notes, bookshelves and articles under one research topic."), |ui| {
                        if ui.add(egui::Button::image_and_text(icons::icon("plus", 15.0, pal.text), "New project")).clicked() {
                            self.modal = Some(Modal::NewProject { name: String::new(), description: String::new(), doc: None });
                        }
                });
                if projects.is_empty() {
                    widgets::empty_state(ui, &pal, "folder", "No projects yet", "A project collects everything for one research topic, and builds its bibliography.");
                }
                for p in &projects {
                    let n_articles = self.db.saved(SavedList::Project(p.id)).len();
                    let n_notes = self.db.notes(Some(p.id)).len();
                    let r = egui::Frame::new()
                        .fill(pal.card)
                        .stroke(Stroke::new(1.0, pal.border))
                        .corner_radius(12)
                        .inner_margin(14)
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal(|ui| {
                                ui.add(icons::icon("folder", 20.0, pal.accent));
                                ui.label(egui::RichText::new(&p.name).font(theme::bold(16.0)));
                            });
                            if !p.description.is_empty() {
                                ui.label(egui::RichText::new(&p.description).color(pal.text_weak));
                            }
                            ui.label(egui::RichText::new(format!("{n_articles} articles · {n_notes} notes")).size(12.0).color(pal.text_faint));
                        })
                        .response
                        .interact(Sense::click());
                    if r.clicked() {
                        acts.push(Action::Navigate(View::Projects(Some(p.id))));
                    }
                    r.context_menu(|ui| {
                        if ui.button("Open").clicked() {
                            acts.push(Action::Navigate(View::Projects(Some(p.id))));
                        }
                        if ui.button("Delete project").on_hover_text("Its notes and bookshelves are kept, just ungrouped").clicked() {
                            let _ = self.db.delete_project(p.id);
                        }
                    });
                    ui.add_space(8.0);
                }
            });
            self.actions.extend(acts);
            return;
        };
        let Some(p) = projects.iter().find(|p| p.id == pid).cloned() else {
            self.actions.push(Action::Navigate(View::Projects(None)));
            return;
        };
        let notes = self.db.notes(Some(pid));
        let shelves: Vec<Bookshelf> = self.db.bookshelves().into_iter().filter(|b| b.project_id == Some(pid)).collect();
        widgets::column(ui, 980.0, |ui| {
            ui.horizontal(|ui| {
                widgets::nav_link(ui, &pal, "Projects", View::Projects(None), &mut acts);
                ui.label(egui::RichText::new("›").color(pal.text_faint));
            });
            widgets::heading(ui, &pal, &p.name, (!p.description.is_empty()).then_some(p.description.as_str()));
            ui.horizontal(|ui| {
                if ui.add(egui::Button::image_and_text(icons::icon("note", 15.0, pal.text), "New note")).clicked()
                    && let Ok(id) = self.db.add_note(Some(pid), "Untitled note")
                {
                    self.notes.selected = Some(id);
                    self.notes.editing = None;
                    acts.push(Action::Navigate(View::Notes(Some(pid))));
                }
                if ui.add(egui::Button::image_and_text(icons::icon("quote", 15.0, pal.text), "Bibliography")).clicked() {
                    self.bib_project = Some(pid);
                    acts.push(Action::Navigate(View::Bibliography));
                }
                if ui.add(egui::Button::image_and_text(icons::icon("bell", 15.0, pal.text), "Follow this topic")).on_hover_text("Get notified about new research on it").clicked() {
                    let f = Follow::Topic(p.name.clone());
                    if !self.settings.follows.contains(&f) {
                        self.settings.follows.push(f);
                        acts.push(Action::SaveSettings);
                        acts.push(Action::Toast(format!("Following “{}”", p.name), ToastKind::Success));
                    }
                }
            });
            ui.add_space(8.0);
            ui.label(egui::RichText::new(format!("NOTES ({})", notes.len())).size(12.0).strong().color(pal.text_faint));
            if notes.is_empty() {
                ui.label(egui::RichText::new("No notes in this project yet.").color(pal.text_weak));
            }
            for n in &notes {
                let r = ui.add(egui::Button::image_and_text(icons::icon("note", 14.0, pal.text_weak), format!("{}  ·  {}", n.title, n.updated.format("%e %b"))).frame(false));
                if r.clicked() {
                    self.notes.selected = Some(n.id);
                    self.notes.editing = None;
                    acts.push(Action::Navigate(View::Notes(Some(pid))));
                }
            }
            if !shelves.is_empty() {
                ui.add_space(8.0);
                ui.label(egui::RichText::new("BOOKSHELVES").size(12.0).strong().color(pal.text_faint));
                ui.horizontal_wrapped(|ui| {
                    for s in &shelves {
                        if ui.button(format!("{} ({})", s.name, self.db.saved(SavedList::Bookshelf(s.id)).len())).clicked() {
                            ui.ctx().data_mut(|d| d.insert_temp(egui::Id::unique("shelf_sel"), s.id));
                            acts.push(Action::Navigate(View::Bookshelves));
                        }
                    }
                });
            }
            ui.add_space(8.0);
            ui.label(egui::RichText::new("ARTICLES").size(12.0).strong().color(pal.text_faint));
        });
        let mut docs: Vec<Doc> = self.db.saved(SavedList::Project(pid)).into_iter().map(|e| e.doc).collect();
        for s in &shelves {
            for e in self.db.saved(SavedList::Bookshelf(s.id)) {
                if !docs.iter().any(|d| d.key() == e.doc.key()) {
                    docs.push(e.doc);
                }
            }
        }
        self.actions.extend(acts);
        if docs.is_empty() {
            widgets::column(ui, 980.0, |ui| ui.label(egui::RichText::new("Add articles with “+ › Add to project” on any card.").color(pal.text_weak)));
        } else {
            self.doc_list(ui, docs, &format!("project-{pid}"), Some(SavedList::Project(pid)));
        }
    }

    pub fn history_view(&mut self, ui: &mut Ui) {
        let pal = self.pal;
        let hist = self.db.history(300);
        widgets::column(ui, 980.0, |ui| {
            widgets::heading(ui, &pal, "History", Some("Recently read. Past Discover days are below each Discover tab. History is limited by disk size (Settings › Storage)."));
            if hist.is_empty() {
                widgets::empty_state(ui, &pal, "history", "No history yet", "Articles you open appear here.");
            }
        });
        if !hist.is_empty() {
            self.doc_list(ui, hist.into_iter().map(|(d, _)| d).collect(), "history", None);
        }
    }

    pub fn queue_view(&mut self, ui: &mut Ui) {
        let pal = self.pal;
        let queue = self.db.queue();
        let mut acts = vec![];
        widgets::column(ui, 980.0, |ui| {
            widgets::title_row(ui, &pal, "Generation Queue", Some("Full articles waiting to be written (Read Later adds here). One is written at a time while the app is open."), |ui| {
                    if ui.button("Clear finished").clicked() {
                        let _ = self.db.clear_finished_queue();
                    }
            });
            if queue.is_empty() {
                widgets::empty_state(ui, &pal, "queue", "The queue is empty", "Add things to Read Later to have their full articles written in the background.");
                return;
            }
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                for e in &queue {
                    let key = e.doc.key();
                    egui::Frame::new().fill(pal.card).stroke(Stroke::new(1.0, pal.border)).corner_radius(10).inner_margin(12).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            let (color, icon) = match e.status {
                                QueueStatus::Queued => (pal.text_weak, "clock"),
                                QueueStatus::Running => (pal.accent, "sparkle"),
                                QueueStatus::Done => (pal.link, "check"),
                                QueueStatus::Failed => (pal.danger, "alert"),
                            };
                            ui.add(icons::icon(icon, 18.0, color));
                            ui.vertical(|ui| {
                                let t = ui.add(egui::Label::new(egui::RichText::new(e.doc.title()).strong()).sense(Sense::click())).on_hover_cursor(egui::CursorIcon::PointingHand);
                                if t.clicked() {
                                    acts.push(Action::Open(e.doc.clone()));
                                }
                                ui.horizontal(|ui| {
                                    widgets::kind_badge(ui, &pal, e.doc.kind());
                                    ui.label(egui::RichText::new(e.status.label()).color(color));
                                    if let Some(g) = self.gens.get(&key)
                                        && g.error.is_none()
                                    {
                                        ui.add(egui::ProgressBar::new(g.fraction).desired_width(160.0).desired_height(5.0).fill(pal.accent));
                                        ui.label(egui::RichText::new(&g.text).size(12.0).color(pal.text_faint));
                                    }
                                });
                                if let Some(err) = &e.error {
                                    ui.label(egui::RichText::new(err).size(12.0).color(pal.danger));
                                }
                            });
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if icon_button(ui, &pal, "trash", "Remove from queue", false).clicked() {
                                    let _ = self.db.remove_from_queue(&key);
                                }
                                if e.status == QueueStatus::Failed && icon_button(ui, &pal, "refresh", "Retry", false).clicked() {
                                    let _ = self.db.enqueue(&e.doc);
                                    self.gens.remove(&key);
                                }
                                if e.status == QueueStatus::Done && ui.button("Read").clicked() {
                                    acts.push(Action::Open(e.doc.clone()));
                                }
                            });
                        });
                    });
                    ui.add_space(6.0);
                }
            });
        });
        self.actions.extend(acts);
    }

    pub fn notifications_view(&mut self, ui: &mut Ui) {
        let pal = self.pal;
        let list = self.db.notifications(200);
        let mut acts = vec![];
        widgets::column(ui, 980.0, |ui| {
            widgets::title_row(ui, &pal, "Notifications", Some("New research from topics and authors you follow, checked while the app is open."), |ui| {
                    if ui.button("Clear all").clicked() {
                        let _ = self.db.clear_notifications();
                    }
                    if ui.button("Check now").clicked() {
                        self.last_follow_check = None;
                    }
            });
            // Follows.
            egui::Frame::new().fill(pal.card).stroke(Stroke::new(1.0, pal.border)).corner_radius(10).inner_margin(12).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(egui::RichText::new("FOLLOWING").size(12.0).strong().color(pal.text_faint));
                let mut remove = None;
                ui.horizontal_wrapped(|ui| {
                    for (i, f) in self.settings.follows.iter().enumerate() {
                        let label = match f {
                            Follow::Topic(t) => format!("# {t}"),
                            Follow::Author(a) => format!("@ {a}"),
                        };
                        if ui.button(format!("{label}  ✕")).on_hover_text("Unfollow").clicked() {
                            remove = Some(i);
                        }
                    }
                    if self.settings.follows.is_empty() {
                        ui.label(egui::RichText::new("Nothing yet.").color(pal.text_weak));
                    }
                });
                if let Some(i) = remove {
                    self.settings.follows.remove(i);
                    acts.push(Action::SaveSettings);
                }
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.settings_ui.new_follow_author, false, "Topic");
                    ui.selectable_value(&mut self.settings_ui.new_follow_author, true, "Author");
                    let r = ui.add(egui::TextEdit::singleline(&mut self.settings_ui.new_follow).hint_text(if self.settings_ui.new_follow_author { "Author name" } else { "Topic, e.g. exoplanets" }).desired_width(260.0));
                    let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if (ui.button("Follow").clicked() || enter) && !self.settings_ui.new_follow.trim().is_empty() {
                        let v = self.settings_ui.new_follow.trim().to_string();
                        let f = if self.settings_ui.new_follow_author { Follow::Author(v) } else { Follow::Topic(v) };
                        if !self.settings.follows.contains(&f) {
                            self.settings.follows.push(f);
                        }
                        self.settings_ui.new_follow.clear();
                        self.last_follow_check = None;
                        acts.push(Action::SaveSettings);
                    }
                });
            });
            ui.add_space(10.0);
            if list.is_empty() {
                widgets::empty_state(ui, &pal, "bell", "No notifications", "Follow topics or authors to hear about new research.");
            }
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                for n in &list {
                    let r = egui::Frame::new()
                        .fill(if n.read { pal.card } else { pal.accent_soft })
                        .stroke(Stroke::new(1.0, pal.border))
                        .corner_radius(10)
                        .inner_margin(12)
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.label(egui::RichText::new(&n.title).size(12.0).color(pal.text_faint));
                            ui.label(egui::RichText::new(&n.body).strong());
                            if let Some(p) = &n.paper {
                                ui.label(egui::RichText::new(p.author_line(4)).size(12.5).color(pal.text_weak));
                            }
                            ui.label(egui::RichText::new(n.created.format("%e %b %Y %H:%M").to_string()).size(11.5).color(pal.text_faint));
                        })
                        .response
                        .interact(Sense::click());
                    if r.clicked()
                        && let Some(p) = &n.paper
                    {
                        acts.push(Action::Open(Doc::Paper(p.clone())));
                    }
                    ui.add_space(6.0);
                }
            });
        });
        self.actions.extend(acts);
    }

    /// Collect the real references behind a project (or all saved items) and export them.
    pub fn bibliography_view(&mut self, ui: &mut Ui) {
        let pal = self.pal;
        let projects = self.db.projects();
        let style = self.settings.citation_style;
        let mut acts = vec![];
        let papers = self.bibliography_papers();
        widgets::column(ui, 980.0, |ui| {
            widgets::heading(ui, &pal, "Bibliography", Some("References from a project's articles and notes, for your own work. Only real sources are cited — never the AI articles themselves."));
            ui.horizontal(|ui| {
                ui.label("From");
                egui::ComboBox::from_id_salt("bib_project")
                    .selected_text(self.bib_project.and_then(|id| projects.iter().find(|p| p.id == id)).map(|p| p.name.clone()).unwrap_or_else(|| "All saved items".into()))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.bib_project, None, "All saved items");
                        for p in &projects {
                            ui.selectable_value(&mut self.bib_project, Some(p.id), &p.name);
                        }
                    });
                ui.label("Style");
                egui::ComboBox::from_id_salt("bib_style").selected_text(style.label()).show_ui(ui, |ui| {
                    for s in CitationStyle::ALL {
                        if ui.selectable_label(style == s, s.label()).clicked() {
                            self.settings.citation_style = s;
                            acts.push(Action::SaveSettings);
                        }
                    }
                });
            });
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let text: String = papers.iter().map(|p| crate::export::cite::paper_reference(p, style)).collect::<Vec<_>>().join("\n\n");
                if ui.add_enabled(!papers.is_empty(), egui::Button::image_and_text(icons::icon("copy", 15.0, pal.text), "Copy")).clicked() {
                    acts.push(Action::Copy(text.clone()));
                }
                if ui.add_enabled(!papers.is_empty(), egui::Button::image_and_text(icons::icon("download", 15.0, pal.text), format!("Save as text ({})", style.label()))).clicked() {
                    crate::jobs::save_text_file(self, "bibliography.txt".into(), ("Text", "txt"), text);
                }
                if ui.add_enabled(!papers.is_empty(), egui::Button::image_and_text(icons::icon("download", 15.0, pal.text), "BibTeX (.bib)")).on_hover_text("For Zotero, JabRef, LaTeX").clicked() {
                    crate::jobs::save_text_file(self, "bibliography.bib".into(), ("BibTeX", "bib"), crate::export::cite::bibtex(&papers));
                }
                if ui.add_enabled(!papers.is_empty(), egui::Button::image_and_text(icons::icon("download", 15.0, pal.text), "RIS (.ris)")).on_hover_text("For Zotero, Mendeley, EndNote").clicked() {
                    crate::jobs::save_text_file(self, "bibliography.ris".into(), ("RIS", "ris"), crate::export::cite::ris(&papers));
                }
            });
            ui.add_space(8.0);
            if papers.is_empty() {
                widgets::empty_state(ui, &pal, "quote", "No references yet", "Save articles to a project (or anywhere) and their real sources appear here.");
                return;
            }
            ui.label(egui::RichText::new(format!("{} references", papers.len())).size(12.0).color(pal.text_faint));
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                for p in &papers {
                    let r = crate::export::cite::paper_reference(p, style);
                    ui.add(egui::Label::new(egui::RichText::new(&r).size(13.5)).wrap()).context_menu(|ui| {
                        if ui.button("Copy reference").clicked() {
                            acts.push(Action::Copy(r.clone()));
                        }
                        if let Some(u) = p.url.clone().or(p.doi.as_ref().map(|d| format!("https://doi.org/{d}")))
                            && ui.button("Open original").clicked()
                        {
                            acts.push(Action::OpenUrl(u));
                        }
                    });
                    ui.add_space(6.0);
                }
            });
        });
        self.actions.extend(acts);
    }

    /// Real papers behind the selected project's (or all saved) items: the papers themselves
    /// plus every source their written articles and quoted notes cite.
    fn bibliography_papers(&self) -> Vec<Paper> {
        let mut docs: Vec<Doc> = vec![];
        let mut sources: Vec<Source> = vec![];
        match self.bib_project {
            Some(pid) => {
                docs.extend(self.db.saved(SavedList::Project(pid)).into_iter().map(|e| e.doc));
                for b in self.db.bookshelves().into_iter().filter(|b| b.project_id == Some(pid)) {
                    docs.extend(self.db.saved(SavedList::Bookshelf(b.id)).into_iter().map(|e| e.doc));
                }
                for n in self.db.notes(Some(pid)) {
                    for k in crate::db::note_links(&n.body) {
                        if let Some(d) = self.db.find_doc(&k) {
                            docs.push(d);
                        }
                    }
                    for q in n.quotes {
                        sources.extend(q.references);
                    }
                }
            }
            None => {
                for list in [SavedList::Favourites, SavedList::ReadLater, SavedList::NoteLink] {
                    docs.extend(self.db.saved(list).into_iter().map(|e| e.doc));
                }
                for b in self.db.bookshelves() {
                    docs.extend(self.db.saved(SavedList::Bookshelf(b.id)).into_iter().map(|e| e.doc));
                }
            }
        }
        let mut out: Vec<Paper> = vec![];
        let mut seen = HashSet::new();
        let mut push = |p: Paper, out: &mut Vec<Paper>| {
            let k = p.doi.clone().map(|d| normalize_doi(&d)).or(p.url.clone()).unwrap_or_else(|| p.title.to_lowercase());
            if seen.insert(k) {
                out.push(p);
            }
        };
        for d in &docs {
            if let Some(p) = d.paper()
                && matches!(d.kind(), ContentKind::FreeFullText | ContentKind::AbstractOnly)
            {
                push(p.clone(), &mut out);
            }
            if let Some(a) = self.db.get_article(&d.key()) {
                let cited: HashSet<u32> = a.passages().flat_map(|p| p.citations.iter().copied()).collect();
                sources.extend(a.sources.into_iter().filter(|s| cited.contains(&s.id)));
            } else if let Doc::Feed(f) = d {
                for b in &f.basis {
                    push(b.clone(), &mut out);
                }
            }
        }
        for s in sources {
            push(source_to_paper(&s), &mut out);
        }
        out.sort_by(|a, b| a.authors.first().cloned().unwrap_or_default().to_lowercase().cmp(&b.authors.first().cloned().unwrap_or_default().to_lowercase()));
        out
    }
}

pub fn source_to_paper(s: &Source) -> Paper {
    let mut p = Paper {
        doi: s.doi.clone(),
        title: s.title.clone(),
        authors: s.authors.clone(),
        year: s.year,
        date: s.date,
        venue: s.venue.clone(),
        publisher: s.publisher.clone(),
        url: s.url.clone(),
        kind: if s.kind == SourceKind::News { ContentKind::News } else { ContentKind::AbstractOnly },
        ..Default::default()
    };
    p.key = s.paper_key.clone().unwrap_or_else(|| p.make_key());
    p
}
