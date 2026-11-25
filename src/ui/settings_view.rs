//! Settings: appearance, models, research sources, Discover, storage, citations, about.

use egui::{Align, Color32, Layout, RichText, Stroke, Ui, vec2};

use super::icons;
use super::theme;
use super::widgets;
use crate::app::{App, Modal};
use crate::model::CitationStyle;
use crate::providers::{ProviderConfig, ProviderKind, Task, secrets};
use crate::settings::ThemePref;

const SECTIONS: [(&str, &str); 7] = [
    ("Appearance", "sun"),
    ("Models", "model"),
    ("Research & sources", "flask"),
    ("Discover", "compass"),
    ("Storage", "download"),
    ("Citations", "quote"),
    ("About", "logo"),
];

impl App {
    pub fn settings_view(&mut self, ui: &mut Ui) {
        let pal = self.pal;
        ui.add_space(8.0);
        widgets::column(ui, 1400.0, |ui| {
            widgets::heading(ui, &pal, "Settings", None);
            // Phones: the sections as a row of chips above the settings.
            if self.mobile {
                ui.horizontal_wrapped(|ui| {
                    for (i, (name, _)) in SECTIONS.iter().enumerate() {
                        if ui.selectable_label(self.settings_ui.section == i, *name).clicked() {
                            self.settings_ui.section = i;
                            if i == 4 {
                                self.settings_ui.usage = Some(self.db.usage());
                            }
                        }
                    }
                });
                ui.add_space(6.0);
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    match self.settings_ui.section {
                        0 => self.settings_appearance(ui),
                        1 => self.settings_models(ui),
                        2 => self.settings_research(ui),
                        3 => self.settings_discover(ui),
                        4 => self.settings_storage(ui),
                        5 => self.settings_citations(ui),
                        _ => self.settings_about(ui),
                    }
                    ui.add_space(20.0);
                    widgets::keep_focused_visible(ui);
                });
                return;
            }
            // Two full-height panes: section list on the left, the section's settings filling the rest.
            let area = ui.available_rect_before_wrap();
            let nav = egui::Rect::from_min_max(area.min, egui::pos2(area.left() + 196.0, area.bottom()));
            let body = egui::Rect::from_min_max(egui::pos2(area.left() + 212.0, area.top()), area.max);
            ui.scope_builder(egui::UiBuilder::new().max_rect(nav), |ui| {
                for (i, (name, icon)) in SECTIONS.iter().enumerate() {
                    let sel = self.settings_ui.section == i;
                    if ui
                        .add(egui::Button::image_and_text(icons::icon(icon, 16.0, if sel { pal.accent } else { pal.text_weak }), RichText::new(*name).color(if sel { pal.text } else { pal.text_weak })).fill(if sel { pal.accent_soft } else { Color32::TRANSPARENT }).min_size(vec2(190.0, 30.0)))
                        .clicked()
                    {
                        self.settings_ui.section = i;
                        if i == 4 {
                            self.settings_ui.usage = Some(self.db.usage());
                        }
                    }
                }
            });
            ui.scope_builder(egui::UiBuilder::new().max_rect(body), |ui| {
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    ui.set_width(ui.available_width() - 12.0);
                    match self.settings_ui.section {
                        0 => self.settings_appearance(ui),
                        1 => self.settings_models(ui),
                        2 => self.settings_research(ui),
                        3 => self.settings_discover(ui),
                        4 => self.settings_storage(ui),
                        5 => self.settings_citations(ui),
                        _ => self.settings_about(ui),
                    }
                    ui.add_space(20.0);
                    widgets::keep_focused_visible(ui);
                });
            });
        });
    }

    fn group(&self, ui: &mut Ui, title: &str, add: impl FnOnce(&mut Ui)) {
        let pal = self.pal;
        egui::Frame::new().fill(pal.card).stroke(Stroke::new(1.0, pal.border)).corner_radius(10).inner_margin(14).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(title).font(theme::bold(15.0)));
            ui.add_space(6.0);
            add(ui);
        });
        ui.add_space(10.0);
    }

    fn settings_appearance(&mut self, ui: &mut Ui) {
        let pal = self.pal;
        let mut theme_change = None;
        let mut save = false;
        let ctx = ui.ctx().clone();
        let s = &mut self.settings;
        egui::Frame::new().fill(pal.card).stroke(Stroke::new(1.0, pal.border)).corner_radius(10).inner_margin(14).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new("Theme").font(theme::bold(15.0)));
            ui.horizontal(|ui| {
                if ui.selectable_label(s.theme == ThemePref::Dark, "Dark").clicked() {
                    theme_change = Some(ThemePref::Dark);
                }
                if ui.selectable_label(s.theme == ThemePref::Light, "Light").clicked() {
                    theme_change = Some(ThemePref::Light);
                }
            });
            ui.add_space(8.0);
            ui.label(RichText::new("Interface size").font(theme::bold(15.0)));
            let r = ui.add(egui::Slider::new(&mut s.ui_scale, 0.75..=1.6).step_by(0.05).show_value(true));
            if r.drag_stopped() || r.lost_focus() {
                save = true;
                ctx.set_zoom_factor(s.ui_scale);
            }
            ui.add_space(8.0);
            if crate::platform::is_android() {
                return;
            }
            ui.label(RichText::new("Window").font(theme::bold(15.0)));
            if ui.checkbox(&mut s.system_title_bar, "Use the desktop's title bar (takes effect after restart)").changed() {
                save = true;
            }
            if ui.checkbox(&mut s.sidebar_open, "Show the sidebar (Ctrl+B)").changed() {
                save = true;
            }
        });
        if let Some(t) = theme_change {
            self.set_theme(&ctx, t);
        }
        if save {
            self.settings_changed();
        }
        ui.add_space(10.0);
        if !crate::platform::is_android() && ui.button("Keyboard shortcuts…").clicked() {
            self.modal = Some(Modal::Shortcuts);
        }
    }

    fn settings_models(&mut self, ui: &mut Ui) {
        let pal = self.pal;
        let mut changed = false;
        ui.label(RichText::new("Gateless Scholar finds sources itself; the model only writes from the sources it's given. Pick a different model per task — e.g. a small local model for headlines and a strong one for full articles.").color(pal.text_weak));
        ui.add_space(8.0);

        // Detected on this machine.
        let detected = self.detected.clone();
        let android = crate::platform::is_android();
        self.group(ui, if android { "Found on this device" } else { "Found on this computer" }, |ui| match &detected {
            None => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Looking for AI CLIs and Ollama…");
                });
            }
            Some(d) => {
                for kind in [ProviderKind::ClaudeCli, ProviderKind::GeminiCli, ProviderKind::CodexCli].into_iter().filter(|k| k.available_here()) {
                    let found = d.clis.iter().find(|(k, _)| *k == kind);
                    ui.horizontal(|ui| {
                        ui.add(icons::icon(if found.is_some() { "check" } else { "x" }, 15.0, if found.is_some() { pal.link } else { pal.text_faint }));
                        ui.label(kind.label());
                        match found {
                            Some((_, path)) => {
                                ui.label(RichText::new(path).size(12.0).color(pal.text_faint));
                            }
                            None => {
                                ui.label(RichText::new("not installed").size(12.0).color(pal.text_faint));
                            }
                        }
                    });
                }
                ui.horizontal(|ui| {
                    ui.add(icons::icon(if d.ollama_running { "check" } else { "x" }, 15.0, if d.ollama_running { pal.link } else { pal.text_faint }));
                    ui.label("Ollama");
                    ui.label(RichText::new(if d.ollama_running { format!("running · {} models", d.ollama_models.len()) } else { "not running at localhost:11434".into() }).size(12.0).color(pal.text_faint));
                });
            }
        });
        ui.horizontal(|ui| {
            if ui.button("Detect again").clicked() {
                self.detected = None;
                crate::jobs::detect_providers(self);
            }
            if let Some(d) = &detected {
                let missing: Vec<ProviderConfig> = d
                    .clis
                    .iter()
                    .map(|(k, _)| *k)
                    .chain(d.ollama_running.then_some(ProviderKind::Ollama))
                    .filter(|k| !self.settings.providers.iter().any(|p| p.kind == *k))
                    .map(|k| {
                        let mut c = ProviderConfig::new(k);
                        if k == ProviderKind::Ollama
                            && let Some(m) = d.ollama_models.first()
                        {
                            c.model = m.clone();
                        }
                        c
                    })
                    .collect();
                if !missing.is_empty() && ui.button(format!("Add {} detected", missing.len())).clicked() {
                    self.settings.providers.extend(missing);
                    changed = true;
                }
            }
        });
        ui.add_space(10.0);

        // Configured providers.
        ui.label(RichText::new("Your models").font(theme::bold(15.0)));
        ui.add_space(4.0);
        let mut remove = None;
        let mut test = None;
        let mut list_ollama = None;
        let mut find_relay = None;
        let ollama_models = self.settings_ui.ollama_models.clone();
        let relay_models = self.settings_ui.relay_models.clone();
        for (i, p) in self.settings.providers.iter_mut().enumerate() {
            egui::Frame::new().fill(pal.card).stroke(Stroke::new(1.0, pal.border)).corner_radius(10).inner_margin(12).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal_wrapped(|ui| {
                    changed |= ui.checkbox(&mut p.enabled, "").on_hover_text("Enabled").changed();
                    changed |= ui.add(egui::TextEdit::singleline(&mut p.label).desired_width(ui.available_width().min(200.0)).font(theme::bold(14.5))).lost_focus();
                    ui.label(RichText::new(p.kind.label()).size(12.0).color(pal.text_faint));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.button("Remove").clicked() {
                            remove = Some(i);
                        }
                        let testing = self.settings_ui.testing.contains(&p.id);
                        if ui.add_enabled(!testing, egui::Button::new(if testing { "Testing…" } else { "Test" })).clicked() {
                            test = Some(p.clone());
                        }
                    });
                });
                let mobile = self.mobile;
                let pid = p.id.clone();
                // Phones: label above each field (one column); desktop: a two-column grid.
                let body = |ui: &mut Ui| {
                    ui.label("Model");
                    ui.horizontal(|ui| {
                        if p.kind == ProviderKind::Relay {
                            // Which of the PC's models to use ("" = the PC's default for articles).
                            let info = relay_models.get(&p.id);
                            let current = if p.model.is_empty() { "PC's default model".to_string() } else { p.label.clone() };
                            egui::ComboBox::from_id_salt(("relay_model", &p.id)).selected_text(current).width(ui.available_width().min(240.0)).show_ui(ui, |ui| {
                                if ui.selectable_label(p.model.is_empty(), "PC's default model").clicked() {
                                    p.model.clear();
                                    p.remote_brand = info.and_then(|i| i.default_id.as_ref().and_then(|d| i.models.iter().find(|m| &m.id == d))).map(|m| m.brand.clone());
                                    changed = true;
                                }
                                for m in info.map(|i| i.models.as_slice()).unwrap_or(&[]) {
                                    if ui.selectable_label(p.model == m.id, &m.describe).clicked() {
                                        p.model = m.id.clone();
                                        p.label = format!("{} (my PC)", m.label);
                                        p.remote_brand = Some(m.brand.clone());
                                        changed = true;
                                    }
                                }
                            });
                            let busy = self.settings_ui.testing.contains(&p.id);
                            if ui.add_enabled(!busy, egui::Button::new(if busy { "Connecting…" } else { "Find models on my PC" }).small()).clicked() {
                                find_relay = Some(p.clone());
                            }
                        } else if p.kind == ProviderKind::Ollama && !ollama_models.is_empty() {
                            egui::ComboBox::from_id_salt(("ollama_model", &p.id)).selected_text(&p.model).width(ui.available_width().min(240.0)).show_ui(ui, |ui| {
                                for m in &ollama_models {
                                    changed |= ui.selectable_value(&mut p.model, m.clone(), m).changed();
                                }
                            });
                        } else {
                            let hint = if p.kind.is_cli() { "CLI default" } else { p.kind.default_model() };
                            changed |= ui.add(egui::TextEdit::singleline(&mut p.model).hint_text(hint).desired_width(ui.available_width().min(240.0))).lost_focus();
                        }
                        if p.kind == ProviderKind::Ollama && ui.small_button("Refresh list").clicked() {
                            list_ollama = Some(p.base_url.clone().unwrap_or_else(|| "http://localhost:11434".into()));
                        }
                    });
                    ui.end_row();
                    if !p.kind.is_cli() {
                        ui.label(if p.kind == ProviderKind::Relay { "PC address" } else { "Address" });
                        let mut url = p.base_url.clone().unwrap_or_default();
                        let hint = match p.kind {
                            ProviderKind::Relay => "shown on your PC: Settings › Models › Use this PC from my phone",
                            ProviderKind::Ollama if android => "http://<your PC>:11434, or http://localhost:11434 for Ollama on this phone",
                            _ => "",
                        };
                        // Keep what's typed (the field is rebuilt from the setting every frame).
                        let r = ui.add(egui::TextEdit::singleline(&mut url).hint_text(hint).desired_width(ui.available_width().min(320.0)));
                        if r.changed() {
                            p.base_url = (!url.trim().is_empty()).then(|| url.trim().to_string());
                        }
                        if r.lost_focus() {
                            changed = true;
                        }
                        ui.end_row();
                    }
                    if p.kind.needs_api_key() {
                        ui.label(if p.kind == ProviderKind::Relay { "Pairing code" } else { "API key" });
                        ui.horizontal(|ui| {
                            let has = secrets::get_api_key(&p.id).ok().flatten().is_some();
                            let input = self.settings_ui.api_key_inputs.entry(p.id.clone()).or_default();
                            let hint = match (has, android) {
                                (true, true) => "saved on this device",
                                (true, false) => "saved in the system keyring",
                                (false, _) if p.kind == ProviderKind::Relay => "e.g. abcd-efgh-jkmn-pqrs",
                                _ => "paste key",
                            };
                            ui.add(egui::TextEdit::singleline(input).password(p.kind != ProviderKind::Relay).hint_text(hint).desired_width(ui.available_width().min(240.0)));
                            if ui.button("Save").clicked() && !input.trim().is_empty() {
                                match secrets::set_api_key(&p.id, input.trim()) {
                                    Ok(()) => {
                                        input.clear();
                                        changed = true;
                                    }
                                    Err(e) => log::warn!("keyring: {e}"),
                                }
                            }
                            if has && ui.button("Forget").clicked() {
                                let _ = secrets::delete_api_key(&p.id);
                                changed = true;
                            }
                        });
                        ui.end_row();
                    }
                    ui.label("Writing style");
                    changed |= ui.checkbox(&mut p.small_model, "Small model: write section by section with fewer sources each").changed();
                    ui.end_row();
                };
                if mobile {
                    ui.vertical(body);
                } else {
                    egui::Grid::new(("prov", &pid)).num_columns(2).spacing([12.0, 6.0]).show(ui, body);
                }
                if let Some(r) = self.settings_ui.tests.get(&p.id) {
                    match r {
                        Ok(t) => ui.colored_label(pal.link, format!("OK — {t}")),
                        Err(e) => ui.colored_label(pal.danger, format!("Problem: {e}")),
                    };
                }
            });
            ui.add_space(6.0);
        }
        if let Some(i) = remove {
            let p = self.settings.providers.remove(i);
            let _ = secrets::delete_api_key(&p.id);
            self.settings.task_providers.retain(|_, id| *id != p.id);
            changed = true;
        }
        if let Some(cfg) = test {
            if changed {
                self.settings_changed();
                changed = false;
            }
            self.settings_ui.testing.insert(cfg.id.clone());
            self.settings_ui.tests.remove(&cfg.id);
            crate::jobs::test_provider(self, cfg);
        }
        if let Some(url) = list_ollama {
            crate::jobs::list_ollama(self, url);
        }
        if let Some(cfg) = find_relay {
            if changed {
                self.settings_changed();
                changed = false;
            }
            self.settings_ui.testing.insert(cfg.id.clone());
            crate::jobs::relay_info(self, cfg);
        }
        ui.horizontal(|ui| {
            ui.menu_button("Add a model…", |ui| {
                for k in ProviderKind::ALL.into_iter().filter(|k| k.available_here()) {
                    let tip = match k {
                        ProviderKind::Relay => "Use the models set up in Gateless Scholar on your computer — e.g. Claude Code with your subscription",
                        ProviderKind::Gemini => "Google's Gemini API — has a free tier",
                        ProviderKind::Anthropic => "Claude via Anthropic's API (billed per use)",
                        ProviderKind::Ollama => "A local model server: on your PC, or on this device",
                        _ => "",
                    };
                    if ui.button(k.label()).on_hover_text(tip).clicked() {
                        let mut c = ProviderConfig::new(k);
                        if k == ProviderKind::Ollama
                            && let Some(m) = self.settings_ui.ollama_models.first()
                        {
                            c.model = m.clone();
                        }
                        if k == ProviderKind::Gemini {
                            c.label = "Gemini API (free tier works)".into();
                        }
                        self.settings.providers.push(c);
                        changed = true;
                    }
                }
            });
        });
        ui.add_space(12.0);

        // Task assignment.
        let providers = self.settings.providers.clone();
        let mut tasks = self.settings.task_providers.clone();
        self.group(ui, "Which model does what", |ui| {
            if providers.is_empty() {
                ui.label(RichText::new("Add a model above first.").color(pal.text_weak));
                return;
            }
            egui::Grid::new("tasks").num_columns(2).spacing([16.0, 6.0]).show(ui, |ui| {
                for t in Task::ALL {
                    ui.label(t.label());
                    let current = tasks.get(&t).and_then(|id| providers.iter().find(|p| &p.id == id)).map(|p| p.describe()).unwrap_or_else(|| "Default (first enabled)".into());
                    egui::ComboBox::from_id_salt(("task", t)).selected_text(current).width(ui.available_width().min(300.0)).show_ui(ui, |ui| {
                        if ui.selectable_label(!tasks.contains_key(&t), "Default (first enabled)").clicked() {
                            tasks.remove(&t);
                            changed = true;
                        }
                        for p in providers.iter().filter(|p| p.enabled) {
                            if ui.selectable_label(tasks.get(&t) == Some(&p.id), p.describe()).clicked() {
                                tasks.insert(t, p.id.clone());
                                changed = true;
                            }
                        }
                    });
                    ui.end_row();
                }
            });
        });
        self.settings.task_providers = tasks;
        if !android {
            changed |= self.relay_server_settings(ui);
        }
        let mut extra = self.settings.cli_extra_research;
        if !android && ui
            .checkbox(&mut extra, "Let AI CLIs (Claude Code, Gemini CLI, Codex) do extra web research themselves")
            .on_hover_text("They always receive the sources Gateless Scholar found; this also lets them search the web. They're never allowed to edit files or run commands.")
            .changed()
        {
            self.settings.cli_extra_research = extra;
            changed = true;
        }
        if changed {
            self.settings_changed();
        }
    }

    /// Desktop: let the phone app use this PC's models.
    fn relay_server_settings(&mut self, ui: &mut Ui) -> bool {
        let pal = self.pal;
        let mut changed = false;
        let mut enabled = self.settings.relay_enabled;
        let mut port = self.settings.relay_port;
        let mut new_code = false;
        let running = self.relay.as_ref().map(|r| (r.port, r.code.clone()));
        let error = self.relay_error.clone();
        let mut copy = None;
        self.group(ui, "Use this PC from my phone", |ui| {
            ui.label(RichText::new("Lets Gateless Scholar on your phone use the models set up here — including Claude Code with your own subscription — while this app is open. Use it on your own Wi-Fi, or over Tailscale from anywhere (Tailscale also encrypts the connection).").size(12.5).color(pal.text_weak));
            changed |= ui.checkbox(&mut enabled, "Let my phone use this PC's AI models").changed();
            if enabled {
                ui.horizontal(|ui| {
                    ui.label("Port");
                    changed |= ui.add(egui::DragValue::new(&mut port).range(1024..=65535)).lost_focus();
                });
                if let Some(e) = &error {
                    ui.colored_label(pal.danger, format!("Couldn't start: {e}"));
                }
                if let Some((port, code)) = &running {
                    ui.add_space(4.0);
                    ui.label(RichText::new("On your phone: Settings › Models › Add a model › Use my PC, then enter:").strong());
                    egui::Grid::new("relay_pair").num_columns(3).spacing([12.0, 6.0]).show(ui, |ui| {
                        for addr in crate::relay::local_addresses(*port) {
                            ui.label("PC address");
                            ui.label(RichText::new(&addr).monospace().color(pal.accent));
                            if ui.small_button("Copy").clicked() {
                                copy = Some(addr.split_whitespace().next().unwrap_or("").to_string());
                            }
                            ui.end_row();
                        }
                        ui.label("Pairing code");
                        ui.label(RichText::new(code).monospace().size(16.0).color(pal.accent));
                        ui.horizontal(|ui| {
                            if ui.small_button("Copy").clicked() {
                                copy = Some(code.clone());
                            }
                            if ui.small_button("New code").on_hover_text("Phones using the old code stop working until you enter the new one").clicked() {
                                new_code = true;
                            }
                        });
                        ui.end_row();
                    });
                    ui.label(RichText::new("Keep the pairing code private: anyone on your network with it can use your models.").size(12.0).color(pal.warn));
                }
            }
        });
        if let Some(c) = copy {
            self.actions.push(crate::app::Action::Copy(c));
        }
        if new_code {
            let _ = secrets::set_api_key("relay-pairing-code", &crate::relay::new_pairing_code());
            changed = true;
        }
        self.settings.relay_enabled = enabled;
        self.settings.relay_port = port;
        changed
    }

    fn settings_research(&mut self, ui: &mut Ui) {
        let pal = self.pal;
        let mut changed = false;
        ui.label(RichText::new("Sources: OpenAlex, Semantic Scholar, arXiv, Crossref, Unpaywall, Europe PMC, CORE (with a key), GDELT and outlet RSS feeds. Only legally available content is used — no logins are bypassed and no shadow libraries are used.").color(pal.text_weak));
        ui.add_space(8.0);
        let mut email = self.settings.contact_email.clone();
        self.group(ui, "Contact email for research APIs (optional)", |ui| {
            ui.label(RichText::new("OpenAlex, Crossref and Unpaywall ask apps to include a contact email (their “polite pool” is faster). It's only sent to those APIs, never anywhere else.").size(12.5).color(pal.text_weak));
            if ui.add(egui::TextEdit::singleline(&mut email).hint_text("you@example.org").desired_width(ui.available_width().min(320.0))).lost_focus() {
                changed = true;
            }
        });
        self.settings.contact_email = email;
        let pal2 = pal;
        let mut core_in = std::mem::take(&mut self.settings_ui.core_key_input);
        let mut s2_in = std::mem::take(&mut self.settings_ui.s2_key_input);
        let mut oa_in = std::mem::take(&mut self.settings_ui.openalex_key_input);
        self.group(ui, "API keys (optional, stored in the system keyring)", |ui| {
            ui.label(RichText::new("Without keys the free public limits apply: OpenAlex allows roughly 100 searches a day per network, and Semantic Scholar is often busy.").size(12.5).color(pal2.text_weak));
            for (label, id, input, url) in [
                ("OpenAlex", "openalex-api-key", &mut oa_in, "https://openalex.org/settings/api"),
                ("CORE", "core-api-key", &mut core_in, "https://core.ac.uk/services/api"),
                ("Semantic Scholar", "s2-api-key", &mut s2_in, "https://www.semanticscholar.org/product/api"),
            ] {
                ui.horizontal(|ui| {
                    ui.label(format!("{label}:"));
                    let has = secrets::get_api_key(id).ok().flatten().is_some();
                    ui.add(egui::TextEdit::singleline(input).password(true).hint_text(if has { "saved" } else { "paste key" }).desired_width(ui.available_width().min(240.0)));
                    if ui.button("Save").clicked() && !input.trim().is_empty() {
                        let _ = secrets::set_api_key(id, input.trim());
                        input.clear();
                        changed = true;
                    }
                    if has && ui.button("Forget").clicked() {
                        let _ = secrets::delete_api_key(id);
                        changed = true;
                    }
                    ui.hyperlink_to(RichText::new("get a key ↗").size(12.0).color(pal2.link), url);
                });
            }
        });
        self.settings_ui.core_key_input = core_in;
        self.settings_ui.s2_key_input = s2_in;
        self.settings_ui.openalex_key_input = oa_in;
        let mut feeds = self.settings.news_feeds.clone();
        let mut new_feed = std::mem::take(&mut self.settings_ui.new_feed);
        self.group(ui, "News feeds (used with GDELT)", |ui| {
            let mut remove = None;
            for (i, f) in feeds.iter().enumerate() {
                ui.horizontal(|ui| {
                    if ui.small_button("✕").clicked() {
                        remove = Some(i);
                    }
                    ui.label(RichText::new(f).size(12.5).color(pal.text_weak));
                });
            }
            if let Some(i) = remove {
                feeds.remove(i);
                changed = true;
            }
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut new_feed).hint_text("https://outlet.example/rss").desired_width(ui.available_width().min(340.0)));
                if ui.button("Add feed").clicked() && new_feed.starts_with("http") {
                    feeds.push(new_feed.trim().to_string());
                    new_feed.clear();
                    changed = true;
                }
                if ui.button("Restore defaults").clicked() {
                    feeds = crate::settings::default_news_feeds();
                    changed = true;
                }
            });
        });
        self.settings.news_feeds = feeds;
        self.settings_ui.new_feed = new_feed;
        if changed {
            self.settings_changed();
        }
    }

    fn settings_discover(&mut self, ui: &mut Ui) {
        let pal = self.pal;
        let mut changed = false;
        let s = &mut self.settings;
        egui::Frame::new().fill(pal.card).stroke(Stroke::new(1.0, pal.border)).corner_radius(10).inner_margin(14).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new("Daily Discover").font(theme::bold(15.0)));
            changed |= ui.checkbox(&mut s.auto_daily, "Gather today's headlines when the app opens each day").changed();
            changed |= ui.checkbox(&mut s.hide_ai_only, "Hide AI-only articles in Discover Research").changed();
            egui::Grid::new("disc").num_columns(2).spacing([16.0, 6.0]).show(ui, |ui| {
                ui.label("Real papers per tab per day");
                changed |= ui.add(egui::DragValue::new(&mut s.daily_research_per_tab).range(3..=40)).changed();
                ui.end_row();
                ui.label("AI articles per tab per day");
                changed |= ui.add(egui::DragValue::new(&mut s.daily_ai_articles_per_tab).range(0..=6)).changed();
                ui.end_row();
                ui.label("News stories per tab per day");
                changed |= ui.add(egui::DragValue::new(&mut s.daily_news_per_tab).range(0..=20)).changed();
                ui.end_row();
                ui.label("Days of past headlines kept");
                changed |= ui.add(egui::DragValue::new(&mut s.feed_days_kept).range(7..=90)).changed();
                ui.end_row();
            });
            ui.label(RichText::new("Only headlines and short abstracts are written each day; full articles are written when you open them or add them to Read Later.").size(12.5).color(pal.text_weak));
        });
        ui.add_space(10.0);
        let mut new_tab = std::mem::take(&mut self.settings_ui.new_tab);
        egui::Frame::new().fill(pal.card).stroke(Stroke::new(1.0, pal.border)).corner_radius(10).inner_margin(14).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new("Topic tabs").font(theme::bold(15.0)));
            ui.label(RichText::new("You can also drag tabs to reorder them, and right-click a tab to rename or remove it.").size(12.5).color(pal.text_weak));
            let mut action = None;
            let n = s.tabs.len();
            for (i, t) in s.tabs.iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.label(t);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.add_enabled(n > 1, egui::Button::new("Remove").small()).clicked() {
                            action = Some((i, 0));
                        }
                        if ui.add_enabled(i + 1 < n, egui::Button::new("↓").small()).clicked() {
                            action = Some((i, 1));
                        }
                        if ui.add_enabled(i > 0, egui::Button::new("↑").small()).clicked() {
                            action = Some((i, 2));
                        }
                    });
                });
            }
            match action {
                Some((i, 0)) => {
                    s.tabs.remove(i);
                    changed = true;
                }
                Some((i, 1)) => {
                    s.tabs.swap(i, i + 1);
                    changed = true;
                }
                Some((i, 2)) => {
                    s.tabs.swap(i, i - 1);
                    changed = true;
                }
                _ => {}
            }
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut new_tab).hint_text("New topic").desired_width(ui.available_width().min(240.0)));
                if ui.button("Add tab").clicked() && !new_tab.trim().is_empty() {
                    s.tabs.push(new_tab.trim().to_string());
                    new_tab.clear();
                    changed = true;
                }
            });
        });
        self.settings_ui.new_tab = new_tab;
        if changed {
            self.discover.tab = self.discover.tab.min(self.settings.tabs.len().saturating_sub(1));
            self.discover.needs_reload = true;
            self.settings_changed();
        }
    }

    fn settings_storage(&mut self, ui: &mut Ui) {
        let pal = self.pal;
        let u = self.settings_ui.usage.clone().unwrap_or_else(|| self.db.usage());
        let mb = |b: u64| format!("{:.1} MB", b as f64 / 1_048_576.0);
        let mut limit = self.settings.history_limit_mb;
        let mut changed = false;
        let dir = self.db.dir.display().to_string();
        self.group(ui, "Disk use", |ui| {
            egui::Grid::new("usage").num_columns(2).spacing([16.0, 4.0]).show(ui, |ui| {
                ui.label("Saved articles (kept permanently)");
                ui.label(mb(u.saved_articles));
                ui.end_row();
                ui.label("History: unsaved articles");
                ui.label(mb(u.history_articles));
                ui.end_row();
                ui.label("History: past Discover days");
                ui.label(mb(u.feed));
                ui.end_row();
                ui.label("History: downloaded data");
                ui.label(mb(u.http_cache));
                ui.end_row();
                ui.label("History: images");
                ui.label(mb(u.image_cache));
                ui.end_row();
                ui.label(RichText::new("History total").strong());
                ui.label(RichText::new(mb(u.history_total())).strong());
                ui.end_row();
            });
            ui.label(RichText::new(format!("Stored in {dir}")).size(12.0).color(pal.text_faint));
        });
        self.group(ui, "History size limit", |ui| {
            ui.label(RichText::new("History (recently read articles, past headlines, caches) is limited by disk size, not count. The oldest goes first.").size(12.5).color(pal.text_weak));
            if ui.add(egui::Slider::new(&mut limit, 50..=10_000).logarithmic(true).suffix(" MB")).drag_stopped() {
                changed = true;
            }
        });
        if limit != self.settings.history_limit_mb {
            self.settings.history_limit_mb = limit;
        }
        let mut clear = false;
        self.group(ui, "Clear cache", |ui| {
            ui.label(RichText::new("Clears history only — never favourites, Read Later, bookshelves, projects or notes. Unsaved AI-written articles are deleted and would have to be written again.").size(12.5).color(pal.text_weak));
            if ui.add(egui::Button::new(RichText::new("Clear cache…").color(Color32::WHITE)).fill(pal.danger)).clicked() {
                clear = true;
            }
        });
        if clear {
            self.modal = Some(Modal::ClearCache);
        }
        if ui.button("Refresh").clicked() {
            self.settings_ui.usage = Some(self.db.usage());
        }
        if changed {
            self.settings_changed();
        }
    }

    fn settings_citations(&mut self, ui: &mut Ui) {
        let pal = self.pal;
        let mut style = self.settings.citation_style;
        self.group(ui, "Reference style", |ui| {
            ui.label(RichText::new("Used for references at the end of articles, exports and the bibliography. Harvard is the default.").size(12.5).color(pal.text_weak));
            for s in CitationStyle::ALL {
                ui.radio_value(&mut style, s, s.label());
            }
        });
        if style != self.settings.citation_style {
            self.settings.citation_style = style;
            self.settings_changed();
        }
    }

    fn settings_about(&mut self, ui: &mut Ui) {
        let pal = self.pal;
        ui.horizontal(|ui| {
            ui.add(icons::icon("logo", 34.0, pal.accent));
            ui.vertical(|ui| {
                ui.label(RichText::new("Gateless Scholar").font(theme::bold(20.0)));
                ui.label(RichText::new(format!("Research, ungated · version {}", env!("CARGO_PKG_VERSION"))).color(pal.text_weak));
            });
        });
        ui.add_space(8.0);
        ui.label("Open source under the MIT or Apache-2.0 licence, at your option.");
        ui.label(RichText::new("Built with egui and the Craft suite's libraries (craft-ui, craft-window) from the ArtCraft project, both MIT/Apache-2.0.").color(pal.text_weak));
        ui.add_space(8.0);
        ui.label(RichText::new("Honesty").font(theme::bold(15.0)));
        ui.label(RichText::new("Every passage records where it came from: original text, AI-reworded original, or AI-written. AI text cites the real sources it was written from; claims without a source are marked “unverified”. Interpretations of abstract-only papers are never presented as the original authors' work.").color(pal.text_weak));
        ui.add_space(8.0);
        ui.label(RichText::new("Data sources").font(theme::bold(15.0)));
        ui.label(RichText::new("OpenAlex (CC0), Semantic Scholar, arXiv, Crossref, Unpaywall, Europe PMC, CORE, GDELT, Wikimedia Commons, and outlet RSS feeds. Images keep their credit and licence.").color(pal.text_weak));
    }
}
