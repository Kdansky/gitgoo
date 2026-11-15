//! Draws columns D and E: one widget painting both sides of each row, so the sides can't drift apart. Inline, the two
//! columns become one. It shows every file that differs, one after another, or only the one picked in C.

use std::ops::Range;
use std::sync::mpsc::Receiver;

use eframe::egui::{self, Align2, Color32, Painter, Rect, Sense, Stroke, StrokeKind, TextFormat, TextStyle, pos2, vec2};
use egui::text::LayoutJob;

use crate::diff::{self, Diff, Item, Kind, Mode};
use crate::git::Change;

const RED: Color32 = Color32::from_rgb(0xf0, 0x50, 0x50);
const GREEN: Color32 = Color32::from_rgb(0x40, 0xc0, 0x50);

/// Which files the view shows.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Layout {
    /// Every file that differs, one after another.
    All,
    /// Only the file picked in C: for diffs too large to show whole.
    One,
}

/// A file's diff, or why there is none.
pub type Body = Result<Diff, String>;

/// One drawn row of the view.
#[derive(Clone, Copy)]
enum Line {
    /// Blank, above each file's header but the first.
    Space(usize),
    /// A file's path and line counts, when the view holds several files.
    Head(usize),
    /// A file with no rows to show: still loading, binary, or empty.
    Note(usize),
    Row(usize, Item),
}

impl Line {
    /// The file and diff row it stands for, which keeps the reader's place when the lines are picked again.
    fn at(self) -> (usize, usize) {
        match self {
            Line::Space(f) | Line::Head(f) | Line::Note(f) => (f, 0),
            Line::Row(f, it) => (f, it.row()),
        }
    }
}

pub struct View {
    /// The commits compared, A's then B's.
    pair: (String, String),
    files: Vec<Change>,
    /// Each file's diff; None until it arrives through `rx`, top file first.
    bodies: Vec<Option<Body>>,
    rx: Receiver<(usize, Body)>,
    /// Horizontal scroll, shared by both sides and every file.
    x: f32,
    /// The rows drawn, and the mode they were picked for; None when they need picking again.
    lines: Vec<Line>,
    mode: Option<Mode>,
    /// The line at the top of the view, and the vertical scroll that put it there.
    first: usize,
    offset: f32,
    /// A file to bring to the top, asked for in C.
    jump: Option<usize>,
}

impl View {
    pub fn new(pair: (String, String), files: Vec<Change>, rx: Receiver<(usize, Body)>) -> Self {
        let bodies = files.iter().map(|_| None).collect();
        Self { pair, files, bodies, rx, x: 0.0, lines: Vec::new(), mode: None, first: 0, offset: 0.0, jump: None }
    }

    /// The file at the top of the view.
    fn top(&self) -> Option<&Change> {
        self.files.get(self.lines.get(self.first).map_or(0, |l| l.at().0))
    }

    pub fn file(&self) -> Option<&str> {
        self.top().map(|c| c.path.as_str())
    }

    /// Brings the file at `path` to the top, if the view holds it.
    pub fn jump(&mut self, path: &str) {
        self.jump = self.files.iter().position(|c| c.path == path);
    }

    /// How many files it holds, and how many have arrived.
    #[cfg(test)]
    pub fn loaded(&self) -> (usize, usize) {
        (self.files.len(), self.bodies.iter().flatten().count())
    }

    pub fn show(&mut self, ui: &mut egui::Ui, mode: Mode) {
        let mut goal = self.jump.take().map(|f| (f, 0));
        while let Ok((f, body)) = self.rx.try_recv() {
            // A file shown alone opens a little above its first change.
            if let (1, Ok(d)) = (self.files.len(), &body) {
                goal = Some((0, d.rows.iter().position(|r| r.kind != Kind::Same).unwrap_or(0).saturating_sub(3)));
            }
            self.bodies[f] = Some(body);
            self.mode = None;
        }
        let v = ui.visuals().clone();
        let mono = TextStyle::Monospace.resolve(ui.style());
        // Both sides are named after the file at the top.
        let Some(c) = self.top() else {
            ui.weak("No differences.");
            return;
        };
        let (a, b) = (&self.pair.0[..8], &self.pair.1[..8]);
        let old = c.blobs[0].is_some().then(|| c.old.as_ref().unwrap_or(&c.path));
        let new = c.blobs[1].is_some().then_some(&c.path);
        let titles = [(old, a), (new, b)].map(|(path, rev)| match path {
            Some(p) => format!("{p} @ {rev}"),
            None => format!("(not in {rev})"),
        });
        ui.columns(2, |cols| {
            for ((col, title), (sign, color)) in cols.iter_mut().zip(&titles).zip([("-", RED), ("+", GREEN)]) {
                let mut job = LayoutJob::default();
                job.append(sign, 0.0, TextFormat::simple(mono.clone(), color));
                job.append(title, 8.0, TextFormat::simple(mono.clone(), v.text_color()));
                col.add(egui::Label::new(job).truncate());
            }
        });
        ui.separator();
        let loaded = || self.bodies.iter().flatten().flatten();
        let char_w = ui.ctx().fonts_mut(|f| f.glyph_width(&mono, '0'));
        let row_h = ui.text_style_height(&TextStyle::Monospace) + 3.0;
        let longest = loaded().map(|d| d.old.len().max(d.new.len())).max().unwrap_or(0);
        let digits = longest.max(1).ilog10() as f32 + 1.0;
        let gutter = char_w * digits + 16.0;
        let tint = |c: Color32, a: u8| Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), a);
        let ink = if v.dark_mode { Color32::from_gray(215) } else { Color32::from_gray(25) };
        let filler = Color32::from_black_alpha(if v.dark_mode { 60 } else { 14 });
        let inline = mode == Mode::Inline;

        // Wheel x, or shift+wheel, scrolls both sides; the vertical scroll area leaves that axis alone.
        let text_w = if inline { ui.available_width() - 2.0 * gutter } else { ui.available_width() / 2.0 - gutter };
        let width = loaded().map(|d| d.width).max().unwrap_or(0);
        let max_x = (width as f32 * char_w - text_w + 2.0 * char_w).max(0.0);
        if ui.rect_contains_pointer(ui.available_rect_before_wrap()) {
            self.x -= ui.input_mut(|i| std::mem::take(&mut i.smooth_scroll_delta.x));
        }
        self.x = self.x.clamp(0.0, max_x);

        let mut area = egui::ScrollArea::vertical().auto_shrink(false);
        if self.mode != Some(mode) {
            let top = self.lines.get(self.first).map_or((0, 0), |l| l.at());
            let many = self.files.len() > 1;
            self.lines.clear();
            for (f, body) in self.bodies.iter().enumerate() {
                if many {
                    self.lines.extend((f > 0).then_some(Line::Space(f)));
                    self.lines.push(Line::Head(f));
                }
                match body {
                    Some(Ok(d)) if !d.rows.is_empty() => self.lines.extend(diff::items(d, mode).into_iter().map(|it| Line::Row(f, it))),
                    _ => self.lines.push(Line::Note(f)),
                }
            }
            // After a mode change, or a file arriving above, the reader stays on the line they were reading.
            let at = self.lines.iter().position(|l| l.at() >= top).unwrap_or(0);
            area = area.vertical_scroll_offset(self.offset + (at as f32 - self.first as f32) * row_h);
            self.first = at;
            self.mode = Some(mode);
        }
        if let Some(goal) = goal {
            self.first = self.lines.iter().position(|l| l.at() >= goal).unwrap_or(0);
            area = area.vertical_scroll_offset(self.first as f32 * row_h);
        }
        let n = self.lines.len();
        // Room below a short last file, so that jumping to it still brings it to the top.
        let last = self.lines.iter().rposition(|l| matches!(l, Line::Space(_)));
        let pad = last.map_or(0, |i| (i + (ui.available_height() / row_h).ceil() as usize).saturating_sub(n));

        // One line of a file in `cell`: its numbers in their gutters, then the text with the changed words marked.
        let x = self.x;
        let line = |ui: &egui::Ui, cell: Rect, nums: &[Option<usize>], s: &str, words: &[Range<usize>], color: Option<Color32>| {
            let p = ui.painter_at(cell);
            if let Some(c) = color {
                p.rect_filled(cell, 0.0, tint(c, 30));
            }
            for (k, n) in nums.iter().enumerate() {
                if let Some(n) = n {
                    let at = cell.min + vec2((k + 1) as f32 * gutter - 8.0, 1.0);
                    p.text(at, Align2::RIGHT_TOP, n + 1, mono.clone(), v.weak_text_color());
                }
            }
            let format = |background| TextFormat { font_id: mono.clone(), color: ink, background, ..Default::default() };
            let mut job = LayoutJob::default();
            let mut at = 0;
            for w in words {
                job.append(&s[at..w.start], 0.0, format(Color32::TRANSPARENT));
                job.append(&s[w.clone()], 0.0, format(tint(color.unwrap_or(ink), 90)));
                at = w.end;
            }
            job.append(&s[at..], 0.0, format(Color32::TRANSPARENT));
            let text = Rect::from_min_max(cell.min + vec2(nums.len() as f32 * gutter, 0.0), cell.max);
            ui.painter_at(text).galley(text.min + vec2(-x, 1.0), p.layout_job(job), ink);
        };

        // Rows touch, so the change colours form solid blocks.
        ui.spacing_mut().item_spacing.y = 0.0;
        // egui's rubber band judges an offset set here against the last frame's height, and springs back from one past it
        // when the lines just grew.
        ui.spacing_mut().scroll.kinetic.rubber_band = false;
        let out = area.show_rows(ui, row_h, n + pad, |ui, range| {
            for &l in &self.lines[range.start.min(n)..range.end.min(n)] {
                let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), row_h), Sense::hover());
                match l {
                    Line::Space(_) => {}
                    Line::Head(f) => {
                        let c = &self.files[f];
                        let p = ui.painter_at(rect);
                        p.rect_filled(rect, 0.0, v.widgets.inactive.bg_fill);
                        let path = match &c.old {
                            Some(old) => format!("{old} → {}", c.path),
                            None => c.path.clone(),
                        };
                        p.text(rect.left_center() + vec2(8.0, 0.0), Align2::LEFT_CENTER, path, mono.clone(), v.strong_text_color());
                        if let Some((added, removed)) = c.lines {
                            let mut job = LayoutJob::default();
                            job.append(&format!("+{added}"), 0.0, TextFormat::simple(mono.clone(), GREEN));
                            job.append(&format!("-{removed}"), 6.0, TextFormat::simple(mono.clone(), RED));
                            let counts = p.layout_job(job);
                            p.galley(rect.right_center() - vec2(8.0 + counts.size().x, counts.size().y / 2.0), counts, ink);
                        }
                    }
                    Line::Note(f) => {
                        let at = rect.left_center() + vec2(gutter, 0.0);
                        match &self.bodies[f] {
                            None => egui::Spinner::new().paint_at(ui, Rect::from_min_size(at - vec2(0.0, row_h / 2.0), vec2(row_h, row_h))),
                            Some(Err(e)) => _ = ui.painter().text(at, Align2::LEFT_CENTER, e, mono.clone(), v.error_fg_color),
                            Some(Ok(_)) => {
                                _ = ui.painter().text(at, Align2::LEFT_CENTER, "Both sides are empty.", mono.clone(), v.weak_text_color())
                            }
                        }
                    }
                    Line::Row(f, item) => {
                        let Some(Ok(d)) = &self.bodies[f] else { continue };
                        match item {
                            Item::Gap { n, .. } => {
                                ui.painter().rect_filled(rect, 0.0, filler);
                                let label = format!("{n} unchanged line{}", if n == 1 { "" } else { "s" });
                                let font = TextStyle::Small.resolve(ui.style());
                                ui.painter().text(rect.center(), Align2::CENTER_CENTER, label, font, v.weak_text_color());
                            }
                            Item::Half(i, side) => {
                                let r = &d.rows[i];
                                let (nums, s, color) = match side {
                                    0 => ([r.old, None], &d.old[r.old.unwrap_or(0)], RED),
                                    _ => ([None, r.new], &d.new[r.new.unwrap_or(0)], GREEN),
                                };
                                line(ui, rect, &nums, s, &r.words[side], Some(color));
                            }
                            Item::Pair(i) if inline => {
                                let r = &d.rows[i];
                                line(ui, rect, &[r.old, r.new], &d.new[r.new.unwrap_or(0)], &[], None);
                            }
                            Item::Pair(i) => {
                                let r = &d.rows[i];
                                let half = rect.width() / 2.0;
                                for (side, (lines, n, color)) in [(&d.old, r.old, RED), (&d.new, r.new, GREEN)].into_iter().enumerate() {
                                    let cell = Rect::from_min_size(rect.min + vec2(side as f32 * half, 0.0), vec2(half, row_h));
                                    match n {
                                        Some(n) => {
                                            line(ui, cell, &[Some(n)], &lines[n], &r.words[side], (r.kind != Kind::Same).then_some(color))
                                        }
                                        None => drop(ui.painter_at(cell).rect_filled(cell, 0.0, filler)),
                                    }
                                }
                                ui.painter().vline(rect.center().x, rect.y_range(), v.widgets.noninteractive.bg_stroke);
                            }
                        }
                    }
                }
            }
        });
        self.offset = out.state.clamped_offset().y;
        // Rounded, not floored as `show_rows` does: a jump's offset can land a hair short of its line.
        self.first = ((self.offset / row_h).round() as usize).min(n - 1);
    }
}

/// A toolbar button showing an icon `draw` paints into a 20×14 rect, as egui's fonts have none that fit. True when clicked.
fn icon_button(ui: &mut egui::Ui, on: bool, name: &str, tip: &str, draw: impl FnOnce(&Painter, Rect, Color32)) -> bool {
    let (rect, resp) = ui.allocate_exact_size(vec2(32.0, 24.0), Sense::click());
    let on = on || resp.clicked();
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::RadioButton, true, on, name));
    let v = ui.style().interact_selectable(&resp, on);
    if on || resp.hovered() {
        ui.painter().rect_filled(rect, 4.0, v.weak_bg_fill);
    }
    draw(ui.painter(), Rect::from_center_size(rect.center(), vec2(20.0, 14.0)), v.fg_stroke.color);
    resp.on_hover_text(tip).clicked()
}

/// A line of text across a page at height y, inset from its sides.
fn text_line(p: &Painter, page: Rect, y: f32, color: Color32) {
    p.line_segment([pos2(page.left() + 3.0, page.top() + y), pos2(page.right() - 3.0, page.top() + y)], Stroke::new(1.0, color));
}

/// The toolbar's switch between every file and one.
pub fn layout_buttons(ui: &mut egui::Ui, layout: &mut Layout) {
    for (l, name, tip) in [
        (Layout::All, "All files", "Every changed file, one after another"),
        (Layout::One, "One file", "Only the file picked in the list, for diffs too large to show whole"),
    ] {
        let draw = |p: &Painter, icon: Rect, fg: Color32| {
            let page = |y: f32, h: f32| {
                let page = Rect::from_min_size(pos2(icon.center().x - 6.0, icon.top() + y), vec2(12.0, h));
                p.rect_stroke(page, 1.0, Stroke::new(1.0, fg), StrokeKind::Inside);
                page
            };
            // Two pages one under the other, or a single one.
            match l {
                Layout::All => {
                    for y in [0.0, 8.0] {
                        text_line(p, page(y, 6.0), 3.0, fg);
                    }
                }
                Layout::One => {
                    let one = page(0.0, 14.0);
                    for y in [4.0, 7.0, 10.0] {
                        text_line(p, one, y, fg);
                    }
                }
            }
        };
        if icon_button(ui, *layout == l, name, tip, draw) {
            *layout = l;
        }
    }
}

/// The toolbar's switch between modes.
pub fn mode_buttons(ui: &mut egui::Ui, mode: &mut Mode) {
    for (m, name, tip) in [
        (Mode::Full, "Side by side", "Side by side, the whole file"),
        (Mode::Chunks, "Changes only", "Side by side, only the changes and 4 lines around each"),
        (Mode::Inline, "Inline", "One column: removed lines above the lines that replace them"),
    ] {
        let draw = |p: &Painter, icon: Rect, fg: Color32| match m {
            Mode::Inline => {
                p.rect_stroke(icon, 1.0, Stroke::new(1.0, fg), StrokeKind::Inside);
                for (y, color) in [(4.0, fg), (7.0, RED), (10.0, GREEN)] {
                    text_line(p, icon, y, color);
                }
            }
            _ => {
                for page in
                    [Rect::from_min_size(icon.min, vec2(9.0, 14.0)), Rect::from_min_size(icon.min + vec2(11.0, 0.0), vec2(9.0, 14.0))]
                {
                    p.rect_stroke(page, 1.0, Stroke::new(1.0, fg), StrokeKind::Inside);
                    if m == Mode::Full {
                        for y in [4.0, 7.0, 10.0] {
                            text_line(p, page, y, fg);
                        }
                    } else {
                        // Lines with a skipped stretch between them.
                        for y in [3.5, 10.5] {
                            text_line(p, page, y, fg);
                        }
                        for dx in [-1.5, 1.5] {
                            p.circle_filled(pos2(page.center().x + dx, page.top() + 7.0), 0.7, fg);
                        }
                    }
                }
            }
        };
        if icon_button(ui, *mode == m, name, tip, draw) {
            *mode = m;
        }
    }
}
