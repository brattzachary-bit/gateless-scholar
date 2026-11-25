//! Colours, fonts and egui style. Dark is the default; light is available.
//! Origin colours are fixed per theme so marking reads the same everywhere.

use egui::{Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Stroke, TextStyle};

use crate::settings::ThemePref;

#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub dark: bool,
    pub bg: Color32,
    pub panel: Color32,
    pub sidebar: Color32,
    pub card: Color32,
    pub card_hover: Color32,
    pub border: Color32,
    pub text: Color32,
    pub text_weak: Color32,
    pub text_faint: Color32,
    pub accent: Color32,
    pub accent_soft: Color32,
    /// AI-written text colour (purple) and its margin bar.
    pub ai: Color32,
    pub ai_soft: Color32,
    /// Links to real sources (green).
    pub link: Color32,
    /// Reworded-original underline.
    pub reworded: Color32,
    pub warn: Color32,
    pub danger: Color32,
    pub badge_free: Color32,
    pub badge_abstract: Color32,
    pub badge_ai: Color32,
    pub badge_news: Color32,
}

impl Palette {
    pub fn for_pref(pref: ThemePref) -> Self {
        match pref {
            ThemePref::Dark => Self::dark(),
            ThemePref::Light => Self::light(),
        }
    }

    pub fn dark() -> Self {
        Palette {
            dark: true,
            bg: Color32::from_rgb(0x19, 0x1a, 0x1a),
            panel: Color32::from_rgb(0x20, 0x21, 0x22),
            sidebar: Color32::from_rgb(0x1d, 0x1e, 0x1f),
            card: Color32::from_rgb(0x23, 0x24, 0x26),
            card_hover: Color32::from_rgb(0x2a, 0x2c, 0x2e),
            border: Color32::from_rgb(0x33, 0x35, 0x37),
            text: Color32::from_rgb(0xe8, 0xe8, 0xe6),
            text_weak: Color32::from_rgb(0xa4, 0xa6, 0xa6),
            text_faint: Color32::from_rgb(0x72, 0x75, 0x76),
            accent: Color32::from_rgb(0x20, 0xb8, 0xcd),
            accent_soft: Color32::from_rgba_unmultiplied(0x20, 0xb8, 0xcd, 40),
            ai: Color32::from_rgb(0xc0, 0x9c, 0xff),
            ai_soft: Color32::from_rgba_unmultiplied(0xa0, 0x7c, 0xf0, 34),
            link: Color32::from_rgb(0x5f, 0xd3, 0x8a),
            reworded: Color32::from_rgb(0xe8, 0xb5, 0x5a),
            warn: Color32::from_rgb(0xf0, 0xa0, 0x40),
            danger: Color32::from_rgb(0xf0, 0x6a, 0x6a),
            badge_free: Color32::from_rgb(0x3f, 0xb9, 0x6f),
            badge_abstract: Color32::from_rgb(0xd9, 0x9a, 0x3a),
            badge_ai: Color32::from_rgb(0xa8, 0x84, 0xf3),
            badge_news: Color32::from_rgb(0x4c, 0x9a, 0xf0),
        }
    }

    pub fn light() -> Self {
        Palette {
            dark: false,
            bg: Color32::from_rgb(0xfb, 0xfa, 0xf7),
            panel: Color32::from_rgb(0xf3, 0xf2, 0xee),
            sidebar: Color32::from_rgb(0xf0, 0xef, 0xea),
            card: Color32::WHITE,
            card_hover: Color32::from_rgb(0xf6, 0xf6, 0xf3),
            border: Color32::from_rgb(0xdd, 0xdc, 0xd6),
            text: Color32::from_rgb(0x1d, 0x1e, 0x20),
            text_weak: Color32::from_rgb(0x5b, 0x5e, 0x61),
            text_faint: Color32::from_rgb(0x8c, 0x8f, 0x92),
            accent: Color32::from_rgb(0x13, 0x86, 0x98),
            accent_soft: Color32::from_rgba_unmultiplied(0x13, 0x86, 0x98, 30),
            ai: Color32::from_rgb(0x6d, 0x3f, 0xd0),
            ai_soft: Color32::from_rgba_unmultiplied(0x6d, 0x3f, 0xd0, 22),
            link: Color32::from_rgb(0x1a, 0x8a, 0x4a),
            reworded: Color32::from_rgb(0xb0, 0x70, 0x10),
            warn: Color32::from_rgb(0xc0, 0x6a, 0x00),
            danger: Color32::from_rgb(0xc0, 0x30, 0x30),
            badge_free: Color32::from_rgb(0x1f, 0x8a, 0x4c),
            badge_abstract: Color32::from_rgb(0xa8, 0x68, 0x10),
            badge_ai: Color32::from_rgb(0x6d, 0x3f, 0xd0),
            badge_news: Color32::from_rgb(0x1f, 0x6a, 0xc0),
        }
    }

    pub fn badge(&self, kind: crate::model::ContentKind) -> Color32 {
        use crate::model::ContentKind::*;
        match kind {
            FreeFullText => self.badge_free,
            AbstractOnly => self.badge_abstract,
            AiArticle => self.badge_ai,
            News => self.badge_news,
        }
    }
}

pub fn apply(ctx: &egui::Context, pal: &Palette, scale: f32) {
    let mut visuals = if pal.dark { egui::Visuals::dark() } else { egui::Visuals::light() };
    visuals.panel_fill = pal.bg;
    visuals.window_fill = pal.panel;
    visuals.extreme_bg_color = if pal.dark { Color32::from_rgb(0x15, 0x16, 0x16) } else { Color32::WHITE };
    visuals.faint_bg_color = pal.card;
    visuals.window_stroke = Stroke::new(1.0, pal.border);
    visuals.window_corner_radius = CornerRadius::same(10);
    visuals.menu_corner_radius = CornerRadius::same(8);
    visuals.hyperlink_color = pal.link;
    visuals.selection.bg_fill = pal.accent.gamma_multiply(0.45);
    visuals.selection.stroke = Stroke::new(1.0, pal.accent);
    visuals.override_text_color = Some(pal.text);
    visuals.window_shadow = egui::Shadow { offset: [0, 6], blur: 24, spread: 0, color: Color32::from_black_alpha(if pal.dark { 110 } else { 40 }) };
    visuals.popup_shadow = egui::Shadow { offset: [0, 4], blur: 16, spread: 0, color: Color32::from_black_alpha(if pal.dark { 100 } else { 35 }) };

    // craft-ui's shared widget-state colouring, from our tokens.
    let palette = craft_ui::theme::WidgetPalette {
        noninteractive: pal.bg,
        inactive: if pal.dark { Color32::from_rgb(0x2b, 0x2d, 0x2f) } else { Color32::from_rgb(0xec, 0xeb, 0xe6) },
        hovered: if pal.dark { Color32::from_rgb(0x34, 0x37, 0x39) } else { Color32::from_rgb(0xe2, 0xe1, 0xdb) },
        active: if pal.dark { Color32::from_rgb(0x3c, 0x40, 0x42) } else { Color32::from_rgb(0xd8, 0xd7, 0xd0) },
        open: if pal.dark { Color32::from_rgb(0x30, 0x33, 0x35) } else { Color32::from_rgb(0xe6, 0xe5, 0xdf) },
    };
    palette.apply_to(&mut visuals.widgets, CornerRadius::same(7), Stroke::new(1.0, pal.text));
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, pal.border);
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, pal.text);
    visuals.widgets.inactive.bg_stroke = Stroke::NONE;
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, pal.border);
    ctx.set_theme(if pal.dark { egui::Theme::Dark } else { egui::Theme::Light });
    let mut style = (*ctx.global_style()).clone();
    style.visuals = visuals;
    style.text_styles = [
        (TextStyle::Heading, FontId::new(22.0, FontFamily::Name("bold".into()))),
        (TextStyle::Body, FontId::new(14.5, FontFamily::Proportional)),
        (TextStyle::Monospace, FontId::new(13.0, FontFamily::Monospace)),
        (TextStyle::Button, FontId::new(14.0, FontFamily::Proportional)),
        (TextStyle::Small, FontId::new(12.0, FontFamily::Proportional)),
    ]
    .into();
    style.spacing.item_spacing = egui::vec2(8.0, 6.0);
    style.spacing.button_padding = egui::vec2(10.0, 5.0);
    style.spacing.interact_size.y = 26.0;
    style.spacing.menu_margin = egui::Margin::same(6);
    style.interaction.tooltip_delay = 0.35;
    style.url_in_tooltip = true;
    ctx.set_global_style(style);
    ctx.set_zoom_factor(scale.clamp(0.6, 2.0));
}

/// Load the desktop's sans-serif font (via fontconfig) as the proportional face, with a bold
/// face as the "bold" family. Falls back to egui's built-in faces.
// TODO(windows): no fc-match on Windows; look in %WINDIR%\Fonts (Segoe UI) instead.
pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let regular = fc_match("sans-serif:style=Regular").or_else(|| fc_match("sans-serif"));
    let bold = fc_match("sans-serif:style=Bold").or_else(|| fc_match("sans-serif:weight=200"));
    let mut bold_family = vec![];
    if let Some(bytes) = regular.as_ref().and_then(|p| std::fs::read(p).ok()) {
        fonts.font_data.insert("system-sans".into(), FontData::from_owned(bytes).into());
        if let Some(f) = fonts.families.get_mut(&FontFamily::Proportional) {
            f.insert(0, "system-sans".into());
        }
    }
    if let Some(bytes) = bold.as_ref().filter(|b| Some(*b) != regular.as_ref()).and_then(|p| std::fs::read(p).ok()) {
        fonts.font_data.insert("system-sans-bold".into(), FontData::from_owned(bytes).into());
        bold_family.push("system-sans-bold".to_string());
    }
    let prop = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    bold_family.extend(prop);
    fonts.families.insert(FontFamily::Name("bold".into()), bold_family);
    ctx.set_fonts(fonts);
}

fn fc_match(pattern: &str) -> Option<String> {
    let out = std::process::Command::new("fc-match").args(["-f", "%{file}", pattern]).output().ok()?;
    let path = String::from_utf8(out.stdout).ok()?;
    let path = path.trim();
    let ok = (path.ends_with(".ttf") || path.ends_with(".otf")) && std::path::Path::new(path).exists();
    ok.then(|| path.to_string())
}

pub fn bold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("bold".into()))
}
pub fn regular(size: f32) -> FontId {
    FontId::new(size, FontFamily::Proportional)
}
