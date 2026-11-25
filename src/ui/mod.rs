//! The window: custom title bar with logo and search, hideable sidebar, the current view,
//! the ask panel, overlays (image viewer, dialogs) and toasts.

pub mod charts;
pub mod discover;
pub mod icons;
pub mod lists;
pub mod notes;
pub mod reader;
pub mod search;
pub mod settings_view;
pub mod theme;
pub mod widgets;

use craft_window::policy::{CaptionFamily, Frame};
use craft_window::Button as WinButton;
use egui::{Align, Color32, CornerRadius, Key, KeyboardShortcut, Layout, Modifiers, Rect, Sense, Stroke, Vec2, vec2};

use crate::app::{Action, App, Modal, ToastKind, View, toast_color};
use crate::model::SavedList;
use crate::settings::ThemePref;
use widgets::icon_button;

pub const TITLE_BAR_HEIGHT: f32 = 50.0;

impl App {
    pub fn draw(&mut self, ui: &mut egui::Ui) {
        let integrated = matches!(self.decision.frame, Frame::Integrated(_));
        self.title_bar(ui, integrated);
        if self.settings.sidebar_open {
            self.sidebar(ui);
        }
        if let View::Reader(key) = self.view.clone()
            && self.readers.get(&key).is_some_and(|r| r.ask_open)
        {
            self.ask_panel(ui, &key);
        }
        egui::CentralPanel::default().frame(egui::Frame::new().fill(self.pal.bg).inner_margin(egui::Margin::symmetric(16, 0))).show(ui, |ui| match self.view.clone() {
            View::Discover => self.discover_view(ui),
            View::Search => self.search_view(ui),
            View::Reader(key) => self.reader_view(ui, &key),
            View::Saved(list) => self.saved_view(ui, list),
            View::Bookshelves => self.bookshelves_view(ui),
            View::Projects(p) => self.projects_view(ui, p),
            View::Notes(p) => self.notes_view(ui, p),
            View::History => self.history_view(ui),
            View::Queue => self.queue_view(ui),
            View::Notifications => self.notifications_view(ui),
            View::Bibliography => self.bibliography_view(ui),
            View::Settings => self.settings_view(ui),
        });
        if integrated {
            self.window_edges(ui);
        }
        self.image_viewer(ui);
        self.modals(ui);
        self.toasts(ui);
    }

    // ------------------------------------------------------------------ title bar

    fn title_bar(&mut self, ui: &mut egui::Ui, integrated: bool) {
        let pal = self.pal;
        egui::Panel::top("title_bar")
            .exact_size(TITLE_BAR_HEIGHT)
            .frame(egui::Frame::new().fill(pal.sidebar).stroke(Stroke::new(1.0, pal.border)).inner_margin(egui::Margin::symmetric(10, 0)))
            .show(ui, |ui| {
                let rect = ui.max_rect();
                // The strip's empty part moves the window and double-click maximises.
                if integrated {
                    craft_window::title_bar::empty_area(ui, rect);
                }
                let captions = match &self.decision.frame {
                    Frame::Integrated(c) => Some(c.clone()),
                    _ => None,
                };
                ui.horizontal_centered(|ui| {
                    if let Some(c) = &captions
                        && !c.layout.left.is_empty()
                    {
                        self.window_buttons(ui, &c.layout.left, c.family);
                        ui.add_space(8.0);
                    }
                    if icon_button(ui, &pal, "sidebar", "Toggle sidebar (Ctrl+B)", self.settings.sidebar_open).clicked() {
                        self.actions.push(Action::ToggleSidebar);
                    }
                    let back = icon_button(ui, &pal, "chevron-left", "Back (Alt+←)", false);
                    if back.clicked() {
                        self.actions.push(Action::Back);
                    }
                    // Logo.
                    let logo = ui
                        .horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 6.0;
                            ui.add(icons::icon("logo", 22.0, pal.accent));
                            ui.label(egui::RichText::new("Gateless Scholar").font(theme::bold(15.5)).color(pal.text));
                        })
                        .response
                        .interact(Sense::click())
                        .on_hover_text("Discover (home)");
                    if logo.clicked() {
                        self.actions.push(Action::Navigate(View::Discover));
                    }
                    ui.add_space(12.0);

                    // Right side first so the search bar can take the middle.
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if let Some(c) = &captions
                            && !c.layout.right.is_empty()
                        {
                            let mut right = c.layout.right.clone();
                            right.reverse(); // outermost last → draw outermost first in RTL
                            self.window_buttons(ui, &right, c.family);
                            ui.add_space(6.0);
                        }
                        let dark = self.settings.theme == ThemePref::Dark;
                        if icon_button(ui, &pal, if dark { "sun" } else { "moon" }, "Light / dark mode (Ctrl+Shift+L)", false).clicked() {
                            self.actions.push(Action::ToggleTheme);
                        }
                        let unread = self.unread_notifications;
                        let bell = icon_button(ui, &pal, "bell", "Notifications", matches!(self.view, View::Notifications));
                        if unread > 0 {
                            let c = bell.rect.right_top() + vec2(-7.0, 7.0);
                            ui.painter().circle_filled(c, 4.0, pal.danger);
                        }
                        if bell.clicked() {
                            self.actions.push(Action::Navigate(View::Notifications));
                        }
                        if self.daily.running {
                            let frac = if self.daily.total > 0 { self.daily.done as f32 / self.daily.total as f32 } else { 0.0 };
                            ui.add(egui::ProgressBar::new(frac).desired_width(110.0).desired_height(6.0).fill(pal.accent))
                                .on_hover_text(format!("{} ({}/{})", self.daily.text, self.daily.done, self.daily.total));
                        }
                        ui.add_space(8.0);
                        // Search bar takes the remaining middle.
                        let w = (ui.available_width() - 8.0).clamp(160.0, 640.0);
                        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                            let pad = ((ui.available_width() - w) / 2.0).max(0.0);
                            ui.add_space(pad);
                            self.search_bar(ui, w);
                        });
                    });
                });
            });
    }

    fn search_bar(&mut self, ui: &mut egui::Ui, width: f32) {
        let pal = self.pal;
        let frame = egui::Frame::new()
            .fill(if pal.dark { Color32::from_rgb(0x26, 0x28, 0x2a) } else { Color32::WHITE })
            .stroke(Stroke::new(1.0, if self.search_has_focus { pal.accent } else { pal.border }))
            .corner_radius(CornerRadius::same(17))
            .inner_margin(egui::Margin::symmetric(12, 4));
        frame.show(ui, |ui| {
            ui.set_width(width - 26.0);
            ui.horizontal(|ui| {
                ui.add(icons::icon("search", 16.0, pal.text_weak));
                let edit = egui::TextEdit::singleline(&mut self.search.input)
                    .hint_text("What do you want to research?")
                    .frame(egui::Frame::NONE)
                    .desired_width(ui.available_width() - 70.0)
                    .id_salt("main_search");
                let resp = ui.add(edit);
                if self.search.focus_request {
                    resp.request_focus();
                    self.search.focus_request = false;
                }
                self.search_has_focus = resp.has_focus();
                if resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                    let q = self.search.input.clone();
                    self.actions.push(Action::Search(q));
                }
                // Mode pill: Scholar / AI.
                let ai = self.search.mode == crate::app::SearchMode::Ai;
                let label = if ai { "AI" } else { "Scholar" };
                let pill = ui
                    .add(egui::Button::new(egui::RichText::new(label).size(12.0).color(if ai { pal.ai } else { pal.text_weak })).corner_radius(CornerRadius::same(10)).min_size(vec2(56.0, 22.0)))
                    .on_hover_text("Search mode: Scholar lists matching papers; AI curates the newest and most relevant first (Ctrl+M)");
                if pill.clicked() {
                    self.search.mode = if ai { crate::app::SearchMode::Scholar } else { crate::app::SearchMode::Ai };
                }
            });
        });
    }

    fn window_buttons(&mut self, ui: &mut egui::Ui, buttons: &[WinButton], family: CaptionFamily) {
        let pal = self.pal;
        let maximized = ui.ctx().input(|i| i.viewport().maximized.unwrap_or(false));
        for b in buttons {
            let (name, tip) = match b {
                WinButton::Minimize => ("win-min", "Minimise"),
                WinButton::Maximize => (if maximized { "win-restore" } else { "win-max" }, if maximized { "Restore" } else { "Maximise" }),
                WinButton::Close => ("win-close", "Close"),
            };
            let size = Vec2::splat(if family == CaptionFamily::Windows { 34.0 } else { 26.0 });
            let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
            let hovered = resp.hovered();
            let close = matches!(b, WinButton::Close);
            // Breeze/Adwaita-style round buttons; Windows-style square ones.
            if hovered {
                let fill = if close { pal.danger } else { pal.card_hover };
                match family {
                    CaptionFamily::Windows => {
                        ui.painter().rect_filled(rect, 0.0, fill);
                    }
                    _ => {
                        ui.painter().circle_filled(rect.center(), size.x / 2.0 - 2.0, fill);
                    }
                }
            } else if family == CaptionFamily::Adwaita {
                ui.painter().circle_filled(rect.center(), size.x / 2.0 - 3.0, pal.card);
            }
            let tint = if hovered && close { Color32::WHITE } else { pal.text_weak };
            icons::icon(name, 16.0, tint).paint_at(ui, Rect::from_center_size(rect.center(), Vec2::splat(16.0)));
            let resp = resp.on_hover_text(tip);
            if resp.clicked() {
                let cmd = match b {
                    WinButton::Minimize => egui::ViewportCommand::Minimized(true),
                    WinButton::Maximize => egui::ViewportCommand::Maximized(!maximized),
                    WinButton::Close => egui::ViewportCommand::Close,
                };
                ui.ctx().send_viewport_cmd(cmd);
            }
            ui.add_space(2.0);
        }
    }

    /// With the app's own title bar the window has no decorations, so its edges resize it.
    fn window_edges(&mut self, ui: &mut egui::Ui) {
        use egui::viewport::ResizeDirection as D;
        let ctx = ui.ctx().clone();
        if ctx.input(|i| i.viewport().maximized.unwrap_or(false) || i.viewport().fullscreen.unwrap_or(false)) {
            return;
        }
        let r = ctx.content_rect();
        let m = 5.0;
        let zones = [
            (Rect::from_min_max(r.left_top(), r.left_top() + vec2(m * 2.0, m * 2.0)), D::NorthWest, egui::CursorIcon::ResizeNorthWest),
            (Rect::from_min_max(r.right_top() + vec2(-m * 2.0, 0.0), r.right_top() + vec2(0.0, m * 2.0)), D::NorthEast, egui::CursorIcon::ResizeNorthEast),
            (Rect::from_min_max(r.left_bottom() + vec2(0.0, -m * 2.0), r.left_bottom() + vec2(m * 2.0, 0.0)), D::SouthWest, egui::CursorIcon::ResizeSouthWest),
            (Rect::from_min_max(r.right_bottom() - vec2(m * 2.0, m * 2.0), r.right_bottom()), D::SouthEast, egui::CursorIcon::ResizeSouthEast),
            (Rect::from_min_max(r.left_top(), r.left_bottom() + vec2(m, 0.0)), D::West, egui::CursorIcon::ResizeWest),
            (Rect::from_min_max(r.right_top() - vec2(m, 0.0), r.right_bottom()), D::East, egui::CursorIcon::ResizeEast),
            (Rect::from_min_max(r.left_top(), r.right_top() + vec2(0.0, m)), D::North, egui::CursorIcon::ResizeNorth),
            (Rect::from_min_max(r.left_bottom() - vec2(0.0, m), r.right_bottom()), D::South, egui::CursorIcon::ResizeSouth),
        ];
        egui::Area::new(egui::Id::unique("window_edges")).order(egui::Order::Foreground).fixed_pos(r.min).interactable(true).show(&ctx, |ui| {
            for (i, (rect, dir, cursor)) in zones.into_iter().enumerate() {
                let resp = ui.interact(rect, egui::Id::unique(("edge", i)), Sense::drag());
                if resp.hovered() || resp.dragged() {
                    ctx.set_cursor_icon(cursor);
                }
                if resp.drag_started() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::BeginResize(dir));
                }
            }
        });
    }

    // ------------------------------------------------------------------ sidebar

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        let pal = self.pal;
        let width = self.settings.sidebar_width.clamp(180.0, 380.0);
        let panel = egui::Panel::left("sidebar")
            .exact_size(width)
            .resizable(false)
            .frame(egui::Frame::new().fill(pal.sidebar).inner_margin(egui::Margin { left: 8, right: 8, top: 10, bottom: 8 }))
            .show(ui, |ui| {
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    let unread = self.unread_notifications;
                    let queued = self.db.queue().iter().filter(|q| matches!(q.status, crate::db::QueueStatus::Queued | crate::db::QueueStatus::Running)).count();
                    self.nav_item(ui, "compass", "Discover", View::Discover, None);
                    self.nav_item(ui, "search", "Search", View::Search, None);
                    ui.add_space(6.0);
                    self.nav_section(ui, "Library", |app, ui| {
                        app.nav_item(ui, "heart", "Favourites", View::Saved(SavedList::Favourites), None);
                        app.nav_item(ui, "clock", "Read Later", View::Saved(SavedList::ReadLater), None);
                        app.nav_item(ui, "books", "Bookshelves", View::Bookshelves, None);
                    });
                    self.nav_section(ui, "Research", |app, ui| {
                        app.nav_item(ui, "folder", "Projects", View::Projects(None), None);
                        app.nav_item(ui, "note", "Notes", View::Notes(None), None);
                        app.nav_item(ui, "quote", "Bibliography", View::Bibliography, None);
                    });
                    self.nav_section(ui, "Activity", |app, ui| {
                        app.nav_item(ui, "history", "History", View::History, None);
                        app.nav_item(ui, "queue", "Generation Queue", View::Queue, (queued > 0).then_some(queued));
                        app.nav_item(ui, "bell", "Notifications", View::Notifications, (unread > 0).then_some(unread));
                    });
                    ui.add_space(6.0);
                    self.nav_item(ui, "settings", "Settings", View::Settings, None);
                    if !self.has_provider() {
                        ui.add_space(10.0);
                        egui::Frame::new().fill(pal.ai_soft).corner_radius(8).inner_margin(10).show(ui, |ui| {
                            ui.label(egui::RichText::new("No AI model set up").strong().color(pal.ai));
                            ui.label(egui::RichText::new("Search and real papers work now. Add a model to have articles written.").size(12.0).color(pal.text_weak));
                            if ui.link("Set up a model…").clicked() {
                                self.settings_ui.section = 1;
                                self.actions.push(Action::Navigate(View::Settings));
                            }
                        });
                    }
                });
            });
        // craft-ui resize handle on the sidebar's edge.
        let edge = panel.response.rect;
        let divider = Rect::from_min_max(egui::pos2(edge.right() - 2.0, edge.top()), egui::pos2(edge.right() + 2.0, edge.bottom()));
        let style = craft_ui::layout::ResizeHandleStyle { hovered: pal.accent.gamma_multiply(0.5), dragged: pal.accent, focused: pal.accent };
        let r = craft_ui::layout::ResizeHandle::new(egui::Id::unique("sidebar_resize"), craft_ui::layout::SplitAxis::Horizontal, divider, "Resize sidebar", style).show(ui);
        if r.delta != 0.0 {
            self.settings.sidebar_width = (self.settings.sidebar_width + r.delta).clamp(180.0, 380.0);
        }
        if r.response.drag_stopped() {
            self.actions.push(Action::SaveSettings);
        }
    }

    fn nav_section(&mut self, ui: &mut egui::Ui, title: &'static str, add: impl FnOnce(&mut Self, &mut egui::Ui)) {
        let open = self.sidebar_sections.contains(title);
        let resp = ui
            .horizontal(|ui| {
                ui.add_space(6.0);
                ui.label(egui::RichText::new(title.to_uppercase()).size(11.0).color(self.pal.text_faint));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.add(icons::icon(if open { "chevron-down" } else { "chevron-right" }, 12.0, self.pal.text_faint));
                });
            })
            .response
            .interact(Sense::click());
        if resp.clicked() {
            if open {
                self.sidebar_sections.remove(title);
            } else {
                self.sidebar_sections.insert(title);
            }
        }
        if open {
            add(self, ui);
        }
        ui.add_space(4.0);
    }

    fn nav_item(&mut self, ui: &mut egui::Ui, icon: &str, label: &str, view: View, count: Option<usize>) {
        let pal = self.pal;
        let selected = match (&self.view, &view) {
            (View::Projects(_), View::Projects(_)) | (View::Notes(_), View::Notes(_)) => true,
            (a, b) => a == b,
        };
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::click());
        if selected {
            ui.painter().rect_filled(rect, 7.0, pal.accent_soft);
        } else if resp.hovered() {
            ui.painter().rect_filled(rect, 7.0, pal.card_hover);
        }
        let tint = if selected { pal.accent } else { pal.text_weak };
        icons::icon(icon, 17.0, tint).paint_at(ui, Rect::from_center_size(rect.left_center() + vec2(18.0, 0.0), Vec2::splat(17.0)));
        ui.painter().text(rect.left_center() + vec2(36.0, 0.0), egui::Align2::LEFT_CENTER, label, theme::regular(14.0), if selected { pal.text } else { pal.text_weak });
        if let Some(n) = count {
            let t = n.to_string();
            let c = rect.right_center() - vec2(16.0, 0.0);
            ui.painter().rect_filled(Rect::from_center_size(c, vec2(10.0 + 7.0 * t.len() as f32, 18.0)), 9.0, pal.accent_soft);
            ui.painter().text(c, egui::Align2::CENTER_CENTER, t, theme::regular(11.5), pal.accent);
        }
        let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
        if resp.clicked() {
            self.actions.push(Action::Navigate(view.clone()));
        }
        resp.context_menu(|ui| {
            if ui.button("Open").clicked() {
                self.actions.push(Action::Navigate(view.clone()));
            }
            if ui.button("Hide sidebar").clicked() {
                self.actions.push(Action::ToggleSidebar);
            }
        });
    }

    // ------------------------------------------------------------------ overlays

    fn toasts(&mut self, ui: &mut egui::Ui) {
        if self.toasts.is_empty() {
            return;
        }
        let pal = self.pal;
        let ctx = ui.ctx().clone();
        let r = ctx.content_rect();
        egui::Area::new(egui::Id::unique("toasts")).order(egui::Order::Tooltip).anchor(egui::Align2::RIGHT_BOTTOM, vec2(-16.0, -16.0)).show(&ctx, |ui| {
            ui.set_max_width((r.width() * 0.4).clamp(260.0, 460.0));
            let mut dismiss = None;
            for (i, t) in self.toasts.iter().enumerate() {
                let resp = egui::Frame::new()
                    .fill(pal.panel)
                    .stroke(Stroke::new(1.0, pal.border))
                    .corner_radius(10)
                    .inner_margin(egui::Margin::symmetric(12, 9))
                    .shadow(ui.visuals().popup_shadow)
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let (dot, _) = ui.allocate_exact_size(Vec2::splat(8.0), Sense::hover());
                            ui.painter().circle_filled(dot.center(), 4.0, toast_color(&pal, t.kind));
                            ui.add(egui::Label::new(egui::RichText::new(&t.text).color(pal.text)).wrap());
                        });
                    })
                    .response
                    .interact(Sense::click());
                if resp.clicked() {
                    dismiss = Some(i);
                }
                ui.add_space(6.0);
            }
            if let Some(i) = dismiss {
                self.toasts.remove(i);
            }
        });
    }

    fn image_viewer(&mut self, ui: &mut egui::Ui) {
        let Some(img) = self.image_viewer.clone() else { return };
        let pal = self.pal;
        let ctx = ui.ctx().clone();
        let mut close = ctx.input(|i| i.key_pressed(Key::Escape));
        let screen = ctx.content_rect();
        egui::Area::new(egui::Id::unique("image_viewer")).order(egui::Order::Foreground).fixed_pos(screen.min).show(&ctx, |ui| {
            let bg = ui.allocate_rect(screen, Sense::click());
            ui.painter().rect_filled(screen, 0.0, Color32::from_black_alpha(235));
            if bg.clicked() {
                close = true;
            }
            let caption_h = 96.0;
            let area = Rect::from_min_max(screen.min + vec2(40.0, 40.0), screen.max - vec2(40.0, 40.0 + caption_h));
            if img.embeddable {
                match self.texture(&img.url) {
                    Some(tex) => {
                        let size = tex.size_vec2();
                        let scale = (area.width() / size.x).min(area.height() / size.y).min(2.0);
                        let rect = Rect::from_center_size(area.center(), size * scale);
                        ui.painter().image(tex.id(), rect, Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
                    }
                    None => {
                        ui.painter().text(area.center(), egui::Align2::CENTER_CENTER, "Loading image…", theme::regular(16.0), Color32::LIGHT_GRAY);
                    }
                }
            } else {
                ui.painter().text(area.center(), egui::Align2::CENTER_CENTER, "This image is copyrighted, so it is shown as a link only.", theme::regular(16.0), Color32::LIGHT_GRAY);
            }
            let cap = Rect::from_min_max(egui::pos2(screen.left() + 40.0, screen.bottom() - caption_h - 20.0), egui::pos2(screen.right() - 40.0, screen.bottom() - 10.0));
            ui.scope_builder(egui::UiBuilder::new().max_rect(cap), |ui| {
                ui.visuals_mut().override_text_color = Some(Color32::from_gray(230));
                ui.label(egui::RichText::new(&img.title).strong().size(15.0));
                ui.horizontal_wrapped(|ui| {
                    if !img.credit.is_empty() {
                        ui.label(format!("Credit: {}", img.credit));
                        ui.label("·");
                    }
                    match &img.licence_url {
                        Some(u) => {
                            ui.hyperlink_to(egui::RichText::new(format!("Licence: {}", img.licence)).color(pal.link), u);
                        }
                        None => {
                            ui.label(format!("Licence: {}", img.licence));
                        }
                    }
                    ui.label("·");
                    ui.hyperlink_to(egui::RichText::new("Source page ↗").color(pal.link), &img.page_url);
                });
                if !img.from_paper {
                    ui.label(egui::RichText::new("Note: this is the closest relevant openly licensed image found — it may not be from the paper.").italics().color(Color32::from_gray(190)));
                }
            });
            let close_rect = Rect::from_center_size(screen.right_top() + vec2(-28.0, 28.0), Vec2::splat(30.0));
            let c = ui.interact(close_rect, egui::Id::unique("viewer_close"), Sense::click());
            ui.painter().circle_filled(close_rect.center(), 15.0, Color32::from_white_alpha(if c.hovered() { 50 } else { 25 }));
            icons::icon("x", 18.0, Color32::WHITE).paint_at(ui, Rect::from_center_size(close_rect.center(), Vec2::splat(18.0)));
            if c.clicked() {
                close = true;
            }
        });
        if close {
            self.image_viewer = None;
        }
    }

    fn modals(&mut self, ui: &mut egui::Ui) {
        let Some(modal) = self.modal.take() else { return };
        let pal = self.pal;
        let ctx = ui.ctx().clone();
        let mut keep = true;
        let mut next: Option<Modal> = None;
        let mut modal = modal;
        let resp = egui::Modal::new(egui::Id::unique("modal")).show(&ctx, |ui| {
            ui.set_width(440.0);
            match &mut modal {
                Modal::ClearCache => {
                    ui.heading("Clear cache?");
                    ui.add_space(6.0);
                    ui.label("This clears your history: recently read articles, past Discover days and downloaded data.");
                    ui.add_space(4.0);
                    ui.label(egui::RichText::new("AI-written articles that aren't saved will be deleted and would have to be written again from scratch.").color(pal.warn));
                    ui.add_space(4.0);
                    ui.label(egui::RichText::new("Favourites, Read Later, bookshelves, projects, notes and the articles they link to are never cleared.").color(pal.text_weak));
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        if ui.add(egui::Button::new(egui::RichText::new("Clear cache").color(Color32::WHITE)).fill(pal.danger)).clicked() {
                            match self.db.clear_cache() {
                                Ok(()) => {
                                    self.readers.retain(|_, r| r.article.is_some() && self.db.has_article(&r.doc.key()));
                                    self.discover.needs_reload = true;
                                    self.settings_ui.usage = Some(self.db.usage());
                                    self.images.clear();
                                    self.toast("Cache cleared", ToastKind::Success);
                                }
                                Err(e) => self.toast(e.to_string(), ToastKind::Error),
                            }
                            keep = false;
                        }
                        if ui.button("Cancel").clicked() {
                            keep = false;
                        }
                    });
                }
                Modal::NewBookshelf { name, doc } => {
                    ui.heading("New bookshelf");
                    let r = ui.add(egui::TextEdit::singleline(name).hint_text("Bookshelf name").desired_width(f32::INFINITY));
                    if ui.memory(|m| m.focused().is_none()) {
                        r.request_focus();
                    }
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        let enter = r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                        if (ui.button("Create").clicked() || enter) && !name.trim().is_empty() {
                            match self.db.add_bookshelf(name.trim(), None) {
                                Ok(id) => {
                                    if let Some(d) = doc {
                                        let _ = self.db.save_doc(SavedList::Bookshelf(id), d);
                                        self.toast(format!("Added to “{}”", name.trim()), ToastKind::Success);
                                    }
                                }
                                Err(e) => self.toast(e.to_string(), ToastKind::Error),
                            }
                            keep = false;
                        }
                        if ui.button("Cancel").clicked() {
                            keep = false;
                        }
                    });
                }
                Modal::NewProject { name, description, doc } => {
                    ui.heading("New project");
                    ui.label(egui::RichText::new("A project groups notes, bookshelves and articles under one research topic.").color(pal.text_weak));
                    let r = ui.add(egui::TextEdit::singleline(name).hint_text("Project name").desired_width(f32::INFINITY));
                    // Focus the name only at first, so the description can be clicked into.
                    if ui.memory(|m| m.focused().is_none()) {
                        r.request_focus();
                    }
                    ui.add(egui::TextEdit::multiline(description).hint_text("What is this project about? (optional)").desired_rows(3).desired_width(f32::INFINITY));
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Create").clicked() && !name.trim().is_empty() {
                            match self.db.add_project(name.trim(), description.trim()) {
                                Ok(id) => {
                                    if let Some(d) = doc {
                                        let _ = self.db.save_doc(SavedList::Project(id), d);
                                    }
                                    self.actions.push(Action::Navigate(View::Projects(Some(id))));
                                }
                                Err(e) => self.toast(e.to_string(), ToastKind::Error),
                            }
                            keep = false;
                        }
                        if ui.button("Cancel").clicked() {
                            keep = false;
                        }
                    });
                }
                Modal::ConfirmRegenerate(key) => {
                    ui.heading("Write this article again?");
                    ui.label("The saved article will be replaced by a newly written one. It may read differently.");
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Rewrite").clicked() {
                            self.actions.push(Action::Regenerate(key.clone()));
                            keep = false;
                        }
                        if ui.button("Cancel").clicked() {
                            keep = false;
                        }
                    });
                }
                Modal::Shortcuts => {
                    ui.heading("Keyboard shortcuts");
                    ui.add_space(6.0);
                    egui::Grid::new("shortcuts").num_columns(2).spacing([24.0, 6.0]).show(ui, |ui| {
                        for (k, v) in SHORTCUTS {
                            ui.label(egui::RichText::new(*k).monospace().color(pal.accent));
                            ui.label(*v);
                            ui.end_row();
                        }
                    });
                    ui.add_space(8.0);
                    if ui.button("Close").clicked() {
                        keep = false;
                    }
                }
                Modal::Welcome => {
                    ui.horizontal(|ui| {
                        ui.add(icons::icon("logo", 30.0, pal.accent));
                        ui.heading("Welcome to Gateless Scholar");
                    });
                    ui.label(egui::RichText::new("Research, ungated.").color(pal.text_weak));
                    ui.add_space(8.0);
                    ui.label("Search papers like Google Scholar, browse a daily feed of research and news, and read AI-written articles built from real, clickable sources.");
                    ui.add_space(6.0);
                    ui.label(egui::RichText::new("How text is marked").strong());
                    widgets::legend(ui, &pal);
                    ui.add_space(6.0);
                    ui.label(egui::RichText::new("When a paper only has a free abstract, the app writes its own clearly labelled interpretation — never presented as the original authors' work — and always links to the real paper.").size(13.0).color(pal.text_weak));
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        if ui.add(egui::Button::new(egui::RichText::new("Set up an AI model").color(Color32::WHITE)).fill(pal.accent)).clicked() {
                            self.settings.welcomed = true;
                            self.settings_ui.section = 1;
                            self.actions.push(Action::SaveSettings);
                            self.actions.push(Action::Navigate(View::Settings));
                            keep = false;
                        }
                        if ui.button("Start exploring").clicked() {
                            self.settings.welcomed = true;
                            self.actions.push(Action::SaveSettings);
                            keep = false;
                        }
                    });
                }
            }
        });
        if resp.should_close() && !matches!(modal, Modal::Welcome) {
            keep = false;
        }
        if keep {
            self.modal = Some(modal);
        } else if let Some(n) = next.take() {
            self.modal = Some(n);
        }
    }

    // ------------------------------------------------------------------ keyboard

    pub fn shortcuts(&mut self, ctx: &egui::Context) {
        let cmd = Modifiers::COMMAND;
        let sc = |m: Modifiers, k: Key| KeyboardShortcut::new(m, k);
        let typing = ctx.egui_wants_keyboard_input();
        ctx.input_mut(|i| {
            if i.consume_shortcut(&sc(cmd, Key::K)) || i.consume_shortcut(&sc(cmd, Key::L)) {
                self.search.focus_request = true;
            }
            if !typing && i.consume_key(Modifiers::NONE, Key::Slash) {
                self.search.focus_request = true;
            }
            if i.consume_shortcut(&sc(cmd, Key::B)) {
                self.actions.push(Action::ToggleSidebar);
            }
            if i.consume_shortcut(&sc(cmd, Key::Comma)) {
                self.actions.push(Action::Navigate(View::Settings));
            }
            if i.consume_shortcut(&sc(cmd | Modifiers::SHIFT, Key::L)) {
                self.actions.push(Action::ToggleTheme);
            }
            if i.consume_shortcut(&sc(cmd, Key::M)) {
                self.search.mode = if self.search.mode == crate::app::SearchMode::Ai { crate::app::SearchMode::Scholar } else { crate::app::SearchMode::Ai };
            }
            if i.consume_shortcut(&sc(Modifiers::ALT, Key::ArrowLeft)) || i.pointer.button_pressed(egui::PointerButton::Extra1) {
                self.actions.push(Action::Back);
            }
            if i.consume_shortcut(&sc(Modifiers::ALT, Key::ArrowRight)) || i.pointer.button_pressed(egui::PointerButton::Extra2) {
                self.actions.push(Action::Forward);
            }
            if i.consume_shortcut(&sc(cmd, Key::D)) {
                self.actions.push(Action::Navigate(View::Discover));
            }
            if i.consume_shortcut(&sc(cmd, Key::N)) {
                self.actions.push(Action::Navigate(View::Notes(None)));
            }
            if i.consume_shortcut(&sc(cmd, Key::R)) {
                self.actions.push(Action::RunDaily(true));
            }
            if i.consume_shortcut(&sc(cmd, Key::Slash)) || i.consume_key(Modifiers::NONE, Key::F1) {
                self.modal = Some(Modal::Shortcuts);
            }
            if i.consume_key(Modifiers::NONE, Key::F11) {
                let fs = i.viewport().fullscreen.unwrap_or(false);
                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(!fs));
            }
            if i.consume_shortcut(&sc(cmd, Key::Q)) {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            if i.consume_shortcut(&sc(cmd, Key::J))
                && let View::Reader(k) = &self.view
                && let Some(r) = self.readers.get_mut(k)
            {
                r.ask_open = !r.ask_open;
            }
            if i.consume_shortcut(&sc(cmd | Modifiers::SHIFT, Key::F))
                && let View::Reader(k) = &self.view
                && let Some(r) = self.readers.get(k)
            {
                self.actions.push(Action::ToggleSave(SavedList::Favourites, r.doc.clone()));
            }
            if matches!(self.view, View::Discover) && !typing {
                for (n, key) in [Key::Num1, Key::Num2, Key::Num3, Key::Num4, Key::Num5, Key::Num6, Key::Num7, Key::Num8, Key::Num9].into_iter().enumerate() {
                    if i.consume_shortcut(&sc(cmd, key)) && n < self.settings.tabs.len() {
                        self.discover.tab = n;
                        self.discover.needs_reload = true;
                    }
                }
                if i.consume_key(Modifiers::NONE, Key::Tab) {
                    self.discover.feed = if self.discover.feed == crate::model::FeedKind::Research { crate::model::FeedKind::News } else { crate::model::FeedKind::Research };
                    self.discover.needs_reload = true;
                }
            }
            if i.key_pressed(Key::Escape) && self.image_viewer.is_none() && self.modal.is_none() && !typing {
                self.actions.push(Action::Back);
            }
        });
    }
}

pub const SHORTCUTS: &[(&str, &str)] = &[
    ("Ctrl+K  /  Ctrl+L  /  /", "Focus the search bar"),
    ("Ctrl+M", "Switch Scholar / AI search mode"),
    ("Ctrl+D", "Discover (home)"),
    ("Tab", "Switch Discover Research / News"),
    ("Ctrl+1…9", "Switch topic tab"),
    ("Ctrl+R", "Refresh today's Discover"),
    ("Ctrl+B", "Show / hide sidebar"),
    ("Ctrl+J", "Ask panel (in an article)"),
    ("Ctrl+Shift+F", "Favourite the open article"),
    ("Ctrl+N", "Notes"),
    ("Alt+← / Alt+→  /  Esc", "Back / forward"),
    ("Ctrl+,", "Settings"),
    ("Ctrl+Shift+L", "Light / dark mode"),
    ("F11", "Full screen"),
    ("F1  /  Ctrl+/", "This list"),
    ("Ctrl+Q", "Quit"),
];
