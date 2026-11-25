//! The app's own monochrome SVG icons (drawn for Gateless Scholar, MIT/Apache like the app),
//! served through craft-ui's `SvgIconSet` and tinted at draw time.

use std::sync::OnceLock;

use craft_ui::icons::{SvgIconSet, SvgStyle};
use egui::{Color32, Image, Vec2};

macro_rules! svg {
    ($body:expr) => {
        concat!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">"#,
            $body,
            "</svg>"
        )
        .as_bytes()
    };
}

static ICONS: &[(&str, &[u8])] = &[
    ("square", svg!(r#"<rect x="4" y="4" width="16" height="16" rx="2"/>"#)),
    ("search", svg!(r#"<circle cx="11" cy="11" r="7"/><path d="M20 20l-4-4"/>"#)),
    ("heart", svg!(r#"<path d="M12 20s-7-4.4-8.6-9A4.6 4.6 0 0 1 12 6.3 4.6 4.6 0 0 1 20.6 11C19 15.6 12 20 12 20z"/>"#)),
    ("heart-fill", svg!(r#"<path fill="currentColor" d="M12 20s-7-4.4-8.6-9A4.6 4.6 0 0 1 12 6.3 4.6 4.6 0 0 1 20.6 11C19 15.6 12 20 12 20z"/>"#)),
    ("clock", svg!(r#"<circle cx="12" cy="12" r="9"/><path d="M12 7v5l3 2"/>"#)),
    ("clock-fill", svg!(r#"<circle cx="12" cy="12" r="9" fill="currentColor" fill-opacity="0.35"/><path d="M12 7v5l3 2"/>"#)),
    ("books", svg!(r#"<rect x="4" y="4" width="4" height="16" rx="1"/><rect x="10" y="4" width="4" height="16" rx="1"/><path d="M16.5 5.2l3.6-1 3 15.4-3.6 1z" transform="translate(-2 0)"/>"#)),
    ("folder", svg!(r#"<path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/>"#)),
    ("note", svg!(r#"<path d="M6 3h9l4 4v14H6z"/><path d="M14 3v5h5"/><path d="M9 13h7M9 17h5"/>"#)),
    ("history", svg!(r#"<path d="M3 12a9 9 0 1 0 3-6.7"/><path d="M3 4v4h4"/><path d="M12 8v4l3 2"/>"#)),
    ("queue", svg!(r#"<path d="M4 6h12M4 12h12M4 18h8"/><path d="M18 15l3 3-3 3"/>"#)),
    ("bell", svg!(r#"<path d="M6 16V11a6 6 0 0 1 12 0v5l2 2H4z"/><path d="M10 21h4"/>"#)),
    ("quote", svg!(r#"<path d="M5 18c2-1 3-3 3-6H5V6h5v6c0 4-2 6-5 7zM14 18c2-1 3-3 3-6h-3V6h5v6c0 4-2 6-5 7z"/>"#)),
    ("settings", svg!(r#"<circle cx="12" cy="12" r="3"/><path d="M12 2v3M12 19v3M2 12h3M19 12h3M4.9 4.9l2.1 2.1M17 17l2.1 2.1M4.9 19.1L7 17M17 7l2.1-2.1"/>"#)),
    ("sun", svg!(r#"<circle cx="12" cy="12" r="4"/><path d="M12 2v2M12 20v2M2 12h2M20 12h2M5 5l1.4 1.4M17.6 17.6L19 19M5 19l1.4-1.4M17.6 6.4L19 5"/>"#)),
    ("moon", svg!(r#"<path d="M20 14.5A8 8 0 0 1 9.5 4 8 8 0 1 0 20 14.5z"/>"#)),
    ("share", svg!(r#"<path d="M12 3v12"/><path d="M8 7l4-4 4 4"/><path d="M5 12v7a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2v-7"/>"#)),
    ("plus", svg!(r#"<path d="M12 5v14M5 12h14"/>"#)),
    ("x", svg!(r#"<path d="M6 6l12 12M18 6L6 18"/>"#)),
    ("chevron-left", svg!(r#"<path d="M15 5l-7 7 7 7"/>"#)),
    ("chevron-right", svg!(r#"<path d="M9 5l7 7-7 7"/>"#)),
    ("chevron-down", svg!(r#"<path d="M5 9l7 7 7-7"/>"#)),
    ("sidebar", svg!(r#"<rect x="3" y="4" width="18" height="16" rx="2"/><path d="M9 4v16"/>"#)),
    ("sparkle", svg!(r#"<path d="M12 3l1.8 5.2L19 10l-5.2 1.8L12 17l-1.8-5.2L5 10l5.2-1.8z"/><path d="M19 15l.8 2.2L22 18l-2.2.8L19 21l-.8-2.2L16 18l2.2-.8z"/>"#)),
    ("newspaper", svg!(r#"<rect x="3" y="5" width="15" height="15" rx="1"/><path d="M18 9h3v9a2 2 0 0 1-4 0"/><path d="M6 9h9M6 13h9M6 17h6"/>"#)),
    ("flask", svg!(r#"<path d="M9 3h6M10 3v6l-5 9a2 2 0 0 0 1.8 3h10.4A2 2 0 0 0 19 18l-5-9V3"/><path d="M7.5 15h9"/>"#)),
    ("image", svg!(r#"<rect x="3" y="4" width="18" height="16" rx="2"/><circle cx="9" cy="10" r="2"/><path d="M21 16l-5-5-9 9"/>"#)),
    ("external", svg!(r#"<path d="M14 4h6v6"/><path d="M20 4l-9 9"/><path d="M18 14v5a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1V7a1 1 0 0 1 1-1h5"/>"#)),
    ("refresh", svg!(r#"<path d="M20 12a8 8 0 1 1-2.3-5.7"/><path d="M20 4v5h-5"/>"#)),
    ("filter", svg!(r#"<path d="M4 5h16l-6 8v6l-4-2v-4z"/>"#)),
    ("trash", svg!(r#"<path d="M4 7h16M9 7V4h6v3M6 7l1 13h10l1-13"/>"#)),
    ("more", svg!(r#"<circle cx="5" cy="12" r="1.2" fill="currentColor"/><circle cx="12" cy="12" r="1.2" fill="currentColor"/><circle cx="19" cy="12" r="1.2" fill="currentColor"/>"#)),
    ("send", svg!(r#"<path d="M4 12l16-8-6 16-2-6z"/><path d="M12 14l8-10"/>"#)),
    ("book-open", svg!(r#"<path d="M3 5h6a3 3 0 0 1 3 3v12a2 2 0 0 0-2-2H3z"/><path d="M21 5h-6a3 3 0 0 0-3 3v12a2 2 0 0 1 2-2h7z"/>"#)),
    ("ask", svg!(r#"<path d="M4 5h16v11H9l-5 4z"/><path d="M10 9a2 2 0 1 1 2.5 1.9c-.4.1-.5.4-.5.8"/><circle cx="12" cy="14" r=".5" fill="currentColor"/>"#)),
    ("compass", svg!(r#"<circle cx="12" cy="12" r="9"/><path d="M15.5 8.5l-2 5-5 2 2-5z"/>"#)),
    ("link", svg!(r#"<path d="M10 14a4 4 0 0 0 5.7 0l3-3a4 4 0 0 0-5.7-5.7l-1 1"/><path d="M14 10a4 4 0 0 0-5.7 0l-3 3a4 4 0 0 0 5.7 5.7l1-1"/>"#)),
    ("download", svg!(r#"<path d="M12 4v11"/><path d="M7 10l5 5 5-5"/><path d="M5 20h14"/>"#)),
    ("check", svg!(r#"<path d="M5 12l5 5 9-10"/>"#)),
    ("alert", svg!(r#"<path d="M12 3l10 18H2z"/><path d="M12 10v5"/><circle cx="12" cy="18" r=".5" fill="currentColor"/>"#)),
    ("logo", svg!(r#"<path d="M4 20V9a8 8 0 0 1 16 0v11"/><path d="M8 20v-9a4 4 0 0 1 8 0v9"/><path d="M2 20h20"/>"#)),
    ("win-min", svg!(r#"<path d="M7 12h10"/>"#)),
    ("win-max", svg!(r#"<rect x="7" y="7" width="10" height="10" rx="1"/>"#)),
    ("win-restore", svg!(r#"<rect x="6" y="9" width="9" height="9" rx="1"/><path d="M9 9V6h9v9h-3"/>"#)),
    ("win-close", svg!(r#"<path d="M7.5 7.5l9 9M16.5 7.5l-9 9"/>"#)),
    ("keyboard", svg!(r#"<rect x="2" y="6" width="20" height="12" rx="2"/><path d="M6 10h.01M10 10h.01M14 10h.01M18 10h.01M7 14h10"/>"#)),
    ("print", svg!(r#"<path d="M6 9V3h12v6"/><rect x="3" y="9" width="18" height="8" rx="2"/><path d="M6 14h12v7H6z"/>"#)),
    ("mail", svg!(r#"<rect x="3" y="5" width="18" height="14" rx="2"/><path d="M3 7l9 6 9-6"/>"#)),
    ("file", svg!(r#"<path d="M6 3h9l4 4v14H6z"/><path d="M14 3v5h5"/>"#)),
    ("copy", svg!(r#"<rect x="8" y="8" width="12" height="12" rx="2"/><path d="M16 8V5a1 1 0 0 0-1-1H5a1 1 0 0 0-1 1v10a1 1 0 0 0 1 1h3"/>"#)),
    ("chart", svg!(r#"<path d="M4 20V4"/><path d="M4 20h16"/><path d="M8 16v-5M12 16V8M16 16v-3"/>"#)),
    ("user", svg!(r#"<circle cx="12" cy="8" r="4"/><path d="M4 21a8 8 0 0 1 16 0"/>"#)),
    ("tag", svg!(r#"<path d="M3 12V4h8l10 10-8 8z"/><circle cx="7.5" cy="7.5" r="1.2"/>"#)),
    ("model", svg!(r#"<rect x="6" y="6" width="12" height="12" rx="2"/><path d="M9 2v4M15 2v4M9 18v4M15 18v4M2 9h4M2 15h4M18 9h4M18 15h4"/>"#)),
];

pub fn set() -> &'static SvgIconSet {
    static SET: OnceLock<SvgIconSet> = OnceLock::new();
    SET.get_or_init(|| SvgIconSet::new(ICONS, SvgStyle { current_color: "#ffffff", stroke_width: Some("1.75") }, "bytes://gateless/icons/", "square"))
}

/// A tinted icon image of the given size.
pub fn icon(name: &str, size: f32, tint: Color32) -> Image<'static> {
    Image::new(set().source(name)).fit_to_exact_size(Vec2::splat(size)).tint(tint)
}
