//! Discover Research (home) and Discover News: topic tabs, today's cards as they are written,
//! older days below, infinite scroll.

use std::collections::HashSet;

use chrono::Utc;
use egui::{Align, Color32, Layout, Rect, Sense, Stroke, Ui, Vec2, vec2};

use super::theme;
use super::widgets::{self, CardEnv, icon_button};
use super::icons;
use crate::app::{Action, App};
use crate::model::*;

/// What the cards need from the database, gathered once per frame.
pub struct CardData {
    pub favourites: HashSet<String>,
    pub read_later: HashSet<String>,
    pub written: HashSet<String>,
    pub shelves: Vec<Bookshelf>,
    pub projects: Vec<Project>,
    pub notes: Vec<Note>,
    pub brand: String,
}

impl App {
    pub fn card_data(&self) -> CardData {
        CardData {
            favourites: self.db.saved(SavedList::Favourites).into_iter().map(|e| e.doc.key()).collect(),
            read_later: self.db.saved(SavedList::ReadLater).into_iter().map(|e| e.doc.key()).collect(),
            written: self.db.article_keys(),
            shelves: self.db.bookshelves(),
            projects: self.db.projects(),
            notes: self.db.notes(None),
            brand: self.provider_brand(),
        }
    }

    pub fn card_env<'a>(&'a self, d: &'a CardData) -> CardEnv<'a> {
        CardEnv {
            pal: &self.pal,
            images: &self.images,
            found_images: &self.found_images,
            brand: &d.brand,
            favourites: &d.favourites,
            read_later: &d.read_later,
            written: &d.written,
            shelves: &d.shelves,
            projects: &d.projects,
            notes: &d.notes,
        }
    }

    pub fn discover_view(&mut self, ui: &mut Ui) {
        let pal = self.pal;
        let data = self.card_data();
        let mut acts = vec![];
        ui.add_space(8.0);
        widgets::column(ui, 980.0, |ui| {
            // Header: Research / News switch, AI-only toggle, refresh, progress.
            ui.horizontal(|ui| {
                let short = ui.available_width() < 560.0;
                for (feed, label, icon) in [(FeedKind::Research, if short { "Research" } else { "Discover Research" }, "flask"), (FeedKind::News, if short { "News" } else { "Discover News" }, "newspaper")] {
                    let sel = self.discover.feed == feed;
                    let r = ui
                        .add(
                            egui::Button::image_and_text(icons::icon(icon, 16.0, if sel { pal.accent } else { pal.text_weak }), egui::RichText::new(label).font(theme::bold(15.0)).color(if sel { pal.text } else { pal.text_weak }))
                                .fill(if sel { pal.accent_soft } else { Color32::TRANSPARENT })
                                .corner_radius(16)
                                .min_size(vec2(0.0, 32.0)),
                        )
                        .on_hover_text("Tab switches between Research and News");
                    if r.clicked() && !sel {
                        self.discover.feed = feed;
                        self.discover.needs_reload = true;
                    }
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if icon_button(ui, &pal, "refresh", "Refresh today's Discover (Ctrl+R)", false).clicked() {
                        acts.push(Action::RunDaily(true));
                    }
                    if self.discover.feed == FeedKind::Research && !self.mobile {
                        let mut hide = self.settings.hide_ai_only;
                        if ui.checkbox(&mut hide, "Hide AI-only articles").on_hover_text("Show only real papers in Discover Research").changed() {
                            self.settings.hide_ai_only = hide;
                            acts.push(Action::SaveSettings);
                        }
                    }
                });
            });
            if self.daily.running {
                ui.add_space(4.0);
                let frac = if self.daily.total > 0 { self.daily.done as f32 / self.daily.total as f32 } else { 0.0 };
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(egui::RichText::new(&self.daily.text).size(12.5).color(pal.text_weak));
                });
                ui.add(egui::ProgressBar::new(frac).desired_height(4.0).fill(pal.accent));
            }
            ui.add_space(6.0);
            self.topic_tabs(ui, &mut acts);
            ui.add_space(8.0);
        });

        let mut expanded = std::mem::take(&mut self.discover.expanded);
        let env = self.card_env(&data);
        let hide_ai = self.settings.hide_ai_only && self.discover.feed == FeedKind::Research;
        let items: Vec<FeedItem> = self.discover.items.iter().filter(|i| !(hide_ai && i.kind == ContentKind::AiArticle)).cloned().collect();
        let today = Utc::now().date_naive();
        let empty = items.is_empty();
        let running = self.daily.running;
        let has_provider = self.has_provider();
        let feed = self.discover.feed;
        let loading_more = self.discover.loading_more;
        let more_exhausted = self.discover.more_exhausted && self.discover.db_exhausted;
        let out = egui::ScrollArea::vertical().auto_shrink([false, false]).id_salt(("feed", feed, self.discover.tab)).show(ui, |ui| {
            widgets::column(ui, 980.0, |ui| {
                if empty {
                    if running {
                        ui.add_space(30.0);
                        ui.vertical_centered(|ui| {
                            ui.spinner();
                            ui.label(egui::RichText::new("Finding today's research…").color(pal.text_weak));
                        });
                    } else {
                        widgets::empty_state(
                            ui,
                            &pal,
                            if feed == FeedKind::News { "newspaper" } else { "flask" },
                            "Nothing here yet",
                            "Press refresh to gather today's headlines for this tab. Real papers appear without an AI model; AI articles and news summaries need one.",
                        );
                        ui.vertical_centered(|ui| {
                            if ui.button("Gather today's Discover").clicked() {
                                acts.push(Action::RunDaily(true));
                            }
                        });
                    }
                }
                let mut last_day = None;
                for item in &items {
                    if last_day != Some(item.day) {
                        last_day = Some(item.day);
                        ui.add_space(6.0);
                        ui.label(egui::RichText::new(day_label(item.day, today)).size(12.5).strong().color(pal.text_faint));
                        ui.add_space(2.0);
                    }
                    let doc = Doc::Feed(item.clone());
                    let mut ex = expanded.contains(&item.id);
                    let before = ex;
                    widgets::doc_card(ui, &env, &doc, &mut ex, &mut acts);
                    if ex != before {
                        if ex {
                            expanded.insert(item.id.clone());
                        } else {
                            expanded.remove(&item.id);
                        }
                    }
                    ui.add_space(8.0);
                }
                if !empty {
                    ui.add_space(8.0);
                    ui.vertical_centered(|ui| {
                        if loading_more {
                            ui.spinner();
                        } else if more_exhausted {
                            ui.label(egui::RichText::new("You're all caught up.").color(pal.text_faint));
                        }
                    });
                    if !has_provider && feed == FeedKind::Research {
                        ui.add_space(6.0);
                        ui.vertical_centered(|ui| {
                            ui.label(egui::RichText::new("Set up an AI model to also get AI-written articles in Discover.").size(12.0).color(pal.text_faint));
                        });
                    }
                    ui.add_space(24.0);
                }
            });
        });
        self.discover.expanded = expanded;
        // Infinite scroll: near the bottom → load more (older days, then older papers).
        let near_bottom = out.state.clamped_offset().y + out.inner_rect.height() >= out.content_size.y - 600.0;
        if near_bottom && !empty && !loading_more && !more_exhausted {
            acts.push(Action::LoadMoreFeed);
        }
        self.actions.extend(acts);
    }

    fn topic_tabs(&mut self, ui: &mut Ui, acts: &mut Vec<Action>) {
        let pal = self.pal;
        let tabs = self.settings.tabs.clone();
        if tabs.is_empty() {
            self.settings.tabs.push("For You".into());
            return;
        }
        let font = theme::regular(14.0);
        let natural: Vec<f32> = tabs.iter().map(|t| ui.painter().layout_no_wrap(t.clone(), font.clone(), pal.text).size().x + 28.0).collect();
        let plus_w = 34.0;
        let overflow_w = 34.0;
        let available = ui.available_width() - plus_w - 8.0;
        let sel = self.discover.tab.min(tabs.len() - 1);
        // craft-ui keeps every shown tab at its natural width and sends the rest to an overflow menu.
        let fit = craft_ui::tabs::fit_whole(&natural, sel, available, overflow_w);
        let (strip, _) = ui.allocate_exact_size(vec2(ui.available_width(), 36.0), Sense::hover());
        ui.painter().line_segment([strip.left_bottom(), strip.right_bottom()], Stroke::new(1.0, pal.border));
        let mut x = strip.left();
        let mut rects = vec![];
        for (idx, w) in &fit.shown {
            let rect = Rect::from_min_size(egui::pos2(x, strip.top()), vec2(*w, strip.height()));
            x += w;
            rects.push((*idx, rect));
        }
        let mut drop_target: Option<usize> = None;
        for (idx, rect) in &rects {
            let idx = *idx;
            let label = &tabs[idx];
            let selected = idx == sel;
            let r = craft_ui::tabs::Tab::new(egui::Id::unique(("topic_tab", label)), label, selected)
                .sense(Sense::click_and_drag())
                .focus_stroke(Stroke::new(1.0, pal.accent))
                .show_at(ui, *rect, |ui, resp| {
                    let color = if selected { pal.text } else if resp.hovered() { pal.text } else { pal.text_weak };
                    ui.painter().text(resp.rect.center(), egui::Align2::CENTER_CENTER, label, font.clone(), color);
                    if selected {
                        let r = resp.rect;
                        ui.painter().line_segment([egui::pos2(r.left() + 10.0, r.bottom() - 1.0), egui::pos2(r.right() - 10.0, r.bottom() - 1.0)], Stroke::new(2.5, pal.accent));
                    }
                });
            if r.clicked() {
                self.discover.tab = idx;
                self.discover.needs_reload = true;
            }
            // Drag a tab onto another to reorder.
            if r.drag_started() {
                self.discover.dragging_tab = Some(idx);
            }
            if self.discover.dragging_tab.is_some() && ui.input(|i| i.pointer.hover_pos()).is_some_and(|p| rect.contains(p)) {
                drop_target = Some(idx);
                if self.discover.dragging_tab != Some(idx) {
                    ui.painter().line_segment([rect.left_top(), rect.left_bottom()], Stroke::new(2.0, pal.accent));
                }
            }
            r.context_menu(|ui| {
                if ui.button("Rename…").clicked() {
                    self.discover.renaming_tab = Some((idx, label.clone()));
                }
                if ui.add_enabled(idx > 0, egui::Button::new("Move left")).clicked() {
                    self.settings.tabs.swap(idx, idx - 1);
                    if sel == idx {
                        self.discover.tab = idx - 1;
                    }
                    acts.push(Action::SaveSettings);
                }
                if ui.add_enabled(idx + 1 < tabs.len(), egui::Button::new("Move right")).clicked() {
                    self.settings.tabs.swap(idx, idx + 1);
                    if sel == idx {
                        self.discover.tab = idx + 1;
                    }
                    acts.push(Action::SaveSettings);
                }
                if ui.button("Follow this topic").on_hover_text("Get notified about new research on it").clicked() {
                    let f = Follow::Topic(label.clone());
                    if !self.settings.follows.contains(&f) {
                        self.settings.follows.push(f);
                        acts.push(Action::SaveSettings);
                        acts.push(Action::Toast(format!("Following “{label}”"), crate::app::ToastKind::Success));
                    }
                }
                ui.separator();
                if ui.add_enabled(tabs.len() > 1, egui::Button::new("Remove tab")).clicked() {
                    self.settings.tabs.remove(idx);
                    self.discover.tab = self.discover.tab.min(self.settings.tabs.len().saturating_sub(1));
                    self.discover.needs_reload = true;
                    acts.push(Action::SaveSettings);
                }
                if ui.button("Add tab…").clicked() {
                    self.discover.new_tab = Some(String::new());
                }
            });
        }
        if let Some(from) = self.discover.dragging_tab
            && !ui.input(|i| i.pointer.any_down())
        {
            self.discover.dragging_tab = None;
            if let Some(to) = drop_target
                && to != from
            {
                let t = self.settings.tabs.remove(from);
                self.settings.tabs.insert(to, t);
                self.discover.tab = to;
                self.discover.needs_reload = true;
                acts.push(Action::SaveSettings);
            }
        }
        // Overflow menu for tabs that don't fit.
        if !fit.overflow.is_empty() {
            let r = Rect::from_min_size(egui::pos2(x, strip.top() + 3.0), vec2(overflow_w, 30.0));
            let resp = ui.interact(r, egui::Id::unique("tabs_overflow"), Sense::click()).on_hover_text("More tabs");
            icons::icon("chevron-down", 16.0, pal.text_weak).paint_at(ui, Rect::from_center_size(r.center(), Vec2::splat(16.0)));
            egui::Popup::menu(&resp).show(|ui| {
                for idx in &fit.overflow {
                    if ui.button(&tabs[*idx]).clicked() {
                        self.discover.tab = *idx;
                        self.discover.needs_reload = true;
                    }
                }
            });
            x += overflow_w;
        }
        // Add tab.
        let r = Rect::from_min_size(egui::pos2(x + 4.0, strip.top() + 3.0), vec2(plus_w - 4.0, 30.0));
        let resp = ui.interact(r, egui::Id::unique("tabs_add"), Sense::click()).on_hover_text("Add a topic tab");
        if resp.hovered() {
            ui.painter().rect_filled(r, 7.0, pal.card_hover);
        }
        icons::icon("plus", 16.0, pal.text_weak).paint_at(ui, Rect::from_center_size(r.center(), Vec2::splat(16.0)));
        if resp.clicked() {
            self.discover.new_tab = Some(String::new());
        }
        // Inline editors for new / renamed tabs.
        if let Some(mut name) = self.discover.new_tab.take() {
            let mut keep = true;
            ui.horizontal(|ui| {
                let r = ui.add(egui::TextEdit::singleline(&mut name).hint_text("New topic, e.g. Neuroscience").desired_width(240.0));
                r.request_focus();
                let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if (ui.button("Add").clicked() || enter) && !name.trim().is_empty() {
                    self.settings.tabs.push(name.trim().to_string());
                    self.discover.tab = self.settings.tabs.len() - 1;
                    self.discover.needs_reload = true;
                    acts.push(Action::SaveSettings);
                    acts.push(Action::RunDaily(false));
                    keep = false;
                }
                if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    keep = false;
                }
            });
            if keep {
                self.discover.new_tab = Some(name);
            }
        }
        if let Some((idx, mut name)) = self.discover.renaming_tab.take() {
            let mut keep = true;
            ui.horizontal(|ui| {
                let r = ui.add(egui::TextEdit::singleline(&mut name).desired_width(240.0));
                r.request_focus();
                let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if (ui.button("Rename").clicked() || enter) && !name.trim().is_empty() {
                    if let Some(t) = self.settings.tabs.get_mut(idx) {
                        *t = name.trim().to_string();
                    }
                    self.discover.needs_reload = true;
                    acts.push(Action::SaveSettings);
                    keep = false;
                }
                if ui.button("Cancel").clicked() {
                    keep = false;
                }
            });
            if keep {
                self.discover.renaming_tab = Some((idx, name));
            }
        }
    }
}

pub fn day_label(day: chrono::NaiveDate, today: chrono::NaiveDate) -> String {
    let diff = (today - day).num_days();
    match diff {
        0 => "TODAY".into(),
        1 => "YESTERDAY".into(),
        2..=6 => day.format("%A").to_string().to_uppercase(),
        _ => day.format("%e %B %Y").to_string().trim().to_uppercase(),
    }
}
