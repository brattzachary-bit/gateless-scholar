//! Charts and diagrams from the AI's small chart specs, drawn in the app's own style.
//! The numbers were validated against the cited sources before the spec got here.

use egui::{Align2, Color32, Pos2, Rect, Sense, Shape, Stroke, Ui, pos2, vec2};

use super::theme::{self, Palette};
use crate::app::Action;
use crate::model::*;

fn series_colors(pal: &Palette) -> [Color32; 6] {
    [pal.accent, pal.ai, pal.badge_abstract, pal.link, pal.badge_news, pal.danger]
}

pub fn chart(ui: &mut Ui, pal: &Palette, spec: &ChartSpec, sources: &[Source], acts: &mut Vec<Action>) {
    egui::Frame::new().fill(pal.card).stroke(Stroke::new(1.0, pal.border)).corner_radius(10).inner_margin(14).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.add(super::icons::icon("chart", 16.0, pal.ai));
            ui.label(egui::RichText::new(&spec.title).font(theme::bold(15.0)));
        });
        let h = match spec.kind {
            ChartKind::Venn => 230.0,
            ChartKind::Flow => 90.0 + 0.0 * spec.nodes.len() as f32,
            _ => 250.0,
        };
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::hover());
        let painter = ui.painter_at(rect.expand(2.0));
        match spec.kind {
            ChartKind::Bar => bar(&painter, pal, spec, rect, resp.hover_pos()),
            ChartKind::Line | ChartKind::Stock => line(&painter, pal, spec, rect, spec.kind == ChartKind::Stock, resp.hover_pos()),
            ChartKind::Venn => venn(&painter, pal, spec, rect),
            ChartKind::Flow => flow(&painter, pal, spec, rect),
        }
        // Legend for multi-series charts.
        if spec.series.len() > 1 {
            ui.horizontal_wrapped(|ui| {
                for (i, s) in spec.series.iter().enumerate() {
                    let (r, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
                    ui.painter().rect_filled(r, 2.0, series_colors(pal)[i % 6]);
                    ui.label(egui::RichText::new(&s.name).size(12.0).color(pal.text_weak));
                }
            });
        }
        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new("AI-specified chart · numbers from").size(12.0).color(pal.ai));
            for c in &spec.citations {
                if let Some(s) = sources.iter().find(|s| s.id == *c) {
                    let r = ui.add(egui::Label::new(egui::RichText::new(format!("[{c}]")).size(12.0).color(pal.link)).sense(Sense::click())).on_hover_text(&s.title);
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
            }
        });
    });
}

fn nice_max(v: f64) -> f64 {
    if v <= 0.0 {
        return 1.0;
    }
    let mag = 10f64.powf(v.log10().floor());
    let n = v / mag;
    let step = if n <= 1.0 { 1.0 } else if n <= 2.0 { 2.0 } else if n <= 5.0 { 5.0 } else { 10.0 };
    step * mag
}

fn fmt_num(v: f64) -> String {
    if v.abs() >= 1e9 {
        format!("{:.1}B", v / 1e9)
    } else if v.abs() >= 1e6 {
        format!("{:.1}M", v / 1e6)
    } else if v.abs() >= 1e4 {
        format!("{:.0}k", v / 1e3)
    } else if v.fract().abs() < 1e-9 {
        format!("{v:.0}")
    } else {
        format!("{v:.2}").trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

/// Axes, gridlines and labels; returns the plot rect and value range.
fn axes(p: &egui::Painter, pal: &Palette, spec: &ChartSpec, rect: Rect, min: f64, max: f64) -> Rect {
    let plot = Rect::from_min_max(rect.min + vec2(48.0, 8.0), rect.max - vec2(8.0, 34.0));
    let font = theme::regular(11.0);
    for i in 0..=4 {
        let t = i as f64 / 4.0;
        let y = plot.bottom() - t as f32 * plot.height();
        p.line_segment([pos2(plot.left(), y), pos2(plot.right(), y)], Stroke::new(1.0, pal.border.gamma_multiply(if i == 0 { 1.0 } else { 0.5 })));
        p.text(pos2(plot.left() - 6.0, y), Align2::RIGHT_CENTER, fmt_num(min + (max - min) * t), font.clone(), pal.text_faint);
    }
    if let Some(y) = &spec.y_label {
        p.text(rect.left_top() + vec2(0.0, -2.0), Align2::LEFT_BOTTOM, y, font.clone(), pal.text_faint);
    }
    if let Some(x) = &spec.x_label {
        p.text(pos2(plot.center().x, rect.bottom()), Align2::CENTER_BOTTOM, x, font, pal.text_faint);
    }
    plot
}

fn bar(p: &egui::Painter, pal: &Palette, spec: &ChartSpec, rect: Rect, hover: Option<Pos2>) {
    let labels: Vec<String> = spec.series.first().map(|s| s.points.iter().map(|(l, _)| l.clone()).collect()).unwrap_or_default();
    if labels.is_empty() {
        return;
    }
    let max = spec.series.iter().flat_map(|s| s.points.iter().map(|(_, v)| *v)).fold(0.0f64, f64::max);
    let min = spec.series.iter().flat_map(|s| s.points.iter().map(|(_, v)| *v)).fold(0.0f64, f64::min);
    let max = nice_max(max);
    let plot = axes(p, pal, spec, rect, min.min(0.0), max);
    let groups = labels.len() as f32;
    let gw = plot.width() / groups;
    let ns = spec.series.len().max(1) as f32;
    let bw = (gw * 0.7 / ns).min(46.0);
    let zero_y = plot.bottom() - ((0.0 - min.min(0.0)) / (max - min.min(0.0))) as f32 * plot.height();
    let font = theme::regular(11.0);
    for (gi, label) in labels.iter().enumerate() {
        let gx = plot.left() + gw * (gi as f32 + 0.5);
        for (si, s) in spec.series.iter().enumerate() {
            let Some((_, v)) = s.points.get(gi) else { continue };
            let x = gx - bw * ns / 2.0 + bw * si as f32;
            let y = plot.bottom() - ((*v - min.min(0.0)) / (max - min.min(0.0))) as f32 * plot.height();
            let r = Rect::from_min_max(pos2(x + 1.0, y.min(zero_y)), pos2(x + bw - 1.0, y.max(zero_y)));
            let hovered = hover.is_some_and(|h| r.expand(2.0).contains(h));
            p.rect_filled(r, 3.0, series_colors(pal)[si % 6].gamma_multiply(if hovered { 1.0 } else { 0.85 }));
            if hovered || spec.series.len() == 1 && labels.len() <= 8 {
                p.text(pos2(r.center().x, r.top() - 3.0), Align2::CENTER_BOTTOM, fmt_num(*v), font.clone(), pal.text_weak);
            }
        }
        let short: String = label.chars().take((gw / 6.5) as usize).collect();
        p.text(pos2(gx, plot.bottom() + 4.0), Align2::CENTER_TOP, short, font.clone(), pal.text_weak);
    }
}

fn line(p: &egui::Painter, pal: &Palette, spec: &ChartSpec, rect: Rect, area: bool, hover: Option<Pos2>) {
    let n = spec.series.iter().map(|s| s.points.len()).max().unwrap_or(0);
    if n == 0 {
        return;
    }
    let vals = spec.series.iter().flat_map(|s| s.points.iter().map(|(_, v)| *v));
    let (mut min, mut max) = vals.fold((f64::MAX, f64::MIN), |(a, b), v| (a.min(v), b.max(v)));
    if area {
        let pad = (max - min).abs() * 0.1 + 1e-9;
        min -= pad;
        max += pad;
    } else {
        min = min.min(0.0);
        max = nice_max(max);
    }
    if (max - min).abs() < 1e-12 {
        max = min + 1.0;
    }
    let plot = axes(p, pal, spec, rect, min, max);
    let x_at = |i: usize| plot.left() + if n > 1 { i as f32 / (n - 1) as f32 * plot.width() } else { plot.width() / 2.0 };
    let y_at = |v: f64| plot.bottom() - ((v - min) / (max - min)) as f32 * plot.height();
    let font = theme::regular(11.0);
    for (si, s) in spec.series.iter().enumerate() {
        let c = series_colors(pal)[si % 6];
        let pts: Vec<Pos2> = s.points.iter().enumerate().map(|(i, (_, v))| pos2(x_at(i), y_at(*v))).collect();
        if area && pts.len() > 1 {
            // Stock-style: shaded area under the line.
            for w in pts.windows(2) {
                let quad = vec![w[0], w[1], pos2(w[1].x, plot.bottom()), pos2(w[0].x, plot.bottom())];
                p.add(Shape::convex_polygon(quad, c.gamma_multiply(0.12), Stroke::NONE));
            }
        }
        p.add(Shape::line(pts.clone(), Stroke::new(2.0, c)));
        for (i, pt) in pts.iter().enumerate() {
            let hovered = hover.is_some_and(|h| (h - *pt).length() < 8.0);
            p.circle_filled(*pt, if hovered { 4.5 } else { 2.5 }, c);
            if hovered {
                p.text(*pt - vec2(0.0, 8.0), Align2::CENTER_BOTTOM, format!("{}: {}", s.points[i].0, fmt_num(s.points[i].1)), font.clone(), pal.text);
            }
        }
    }
    if let Some(s) = spec.series.first() {
        let step = (n as f32 / (plot.width() / 70.0)).ceil().max(1.0) as usize;
        for (i, (l, _)) in s.points.iter().enumerate().step_by(step) {
            p.text(pos2(x_at(i), plot.bottom() + 4.0), Align2::CENTER_TOP, l, font.clone(), pal.text_weak);
        }
    }
}

fn venn(p: &egui::Painter, pal: &Palette, spec: &ChartSpec, rect: Rect) {
    let sets = &spec.sets;
    if sets.is_empty() {
        return;
    }
    let max = sets.iter().map(|s| s.size).fold(0.0f64, f64::max).max(1e-9);
    let base = rect.height() * 0.36;
    let centre = rect.center();
    let offsets: Vec<egui::Vec2> = match sets.len() {
        1 => vec![vec2(0.0, 0.0)],
        2 => vec![vec2(-base * 0.55, 0.0), vec2(base * 0.55, 0.0)],
        _ => vec![vec2(-base * 0.55, -base * 0.3), vec2(base * 0.55, -base * 0.3), vec2(0.0, base * 0.5)],
    };
    let font = theme::regular(12.0);
    for (i, s) in sets.iter().take(3).enumerate() {
        let r = base * (s.size / max).sqrt() as f32;
        let c = centre + offsets[i];
        let col = series_colors(pal)[i];
        p.circle(c, r, col.gamma_multiply(0.18), Stroke::new(1.5, col));
        let lp = c + offsets[i].normalized() * r * 0.45;
        p.text(lp, Align2::CENTER_CENTER, format!("{}\n{}", s.label, fmt_num(s.size)), font.clone(), pal.text);
        for (j, ov) in &s.overlaps {
            if *j > i && *j < offsets.len().min(3) {
                let mid = centre + (offsets[i] + offsets[*j]) / 2.0;
                p.text(mid, Align2::CENTER_CENTER, fmt_num(*ov), theme::bold(12.0), pal.text);
            }
        }
    }
}

fn flow(p: &egui::Painter, pal: &Palette, spec: &ChartSpec, rect: Rect) {
    let n = spec.nodes.len();
    if n == 0 {
        return;
    }
    let gap = 26.0;
    let w = ((rect.width() - gap * (n as f32 - 1.0)) / n as f32).max(40.0);
    let h = 54.0;
    let y = rect.center().y;
    let font = theme::regular(12.0);
    let boxes: Vec<Rect> = (0..n).map(|i| Rect::from_min_size(pos2(rect.left() + i as f32 * (w + gap), y - h / 2.0), vec2(w, h))).collect();
    let edges: Vec<(usize, usize, String)> = if spec.edges.is_empty() { (1..n).map(|i| (i - 1, i, String::new())).collect() } else { spec.edges.clone() };
    for (a, b, label) in &edges {
        let (Some(ra), Some(rb)) = (boxes.get(*a), boxes.get(*b)) else { continue };
        let (from, to) = if ra.center().x < rb.center().x { (ra.right_center(), rb.left_center()) } else { (ra.left_center(), rb.right_center()) };
        p.arrow(from, to - from, Stroke::new(1.5, pal.text_weak));
        if !label.is_empty() {
            p.text(pos2((from.x + to.x) / 2.0, from.y - 10.0), Align2::CENTER_BOTTOM, label, theme::regular(10.5), pal.text_faint);
        }
    }
    for (i, r) in boxes.iter().enumerate() {
        p.rect(*r, 8.0, pal.accent_soft, Stroke::new(1.0, pal.accent), egui::StrokeKind::Inside);
        let galley = p.layout(spec.nodes[i].clone(), font.clone(), pal.text, r.width() - 8.0);
        p.galley(r.center() - galley.size() / 2.0, galley, pal.text);
    }
}
