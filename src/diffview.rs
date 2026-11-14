//! Draws columns D and E: one widget painting both sides of each row, so the sides can't drift apart. Inline, the two
//! columns become one.

use std::ops::Range;

use eframe::egui::{self, Align2, Color32, Rect, Sense, Stroke, StrokeKind, TextFormat, TextStyle, pos2, vec2};
use egui::text::LayoutJob;

use crate::diff::{self, Diff, Item, Kind, Mode};

const RED: Color32 = Color32::from_rgb(0xf0, 0x50, 0x50);
const GREEN: Color32 = Color32::from_rgb(0x40, 0xc0, 0x50);

pub struct View {
    diff: Diff,
    /// What each side shows: path and commit, old first.
    titles: [String; 2],
    /// Horizontal scroll, shared by both sides.
    x: f32,
    /// The rows drawn, and the mode they were picked for; None until first shown.
    items: Vec<Item>,
    mode: Option<Mode>,
    /// The diff row at the top of the view, so a mode change keeps the reader's place.
    top: usize,
}

impl View {
    pub fn new(diff: Diff, titles: [String; 2]) -> Self {
        Self { diff, titles, x: 0.0, items: Vec::new(), mode: None, top: 0 }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, mode: Mode) {
        let v = ui.visuals().clone();
        let mono = TextStyle::Monospace.resolve(ui.style());
        ui.columns(2, |cols| {
            for ((col, title), (sign, color)) in cols.iter_mut().zip(&self.titles).zip([("-", RED), ("+", GREEN)]) {
                let mut job = LayoutJob::default();
                job.append(sign, 0.0, TextFormat::simple(mono.clone(), color));
                job.append(title, 8.0, TextFormat::simple(mono.clone(), v.text_color()));
                col.add(egui::Label::new(job).truncate());
            }
        });
        ui.separator();
        let d = &self.diff;
        if d.rows.is_empty() {
            ui.weak("Both sides are empty.");
            return;
        }
        let char_w = ui.ctx().fonts_mut(|f| f.glyph_width(&mono, '0'));
        let row_h = ui.text_style_height(&TextStyle::Monospace) + 3.0;
        let digits = d.old.len().max(d.new.len()).max(1).ilog10() as f32 + 1.0;
        let gutter = char_w * digits + 16.0;
        let tint = |c: Color32, a: u8| Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), a);
        let ink = if v.dark_mode { Color32::from_gray(215) } else { Color32::from_gray(25) };
        let filler = Color32::from_black_alpha(if v.dark_mode { 60 } else { 14 });
        let inline = mode == Mode::Inline;

        // Wheel x, or shift+wheel, scrolls both sides; the vertical scroll area leaves that axis alone.
        let text_w = if inline { ui.available_width() - 2.0 * gutter } else { ui.available_width() / 2.0 - gutter };
        let max_x = (d.width as f32 * char_w - text_w + 2.0 * char_w).max(0.0);
        if ui.rect_contains_pointer(ui.available_rect_before_wrap()) {
            self.x -= ui.input_mut(|i| std::mem::take(&mut i.smooth_scroll_delta.x));
        }
        self.x = self.x.clamp(0.0, max_x);

        let mut area = egui::ScrollArea::vertical().auto_shrink(false);
        if self.mode != Some(mode) {
            self.items = diff::items(d, mode);
            // Shown for the first time: a little above the first change. After a mode change: where the reader was.
            let target = match self.mode {
                None => d.rows.iter().position(|r| r.kind != Kind::Same).unwrap_or(0).saturating_sub(3),
                Some(_) => self.top,
            };
            let at = self.items.iter().position(|it| it.row() >= target).unwrap_or(0);
            area = area.vertical_scroll_offset(at as f32 * row_h);
            self.mode = Some(mode);
        }

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
        let mut top = self.top;
        area.show_rows(ui, row_h, self.items.len(), |ui, range| {
            top = self.items.get(range.start).map_or(top, |it| it.row());
            for &item in &self.items[range] {
                let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), row_h), Sense::hover());
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
                                Some(n) => line(ui, cell, &[Some(n)], &lines[n], &r.words[side], (r.kind != Kind::Same).then_some(color)),
                                None => drop(ui.painter_at(cell).rect_filled(cell, 0.0, filler)),
                            }
                        }
                        ui.painter().vline(rect.center().x, rect.y_range(), v.widgets.noninteractive.bg_stroke);
                    }
                }
            }
        });
        self.top = top;
    }
}

/// The toolbar's switch between modes: three painted icons, as egui's fonts have none that fit.
pub fn mode_buttons(ui: &mut egui::Ui, mode: &mut Mode) {
    for (m, name, tip) in [
        (Mode::Full, "Side by side", "Side by side, the whole file"),
        (Mode::Chunks, "Changes only", "Side by side, only the changes and 4 lines around each"),
        (Mode::Inline, "Inline", "One column: removed lines above the lines that replace them"),
    ] {
        let (rect, resp) = ui.allocate_exact_size(vec2(32.0, 24.0), Sense::click());
        if resp.clicked() {
            *mode = m;
        }
        let on = *mode == m;
        resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::RadioButton, true, on, name));
        let v = ui.style().interact_selectable(&resp, on);
        let p = ui.painter();
        if on || resp.hovered() {
            p.rect_filled(rect, 4.0, v.weak_bg_fill);
        }
        let fg = v.fg_stroke.color;
        let icon = Rect::from_center_size(rect.center(), vec2(20.0, 14.0));
        // A text line across a page at height y, inset from its sides.
        let text = |page: Rect, y: f32, color: Color32| {
            p.line_segment([pos2(page.left() + 3.0, page.top() + y), pos2(page.right() - 3.0, page.top() + y)], Stroke::new(1.0, color))
        };
        match m {
            Mode::Inline => {
                p.rect_stroke(icon, 1.0, Stroke::new(1.0, fg), StrokeKind::Inside);
                for (y, color) in [(4.0, fg), (7.0, RED), (10.0, GREEN)] {
                    text(icon, y, color);
                }
            }
            _ => {
                for page in
                    [Rect::from_min_size(icon.min, vec2(9.0, 14.0)), Rect::from_min_size(icon.min + vec2(11.0, 0.0), vec2(9.0, 14.0))]
                {
                    p.rect_stroke(page, 1.0, Stroke::new(1.0, fg), StrokeKind::Inside);
                    if m == Mode::Full {
                        for y in [4.0, 7.0, 10.0] {
                            text(page, y, fg);
                        }
                    } else {
                        // Lines with a skipped stretch between them.
                        for y in [3.5, 10.5] {
                            text(page, y, fg);
                        }
                        for dx in [-1.5, 1.5] {
                            p.circle_filled(pos2(page.center().x + dx, page.top() + 7.0), 0.7, fg);
                        }
                    }
                }
            }
        }
        resp.on_hover_text(tip);
    }
}
