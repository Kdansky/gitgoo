//! Repositories opened before: kept between runs, listed on the start screen and in the "Open recent" dialog.

use std::path::{Path, PathBuf};

use eframe::egui::{self, Align2, Rect, Sense, TextStyle, pos2, vec2};

const KEY: &str = "recent";
const KEEP: usize = 20;

/// A repository opened before.
struct Recent {
    path: PathBuf,
    /// Still there: folders get moved and deleted between runs.
    here: bool,
}

/// Most recently opened first.
pub struct Recents(Vec<Recent>);

/// What the start screen, the toolbar or the dialog asks of the app.
pub enum Ask {
    /// Bring up the dialog.
    Show,
    Close,
    Open(PathBuf),
    Forget(PathBuf),
    ForgetMissing,
    /// Pick a folder not in the list.
    Browse,
}

impl Recents {
    pub fn load(storage: Option<&dyn eframe::Storage>) -> Self {
        let paths: Vec<PathBuf> = storage.and_then(|s| eframe::get_value(s, KEY)).unwrap_or_default();
        let mut r = Self(paths.into_iter().map(|path| Recent { path, here: false }).collect());
        r.check();
        r
    }

    pub fn save(&self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, KEY, &self.0.iter().map(|r| &r.path).collect::<Vec<_>>());
    }

    /// Looks again at which are still there. A repository's root holds `.git`: a folder, or a file in a worktree.
    pub fn check(&mut self) {
        for r in &mut self.0 {
            r.here = r.path.join(".git").exists();
        }
    }

    pub fn remember(&mut self, path: &Path) {
        self.forget(path);
        self.0.insert(0, Recent { path: path.to_owned(), here: true });
        self.0.truncate(KEEP);
    }

    pub fn forget(&mut self, path: &Path) {
        self.0.retain(|r| r.path != path);
    }

    pub fn forget_missing(&mut self) {
        self.0.retain(|r| r.here);
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// One row per repository; `current` is the one open. A missing one can only be forgotten.
pub fn list(ui: &mut egui::Ui, recents: &Recents, current: Option<&Path>) -> Option<Ask> {
    let mut ask = None;
    let v = ui.visuals().clone();
    let body = TextStyle::Body.resolve(ui.style());
    let line_h = ui.text_style_height(&TextStyle::Body);
    let (row_h, w) = (2.0 * line_h + 10.0, ui.available_width().min(600.0));
    for r in &recents.0 {
        let name = r.path.file_name().map_or_else(|| r.path.display().to_string(), |n| n.to_string_lossy().into_owned());
        let (rect, resp) = ui.allocate_exact_size(vec2(w, row_h), Sense::click());
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::ListItem, r.here, &name));
        let open = current == Some(r.path.as_path());
        let hovered = ui.rect_contains_pointer(rect);
        let state = ui.style().interact_selectable(&resp, open);
        if r.here && (open || hovered) {
            ui.painter().rect_filled(rect, 3.0, state.weak_bg_fill);
        }
        // Text stops short of where the Forget button goes.
        let p = ui.painter_at(rect.with_max_x(rect.right() - 80.0));
        let (ink, below, below_ink) = match r.here {
            true => (state.text_color(), r.path.display().to_string(), v.weak_text_color()),
            false => (v.weak_text_color(), format!("missing: {}", r.path.display()), v.error_fg_color),
        };
        p.text(rect.min + vec2(8.0, 5.0), Align2::LEFT_TOP, &name, body.clone(), ink);
        p.text(rect.min + vec2(8.0, 5.0 + line_h), Align2::LEFT_TOP, below, body.clone(), below_ink);
        // On the row under the pointer, and always on a missing one.
        if hovered || !r.here {
            let button = Rect::from_center_size(pos2(rect.right() - 40.0, rect.center().y), vec2(64.0, line_h + 6.0));
            if ui.put(button, egui::Button::new("Forget")).clicked() {
                ask = Some(Ask::Forget(r.path.clone()));
            }
        }
        if resp.clicked() && r.here && ask.is_none() {
            ask = Some(if open { Ask::Close } else { Ask::Open(r.path.clone()) });
        }
    }
    ask
}

/// The "Open recent" dialog. `error` says why the last folder didn't open.
pub fn dialog(ctx: &egui::Context, recents: &Recents, current: Option<&Path>, error: Option<&str>) -> Option<Ask> {
    let mut ask = None;
    let modal = egui::Modal::new(egui::Id::unique("recent")).show(ctx, |ui| {
        ui.set_width(600.0);
        ui.heading("Open recent");
        ui.separator();
        if let Some(e) = error {
            ui.colored_label(ui.visuals().error_fg_color, e);
            ui.add_space(6.0);
        }
        if recents.is_empty() {
            ui.weak("Nothing opened yet.");
        }
        egui::ScrollArea::vertical().max_height(ctx.content_rect().height() * 0.6).show(ui, |ui| ask = list(ui, recents, current));
        ui.separator();
        ui.horizontal(|ui| {
            if ui.button("Open another folder…").clicked() {
                ask = Some(Ask::Browse);
            }
            let missing = recents.0.iter().filter(|r| !r.here).count();
            if missing > 0 && ui.button(format!("Forget {missing} missing")).clicked() {
                ask = Some(Ask::ForgetMissing);
            }
        });
    });
    ask.or(modal.should_close().then_some(Ask::Close))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// Storage in memory, standing in for eframe's file.
    #[derive(Default)]
    struct Mem(HashMap<String, String>);

    impl eframe::Storage for Mem {
        fn get_string(&self, key: &str) -> Option<String> {
            self.0.get(key).cloned()
        }
        fn set_string(&mut self, key: &str, value: String) {
            self.0.insert(key.to_owned(), value);
        }
        fn remove_string(&mut self, key: &str) {
            self.0.remove(key);
        }
        fn flush(&mut self) {}
    }

    #[test]
    fn newest_first_without_repeats_and_kept_between_runs() {
        let here = std::env::temp_dir().join(format!("gitgoo-{}-recent", std::process::id()));
        std::fs::create_dir_all(here.join(".git")).unwrap();
        let gone = PathBuf::from("/nowhere/gitgoo-gone");
        let mut r = Recents(Vec::new());
        r.remember(&gone);
        r.remember(&here);
        r.remember(&gone);
        assert_eq!(r.0.iter().map(|r| r.path.clone()).collect::<Vec<_>>(), [gone.clone(), here.clone()]);
        let mut mem = Mem::default();
        r.save(&mut mem);
        let mut back = Recents::load(Some(&mem));
        assert_eq!(back.0.iter().map(|r| (r.path.clone(), r.here)).collect::<Vec<_>>(), [(gone, false), (here.clone(), true)]);
        back.forget_missing();
        assert_eq!(back.0.len(), 1);
        for i in 0..KEEP + 5 {
            back.remember(Path::new(&format!("/r{i}")));
        }
        assert_eq!(back.0.len(), KEEP);
        std::fs::remove_dir_all(here).unwrap();
    }
}
