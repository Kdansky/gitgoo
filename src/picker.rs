//! The branch picker: a map of branches leaving and rejoining the trunk, time running left to right.

use eframe::egui::{self, Color32, Pos2, Rect, Sense, Shape, Stroke, TextStyle, pos2, vec2};
use egui::epaint::CubicBezierShape;

use crate::git;
use crate::graph::{Branch, Graph, Lane};

pub struct Picker {
    /// The history column it picks for: 0 is A, 1 is B.
    pub side: usize,
    /// None while loading.
    pub graph: Option<git::Result<Graph>>,
    filter: String,
}

pub enum Outcome {
    Open,
    Closed,
    Picked(String),
}

const ROW: f32 = 28.0;

const PALETTE: [Color32; 8] = [
    Color32::from_rgb(0x4e, 0x79, 0xa7),
    Color32::from_rgb(0xf2, 0x8e, 0x2b),
    Color32::from_rgb(0x59, 0xa1, 0x4f),
    Color32::from_rgb(0xe1, 0x57, 0x59),
    Color32::from_rgb(0x76, 0xb7, 0xb2),
    Color32::from_rgb(0xed, 0xc9, 0x48),
    Color32::from_rgb(0xb0, 0x7a, 0xa1),
    Color32::from_rgb(0xff, 0x9d, 0xa7),
];

impl Picker {
    pub fn new(side: usize) -> Self {
        Self { side, graph: None, filter: String::new() }
    }

    /// `current` is the column's branch, highlighted.
    pub fn show(&mut self, ctx: &egui::Context, current: &str) -> Outcome {
        let mut picked = None;
        let size = ctx.content_rect().size() * vec2(0.85, 0.8);
        let modal = egui::Modal::new(egui::Id::unique("branches")).show(ctx, |ui| {
            ui.set_min_size(size);
            ui.set_max_size(size);
            ui.horizontal(|ui| {
                ui.heading(format!("Branch for column {}", ["A", "B"][self.side]));
                ui.add_space(16.0);
                // Typing always filters: nothing else in the dialog takes keys.
                ui.add(egui::TextEdit::singleline(&mut self.filter).hint_text("Filter; Enter picks the top match")).request_focus();
            });
            ui.separator();
            let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
            match &self.graph {
                None => drop(ui.spinner()),
                Some(Err(e)) => drop(ui.colored_label(ui.visuals().error_fg_color, e.as_str())),
                Some(Ok(g)) => picked = map(ui, g, &self.filter, current, enter),
            }
        });
        match picked {
            Some(name) => Outcome::Picked(name),
            None if modal.should_close() => Outcome::Closed,
            None => Outcome::Open,
        }
    }
}

/// Draws the map and returns the branch clicked, or the top match when Enter was pressed.
fn map(ui: &mut egui::Ui, g: &Graph, filter: &str, current: &str, enter: bool) -> Option<String> {
    let f = filter.to_lowercase();
    let lanes: Vec<&Lane> = g.lanes.iter().filter(|l| l.branch.name.to_lowercase().contains(&f)).collect();
    if enter && !f.is_empty() {
        let top = lanes.first().map(|l| &l.branch).or(Some(&g.trunk).filter(|t| t.name.to_lowercase().contains(&f)));
        if let Some(b) = top {
            return Some(b.name.clone());
        }
    }
    // Events are spaced evenly in time order rather than by date, so a burst of activity doesn't crowd into a sliver.
    let mut times: Vec<i64> =
        lanes.iter().flat_map(|l| [Some(l.fork.time), Some(l.branch.time), l.merge.as_ref().map(|m| m.time)]).flatten().collect();
    times.push(g.trunk.time);
    times.sort_unstable();
    times.dedup();
    let rank = |t: i64| times.partition_point(|&x| x < t) as f32;

    let v = ui.visuals().clone();
    let font = TextStyle::Body.resolve(ui.style());
    let painter = ui.painter().clone();
    let layout = |b: &Branch| {
        let color = if b.remote { v.weak_text_color() } else { v.strong_text_color() };
        painter.layout_no_wrap(b.name.clone(), font.clone(), color)
    };
    let label_w = lanes.iter().map(|l| &l.branch).chain([&g.trunk]).map(|b| layout(b).size().x).fold(0.0, f32::max);
    let margin = 16.0;
    let span = (times.len().max(2) - 1) as f32;
    // Wide enough to tell events apart, narrow enough that a quiet repo's map doesn't sprawl.
    let step = ((ui.available_width() - 2.0 * margin - label_w - 16.0) / span).clamp(24.0, 120.0);
    let size = vec2(2.0 * margin + span * step + label_w + 16.0, (lanes.len() + 1) as f32 * ROW);
    let bend = (step * 0.6).min(14.0);

    let mut picked = None;
    egui::ScrollArea::both().auto_shrink(false).show(ui, |ui| {
        let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
        let x = |t: i64| rect.left() + margin + rank(t) * step;
        let y = |row: usize| rect.top() + (row as f32 + 0.5) * ROW;
        let p = ui.painter();
        let curve = |pts: [Pos2; 4], stroke: Stroke| p.add(CubicBezierShape::from_points_stroke(pts, false, Color32::TRANSPARENT, stroke));
        let y0 = y(0);
        p.line_segment([pos2(x(times[0]), y0), pos2(x(g.trunk.time), y0)], Stroke::new(3.0, v.strong_text_color()));
        // Where each label goes: its row, the x it follows, its lane.
        let mut labels = vec![(0, x(g.trunk.time), None)];
        for (i, &l) in lanes.iter().enumerate() {
            let row = i + 1;
            let yi = y(row);
            let color = PALETTE[i % PALETTE.len()];
            // Merged branches are history; the open ones are what you're likely here for.
            let stroke = Stroke::new(2.0, if l.merge.is_some() { color.gamma_multiply(0.5) } else { color });
            let end = if l.commits == 0 {
                // Only a pointer at a trunk commit: a dashed drop to its name.
                let xt = x(l.branch.time);
                p.extend(Shape::dashed_line(&[pos2(xt, y0), pos2(xt, yi)], stroke, 4.0, 3.0));
                xt
            } else {
                let xf = x(l.fork.time);
                let xt = x(l.branch.time).max(xf + bend);
                curve([pos2(xf, y0), pos2(xf, yi), pos2(xf, yi), pos2(xf + bend, yi)], stroke);
                p.line_segment([pos2(xf + bend, yi), pos2(xt, yi)], stroke);
                match &l.merge {
                    Some(m) => {
                        let xm = x(m.time).max(xt + bend);
                        curve([pos2(xt, yi), pos2(xm, yi), pos2(xm, yi), pos2(xm, y0)], stroke);
                        xm
                    }
                    None => xt,
                }
            };
            labels.push((row, end, Some(l)));
        }
        // Labels last, on a backing that masks the lines crossing behind them.
        for (row, at, lane) in labels {
            let b = lane.map_or(&g.trunk, |l| &l.branch);
            let galley = layout(b);
            let r = Rect::from_min_size(pos2(at + 8.0, y(row) - galley.size().y / 2.0), galley.size()).expand2(vec2(4.0, 2.0));
            let resp = ui.interact(r, ui.scope_id().with(("branch", row)), Sense::click());
            resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, b.name == current, &b.name));
            let fill = if b.name == current {
                v.selection.bg_fill
            } else if resp.hovered() {
                v.widgets.hovered.weak_bg_fill
            } else {
                v.window_fill
            };
            p.rect_filled(r, 3.0, fill);
            p.galley(r.min + vec2(4.0, 2.0), galley, v.text_color());
            if resp.clicked() {
                picked = Some(b.name.clone());
            }
            resp.on_hover_text(tooltip(&g.trunk.name, b, lane));
        }
    });
    picked
}

fn tooltip(trunk: &str, b: &Branch, lane: Option<&Lane>) -> String {
    let mut s = format!("{}\nlast commit {}", b.name, b.date);
    if let Some(l) = lane {
        match l.commits {
            0 => s += &format!("\npoints at a commit on {trunk}"),
            n => s += &format!("\n{n} commit{} since leaving {trunk} {}", if n == 1 { "" } else { "s" }, l.fork.when),
        }
        if let Some(m) = &l.merge {
            s += &format!("\nmerged into {trunk} {}", m.when);
        }
    }
    s
}
