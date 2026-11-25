//! Search: Scholar mode (a list of matching papers, like Google Scholar) and AI mode (the AI
//! curates the newest and most relevant research at the top, then the normal list below).
//! Journals and news are searched separately (scope selector).

use egui::{Align, Color32, Layout, Ui, vec2};

use super::icons;
use super::theme;
use super::widgets::{self, icon_button};
use crate::app::{App, SearchMode};
use crate::model::*;

impl App {
    pub fn search_view(&mut self, ui: &mut Ui) {
        let pal = self.pal;
        let data = self.card_data();
        let mut acts = vec![];
        let mut rerun = false;
        ui.add_space(8.0);
        widgets::column(ui, 980.0, |ui| {
            // Big search box (mirrors the title-bar one) for an obvious place to start.
            ui.horizontal(|ui| {
                let r = ui.add(
                    egui::TextEdit::singleline(&mut self.search.input)
                        .hint_text("What do you want to research?")
                        .font(theme::regular(17.0))
                        .desired_width(ui.available_width() - 110.0)
                        .margin(vec2(12.0, 8.0))
                        .id_salt("big_search"),
                );
                if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    rerun = true;
                }
                if ui.add(egui::Button::image_and_text(icons::icon("search", 16.0, Color32::WHITE), egui::RichText::new("Search").color(Color32::WHITE)).fill(pal.accent).min_size(vec2(96.0, 36.0))).clicked() {
                    rerun = true;
                }
            });
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                // Mode.
                for (mode, label, tip) in [
                    (SearchMode::Scholar, "Scholar mode", "A list of matching papers, filtered and sorted as you choose"),
                    (SearchMode::Ai, "AI mode", "The AI chooses filters, curates the newest and most relevant research at the top, then the normal list follows"),
                ] {
                    let sel = self.search.mode == mode;
                    let icon = if mode == SearchMode::Ai { "sparkle" } else { "books" };
                    let tint = if sel { if mode == SearchMode::Ai { pal.ai } else { pal.accent } } else { pal.text_weak };
                    if ui
                        .add(egui::Button::image_and_text(icons::icon(icon, 15.0, tint), egui::RichText::new(label).color(if sel { pal.text } else { pal.text_weak })).fill(if sel { pal.accent_soft } else { Color32::TRANSPARENT }).corner_radius(14))
                        .on_hover_text(tip)
                        .clicked()
                        && !sel
                    {
                        self.search.mode = mode;
                        rerun = !self.search.input.trim().is_empty();
                    }
                }
                ui.separator();
                // Scope: journals or news.
                ui.label(egui::RichText::new("Search in").color(pal.text_weak));
                for (scope, label) in [(SearchScope::Journals, "Journals"), (SearchScope::News, "News")] {
                    if ui.selectable_label(self.search.query.scope == scope, label).clicked() && self.search.query.scope != scope {
                        self.search.query.scope = scope;
                        rerun = !self.search.input.trim().is_empty();
                    }
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if self.search.mode == SearchMode::Scholar && icon_button(ui, &pal, "filter", "Filters", self.search.show_filters).clicked() {
                        self.search.show_filters = !self.search.show_filters;
                    }
                });
            });
            if self.search.mode == SearchMode::Scholar && self.search.show_filters && self.filters_row(ui) {
                rerun = !self.search.input.trim().is_empty();
            }
            if self.search.mode == SearchMode::Ai {
                if let Some(plan) = &self.search.plan {
                    ui.add_space(4.0);
                    ui.horizontal_wrapped(|ui| {
                        ui.add(icons::icon("sparkle", 14.0, pal.ai));
                        let text = if plan.explanation.is_empty() { format!("Searching: {}", plan.queries.join(" · ")) } else { plan.explanation.clone() };
                        ui.label(egui::RichText::new(text).size(12.5).color(pal.ai));
                    });
                }
            }
            ui.add_space(6.0);
        });

        if rerun {
            self.run_search(false);
        }

        let mut expanded = std::mem::take(&mut self.search.expanded);
        let env = self.card_env(&data);
        let results = self.search.results.clone();
        let loading = self.search.loading;
        let exhausted = self.search.exhausted;
        let error = self.search.error.clone();
        let curated = self.search.curated.clone();
        let curating = self.search.curating;
        let mode = self.search.mode;
        let has_query = !self.search.query.text.is_empty();
        let out = egui::ScrollArea::vertical().auto_shrink([false, false]).id_salt(("search", self.search.id)).show(ui, |ui| {
            widgets::column(ui, 980.0, |ui| {
                if !has_query {
                    widgets::empty_state(ui, &pal, "search", "Search the research", "Type a topic, question, title, author or DOI. Scholar mode lists papers; AI mode curates the newest and most relevant first.");
                    return;
                }
                if let Some(e) = &error {
                    ui.colored_label(pal.danger, format!("Search failed: {e}"));
                }
                // AI mode: curated block at the top.
                if mode == SearchMode::Ai {
                    if curating {
                        egui::Frame::new().fill(pal.ai_soft).corner_radius(12).inner_margin(14).show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal(|ui| {
                                ui.spinner();
                                ui.label(egui::RichText::new("Curating the newest and most relevant research…").color(pal.ai));
                            });
                        });
                        ui.add_space(10.0);
                    } else if let Some(c) = &curated {
                        egui::Frame::new().fill(pal.ai_soft).stroke(egui::Stroke::new(1.0, pal.ai.gamma_multiply(0.4))).corner_radius(12).inner_margin(14).show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal(|ui| {
                                ui.add(icons::icon("sparkle", 16.0, pal.ai));
                                ui.label(egui::RichText::new("AI overview").font(theme::bold(15.0)).color(pal.ai));
                                widgets::badge(ui, "AI-written", pal.ai);
                            });
                            ui.add_space(4.0);
                            if !c.overview.is_empty() {
                                widgets::paragraph(ui, &pal, &c.overview, &c.sources, 14.5, &mut acts);
                            }
                        });
                        ui.add_space(10.0);
                        let top: Vec<&Paper> = c.top_keys.iter().filter_map(|k| results.iter().find(|p| &p.key == k)).collect();
                        if !top.is_empty() {
                            ui.label(egui::RichText::new("CURATED: NEWEST AND MOST RELEVANT").size(12.0).strong().color(pal.text_faint));
                            ui.add_space(4.0);
                            for p in top {
                                let doc = Doc::Paper(p.clone());
                                let mut ex = expanded.contains(&p.key);
                                let before = ex;
                                widgets::doc_card(ui, &env, &doc, &mut ex, &mut acts);
                                toggle(&mut expanded, &p.key, before, ex);
                                ui.add_space(8.0);
                            }
                            ui.add_space(6.0);
                            ui.label(egui::RichText::new("ALL RESULTS").size(12.0).strong().color(pal.text_faint));
                            ui.add_space(4.0);
                        }
                    }
                }
                if results.is_empty() && !loading && error.is_none() {
                    widgets::empty_state(ui, &pal, "search", "No results", "Try different words, widen the date range or switch off filters.");
                }
                for p in &results {
                    let doc = Doc::Paper(p.clone());
                    let mut ex = expanded.contains(&p.key);
                    let before = ex;
                    widgets::doc_card(ui, &env, &doc, &mut ex, &mut acts);
                    toggle(&mut expanded, &p.key, before, ex);
                    ui.add_space(8.0);
                }
                ui.vertical_centered(|ui| {
                    if loading {
                        ui.add_space(10.0);
                        ui.spinner();
                    } else if exhausted && !results.is_empty() {
                        ui.label(egui::RichText::new("End of results").color(pal.text_faint));
                    }
                });
                ui.add_space(24.0);
            });
        });
        self.search.expanded = expanded;
        let near_bottom = out.state.clamped_offset().y + out.inner_rect.height() >= out.content_size.y - 500.0;
        if near_bottom && !self.search.results.is_empty() && !self.search.loading && !self.search.exhausted && self.search.mode == SearchMode::Scholar {
            self.run_search(true);
        }
        self.actions.extend(acts);
    }

    /// Filters: date range, free full text only, content type, subject, sort. Returns true if changed.
    fn filters_row(&mut self, ui: &mut Ui) -> bool {
        let pal = self.pal;
        let mut changed = false;
        egui::Frame::new().fill(pal.card).stroke(egui::Stroke::new(1.0, pal.border)).corner_radius(10).inner_margin(10).show(ui, |ui| {
            ui.set_width(ui.available_width());
            let f = &mut self.search.query.filters;
            ui.horizontal_wrapped(|ui| {
                ui.label("From");
                let mut from = f.from_year.unwrap_or(0);
                if ui.add(egui::DragValue::new(&mut from).range(0..=2100).custom_formatter(|v, _| if v == 0.0 { "any".into() } else { format!("{v}") })).changed() {
                    f.from_year = (from > 0).then_some(from);
                    changed = true;
                }
                ui.label("to");
                let mut to = f.to_year.unwrap_or(0);
                if ui.add(egui::DragValue::new(&mut to).range(0..=2100).custom_formatter(|v, _| if v == 0.0 { "any".into() } else { format!("{v}") })).changed() {
                    f.to_year = (to > 0).then_some(to);
                    changed = true;
                }
                ui.separator();
                changed |= ui.checkbox(&mut f.free_full_text_only, "Free full text only").changed();
                ui.separator();
                ui.label("Sort");
                egui::ComboBox::from_id_salt("sort")
                    .selected_text(match f.sort {
                        SortBy::Relevance => "Relevance",
                        SortBy::Newest => "Newest",
                        SortBy::MostCited => "Most cited",
                    })
                    .show_ui(ui, |ui| {
                        changed |= ui.selectable_value(&mut f.sort, SortBy::Relevance, "Relevance").changed();
                        changed |= ui.selectable_value(&mut f.sort, SortBy::Newest, "Newest").changed();
                        changed |= ui.selectable_value(&mut f.sort, SortBy::MostCited, "Most cited").changed();
                    });
            });
            ui.horizontal_wrapped(|ui| {
                ui.label("Type");
                for k in [ContentKind::FreeFullText, ContentKind::AbstractOnly, ContentKind::AiArticle, ContentKind::News] {
                    let mut on = f.kinds.contains(&k);
                    let label = match k {
                        ContentKind::FreeFullText => "Real – free full text",
                        ContentKind::AbstractOnly => "Real – abstract only",
                        ContentKind::AiArticle => "AI article",
                        ContentKind::News => "News",
                    };
                    if ui.checkbox(&mut on, label).changed() {
                        if on {
                            f.kinds.push(k);
                        } else {
                            f.kinds.retain(|x| *x != k);
                        }
                        changed = true;
                    }
                }
                ui.separator();
                ui.label("Subject");
                let mut subject = f.subject.clone().unwrap_or_default();
                let r = ui.add(egui::TextEdit::singleline(&mut subject).hint_text("any").desired_width(140.0));
                if r.changed() {
                    f.subject = (!subject.trim().is_empty()).then(|| subject.trim().to_string());
                }
                if r.lost_focus() {
                    changed = true;
                }
                if ui.small_button("Reset").clicked() {
                    *f = SearchFilters::default();
                    changed = true;
                }
            });
        });
        changed
    }
}

fn toggle(set: &mut std::collections::HashSet<String>, key: &str, before: bool, after: bool) {
    if before != after {
        if after {
            set.insert(key.to_string());
        } else {
            set.remove(key);
        }
    }
}
