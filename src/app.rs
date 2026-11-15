use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};

use eframe::egui::{self, Color32, Sense, TextFormat, TextStyle, vec2};
use egui::text::{LayoutJob, TextWrapping};

use crate::diffview::{self, Layout};
use crate::git::{self, Commit};
use crate::recent::{self, Ask, Recents};
use crate::{diff, graph, picker};

const PAGE: usize = 500;

#[derive(PartialEq)]
enum Paging {
    More,
    Loading,
    Done,
}

/// One history column, A or B.
struct Side {
    branch: String,
    commits: Vec<Commit>,
    paging: Paging,
    error: Option<String>,
    picked: Option<String>,
}

/// What a background git call fills in.
enum Load<T> {
    /// Nothing picked to load.
    Waiting,
    Loading,
    Ready(T),
    Failed(String),
}

/// An open repository. Its background work reports on its own channel, so results for a repository since replaced
/// go nowhere.
struct Repo {
    path: PathBuf,
    sides: [Side; 2],
    /// The commits `files` compares: A's pick, B's pick.
    pair: Option<(String, String)>,
    files: Load<Vec<git::Change>>,
    /// The file picked in C. Showing all files, the one at the top of the view.
    file: Option<String>,
    layout: Layout,
    /// The commits `view` compares, and the one file it shows, or None for all of them.
    shown: Option<((String, String), Option<String>)>,
    view: Option<diffview::View>,
    picker: Option<picker::Picker>,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
}

/// A background result for an open repository.
enum Msg {
    Page { side: usize, branch: String, skip: usize, page: git::Result<Vec<Commit>>, keep: bool },
    Graph(git::Result<graph::Graph>),
    Files { pair: (String, String), files: git::Result<Vec<git::Change>> },
}

impl Repo {
    fn new(path: PathBuf) -> Self {
        let head = git::head(&path);
        let side = || Side { branch: head.clone(), commits: Vec::new(), paging: Paging::More, error: None, picked: None };
        let (tx, rx) = channel();
        Self {
            sides: [side(), side()],
            path,
            pair: None,
            files: Load::Waiting,
            file: None,
            layout: Layout::All,
            shown: None,
            view: None,
            picker: None,
            tx,
            rx,
        }
    }

    /// Reloads the file list when the picked commits have changed since the last call.
    fn compare(&mut self, ctx: &egui::Context) {
        let pair = self.sides[0].picked.clone().zip(self.sides[1].picked.clone());
        if pair == self.pair {
            return;
        }
        self.pair = pair.clone();
        let Some((a, b)) = pair else {
            self.files = Load::Waiting;
            return;
        };
        self.files = Load::Loading;
        let (tx, ctx, repo) = (self.tx.clone(), ctx.clone(), self.path.clone());
        std::thread::spawn(move || {
            let files = git::changes(&repo, &a, &b);
            _ = tx.send(Msg::Files { pair: (a, b), files });
            ctx.request_repaint();
        });
    }

    /// Rebuilds columns D and E when the picked commits or the layout changed, or, showing one file, the picked file.
    fn show_files(&mut self, ctx: &egui::Context) {
        // Until the new list arrives it's unknown whether the picked file still differs.
        let Load::Ready(list) = &self.files else { return };
        let key = match self.layout {
            Layout::All => self.pair.clone().map(|p| (p, None)),
            Layout::One => self.pair.clone().zip(self.file.clone()).map(|(p, f)| (p, Some(f))),
        };
        if key == self.shown {
            return;
        }
        self.shown = key.clone();
        let Some((pair, one)) = key else {
            self.view = None;
            return;
        };
        let files: Vec<git::Change> = list.iter().filter(|c| one.as_ref().is_none_or(|f| f == &c.path)).cloned().collect();
        let blobs: Vec<_> = files.iter().map(|c| c.blobs.clone()).collect();
        // The view shows each file as it arrives, top first. The channel goes with the view, so the worker for a replaced
        // one stops at its next file.
        let (tx, rx) = channel();
        let (ctx, repo) = (ctx.clone(), self.path.clone());
        std::thread::spawn(move || {
            let mut cat = git::Cat::new(&repo);
            for (i, ids) in blobs.iter().enumerate() {
                let mut text = |id: &Option<String>| match (&mut cat, id) {
                    (Err(e), _) => Err(e.clone()),
                    (Ok(_), None) => Ok(Some(String::new())),
                    (Ok(cat), Some(id)) => cat.blob(id).map(diff::text),
                };
                let body = match (text(&ids[0]), text(&ids[1])) {
                    (Err(e), _) | (_, Err(e)) => Err(e),
                    (Ok(Some(o)), Ok(Some(n))) => Ok(diff::diff(&o, &n)),
                    _ => Err("binary file not supported".to_owned()),
                };
                if tx.send((i, body)).is_err() {
                    return;
                }
                ctx.request_repaint();
            }
        });
        let mut view = diffview::View::new(pair, files, rx);
        // Showing all files, a new comparison or a switch from one file opens at the file the reader was on.
        if let Some(f) = self.file.as_ref().filter(|_| one.is_none()) {
            view.jump(f);
        }
        self.view = Some(view);
    }

    fn load(&mut self, side: usize, ctx: &egui::Context) {
        let s = &mut self.sides[side];
        s.paging = Paging::Loading;
        let (tx, ctx, repo, branch, skip) = (self.tx.clone(), ctx.clone(), self.path.clone(), s.branch.clone(), s.commits.len());
        let picked = s.picked.clone().filter(|_| skip == 0);
        std::thread::spawn(move || {
            let page = git::log(&repo, &branch, skip, PAGE);
            // On a newly picked branch, the column's commit stays picked only if the branch contains it.
            let keep = picked.is_none_or(|c| git::contains(&repo, &branch, &c));
            _ = tx.send(Msg::Page { side, branch, skip, page, keep });
            ctx.request_repaint();
        });
    }

    fn pick_branch(&mut self, side: usize, ctx: &egui::Context) {
        self.picker = Some(picker::Picker::new(side));
        let (tx, ctx, repo) = (self.tx.clone(), ctx.clone(), self.path.clone());
        std::thread::spawn(move || {
            _ = tx.send(Msg::Graph(graph::load(&repo)));
            ctx.request_repaint();
        });
    }

    /// Points a history column at another branch. Its pick survives until the first page shows whether it's in there.
    fn switch(&mut self, side: usize, branch: String) {
        let s = &mut self.sides[side];
        if s.branch != branch {
            *s = Side { branch, picked: s.picked.take(), commits: Vec::new(), paging: Paging::More, error: None };
        }
    }

    fn receive(&mut self) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::Page { side, branch, skip, page, keep } => {
                    let s = &mut self.sides[side];
                    // The column has since switched branch, or this page already arrived.
                    if s.branch != branch || s.commits.len() != skip {
                        continue;
                    }
                    if !keep {
                        s.picked = None;
                    }
                    match page {
                        Ok(page) => {
                            s.paging = if page.len() < PAGE { Paging::Done } else { Paging::More };
                            s.commits.extend(page);
                        }
                        Err(e) => {
                            s.error = Some(e);
                            s.paging = Paging::Done;
                        }
                    }
                }
                Msg::Files { pair, files } => {
                    // A later pick superseded this comparison.
                    if self.pair.as_ref() != Some(&pair) {
                        continue;
                    }
                    self.files = match files {
                        Ok(list) => {
                            // Keep the picked file only if it still differs.
                            if !list.iter().any(|c| self.file.as_ref() == Some(&c.path)) {
                                self.file = None;
                            }
                            Load::Ready(list)
                        }
                        Err(e) => Load::Failed(e),
                    };
                }
                Msg::Graph(graph) => {
                    if let Some(p) = &mut self.picker {
                        p.graph = Some(graph);
                    }
                }
            }
        }
    }
}

pub struct App {
    /// None until a repository is open; the start screen shows meanwhile.
    repo: Option<Repo>,
    recents: Recents,
    /// The "Open recent" dialog is up.
    recent_open: bool,
    /// The recent list changed, and should reach the disk now rather than at exit.
    dirty: bool,
    /// Why the last folder didn't open.
    error: Option<String>,
    picking: bool,
    /// How the diff view lays out a diff, whichever file it shows.
    mode: diff::Mode,
    /// Where the folder picker reports.
    tx: Sender<Option<PathBuf>>,
    rx: Receiver<Option<PathBuf>>,
}

impl App {
    /// Opens `dir` if given, else the folder picker: double-clicking the binary passes no arguments.
    pub fn new(cc: &eframe::CreationContext, dir: Option<PathBuf>) -> Self {
        let (tx, rx) = channel();
        let mode = cc.storage.and_then(|s| eframe::get_value(s, "mode")).unwrap_or(diff::Mode::Full);
        let recents = Recents::load(cc.storage);
        let mut app = Self { repo: None, recents, recent_open: false, dirty: false, error: None, picking: false, mode, tx, rx };
        match dir {
            Some(dir) => app.open(dir, &cc.egui_ctx),
            None => app.pick(rfd::AsyncFileDialog::new().set_parent(cc), &cc.egui_ctx),
        }
        app
    }

    /// The dialog is created here on the main thread, as macOS requires, and awaited on a worker so the window keeps
    /// responding; a blocked window gets greyed out as hung while the user browses.
    fn pick(&mut self, dialog: rfd::AsyncFileDialog, ctx: &egui::Context) {
        self.picking = true;
        let dir = dialog.set_title("Open repository").pick_folder();
        let (tx, ctx) = (self.tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            _ = tx.send(pollster::block_on(dir).map(|d| d.path().to_owned()));
            ctx.request_repaint();
        });
    }

    fn open(&mut self, dir: PathBuf, ctx: &egui::Context) {
        let path = match git::root(&dir) {
            Ok(path) => path,
            Err(e) => {
                self.error = Some(format!("{}\n{}", dir.display(), e.lines().next().unwrap_or_default()));
                // With a repository open there's no start screen to say so.
                self.recent_open |= self.repo.is_some();
                return;
            }
        };
        let name = path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(format!("{name} — gitgoo")));
        self.recents.remember(&path);
        self.dirty = true;
        self.repo = Some(Repo::new(path));
        self.error = None;
        self.recent_open = false;
    }
}

impl eframe::App for App {
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        self.recents.save(storage);
        eframe::set_value(storage, "mode", &self.mode);
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        while let Ok(dir) = self.rx.try_recv() {
            self.picking = false;
            if let Some(dir) = dir {
                self.open(dir, ui.ctx());
            }
        }
        if std::mem::take(&mut self.dirty)
            && let Some(s) = frame.storage_mut()
        {
            eframe::App::save(self, s);
            s.flush();
        }
        let ask = match &mut self.repo {
            None => start(ui, &self.recents, self.picking, self.error.as_deref()),
            Some(repo) => {
                repo.receive();
                let mut ask = None;
                // First, so it spans the window above every column.
                egui::Panel::top("toolbar").show(ui, |ui| {
                    ui.horizontal(|ui| {
                        if ui.button("Open recent").clicked() {
                            ask = Some(Ask::Show);
                        }
                        ui.separator();
                        diffview::layout_buttons(ui, &mut repo.layout);
                        ui.separator();
                        diffview::mode_buttons(ui, &mut self.mode);
                    })
                });
                let mut branch = None;
                for (i, id) in ["a", "b"].into_iter().enumerate() {
                    let more = egui::Panel::left(id).resizable(true).default_size(300.0).min_size(120.0).show(ui, |ui| {
                        let s = &mut repo.sides[i];
                        if header(ui, &s.branch).on_hover_text("Pick a branch").clicked() {
                            branch = Some(i);
                        }
                        ui.separator();
                        history(ui, s)
                    });
                    if more.inner {
                        repo.load(i, ui.ctx());
                    }
                }
                if let Some(i) = branch {
                    repo.pick_branch(i, ui.ctx());
                }
                repo.compare(ui.ctx());
                egui::Panel::left("c").resizable(true).default_size(300.0).min_size(120.0).show(ui, |ui| files(ui, repo));
                repo.show_files(ui.ctx());
                egui::CentralPanel::default().show(ui, |ui| match &mut repo.view {
                    Some(view) => {
                        view.show(ui, self.mode);
                        // C marks the file being read.
                        if repo.file.as_deref() != view.file() {
                            repo.file = view.file().map(str::to_owned);
                        }
                    }
                    None if repo.layout == Layout::One => drop(ui.weak("Pick a file in C.")),
                    None => drop(ui.weak("Pick a commit in A and B.")),
                });
                match repo.picker.as_mut().map(|p| (p.side, p.show(ui.ctx(), &repo.sides[p.side].branch))) {
                    Some((side, picker::Outcome::Picked(branch))) => {
                        repo.picker = None;
                        repo.switch(side, branch);
                    }
                    Some((_, picker::Outcome::Closed)) => repo.picker = None,
                    _ => {}
                }
                if self.recent_open {
                    ask = ask.or(recent::dialog(ui.ctx(), &self.recents, Some(&repo.path), self.error.as_deref()));
                }
                ask
            }
        };
        match ask {
            Some(Ask::Show) => {
                self.recents.check();
                self.error = None;
                self.recent_open = true;
            }
            Some(Ask::Close) => self.recent_open = false,
            Some(Ask::Open(path)) => self.open(path, ui.ctx()),
            Some(Ask::Forget(path)) => {
                self.recents.forget(&path);
                self.dirty = true;
            }
            Some(Ask::ForgetMissing) => {
                self.recents.forget_missing();
                self.dirty = true;
            }
            Some(Ask::Browse) => {
                self.recent_open = false;
                self.pick(rfd::AsyncFileDialog::new().set_parent(&*frame), ui.ctx());
            }
            None => {}
        }
    }
}

/// The screen before any repository is open.
fn start(ui: &mut egui::Ui, recents: &Recents, picking: bool, error: Option<&str>) -> Option<Ask> {
    let mut ask = None;
    egui::CentralPanel::default().show(ui, |ui| {
        ui.vertical_centered(|ui| {
            ui.add_space(ui.available_height() / 5.0);
            ui.heading("gitgoo");
            ui.add_space(8.0);
            if ui.add_enabled(!picking, egui::Button::new("Open repository…")).clicked() {
                ask = Some(Ask::Browse);
            }
            if let Some(e) = error {
                ui.add_space(8.0);
                ui.colored_label(ui.visuals().error_fg_color, e);
            }
            if !recents.is_empty() {
                ui.add_space(24.0);
                ask = ask.take().or(recent::list(ui, recents, None));
            }
        });
    });
    ask
}

/// The branch name heading a history column, with a triangle saying it opens something. Painted, as egui's fonts
/// have no triangle glyph.
fn header(ui: &mut egui::Ui, branch: &str) -> egui::Response {
    let galley = ui.painter().layout_no_wrap(branch.to_owned(), TextStyle::Heading.resolve(ui.style()), ui.visuals().strong_text_color());
    let (rect, resp) = ui.allocate_exact_size(galley.size() + vec2(28.0, 6.0), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, branch));
    let v = ui.style().interact(&resp);
    if resp.hovered() {
        ui.painter().rect_filled(rect, 3.0, v.weak_bg_fill);
    }
    let c = rect.right_center() - vec2(12.0, 0.0);
    let triangle = vec![c + vec2(-5.0, -2.5), c + vec2(5.0, -2.5), c + vec2(0.0, 3.5)];
    ui.painter().add(egui::Shape::convex_polygon(triangle, v.fg_stroke.color, egui::Stroke::NONE));
    ui.painter().galley(rect.min + vec2(4.0, 3.0), galley, v.text_color());
    resp
}

/// Draws one history column. True when it has scrolled near the end of what's loaded and more exists.
fn history(ui: &mut egui::Ui, side: &mut Side) -> bool {
    if let Some(e) = &side.error {
        ui.colored_label(ui.visuals().error_fg_color, e);
        return false;
    }
    let body = TextStyle::Body.resolve(ui.style());
    let mono = TextStyle::Monospace.resolve(ui.style());
    let line_h = ui.text_style_height(&TextStyle::Body);
    let row_h = 2.0 * line_h + 8.0;
    // Keyed by branch, so a switched column starts at the top.
    let area = egui::ScrollArea::vertical().id_salt(&side.branch).auto_shrink(false);
    let out = area.show_rows(ui, row_h, side.commits.len(), |ui, rows| {
        for c in &side.commits[rows.clone()] {
            let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), row_h), Sense::click());
            if resp.clicked() {
                side.picked = Some(c.id.clone());
            }
            let picked = side.picked.as_deref() == Some(c.id.as_str());
            resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::ListItem, true, picked, &c.subject));
            let v = ui.style().interact_selectable(&resp, picked);
            let painter = ui.painter_at(rect);
            if picked || resp.hovered() {
                painter.rect_filled(rect, 2.0, v.weak_bg_fill);
            }
            let weak = if picked { v.text_color() } else { ui.visuals().weak_text_color() };
            let wrap = TextWrapping::truncate_at_width(rect.width() - 12.0);
            let mut top = LayoutJob::simple_singleline(c.subject.clone(), body.clone(), v.text_color());
            top.wrap = wrap.clone();
            let mut bottom = LayoutJob::default();
            bottom.append(&c.id[..8], 0.0, TextFormat::simple(mono.clone(), weak));
            bottom.append(&format!("  {} · {}", c.author, c.when), 0.0, TextFormat::simple(body.clone(), weak));
            bottom.wrap = wrap;
            painter.galley(rect.min + vec2(6.0, 4.0), painter.layout_job(top), Color32::PLACEHOLDER);
            painter.galley(rect.min + vec2(6.0, 4.0 + line_h), painter.layout_job(bottom), Color32::PLACEHOLDER);
            resp.on_hover_ui(|ui| {
                // Messages are wrapped for a 72-column terminal; egui's default tooltip width would wrap them again.
                ui.set_max_width(80.0 * ui.ctx().fonts_mut(|f| f.glyph_width(&mono, '0')));
                ui.label(egui::RichText::new(&c.subject).strong());
                if !c.body.is_empty() {
                    ui.label(egui::RichText::new(&c.body).monospace());
                }
                ui.weak(format!("{} · {}", c.author, c.date));
            });
        }
        rows.end
    });
    side.paging == Paging::More && out.inner + PAGE / 5 >= side.commits.len()
}

fn green(v: &egui::Visuals) -> Color32 {
    if v.dark_mode { Color32::from_rgb(0x6a, 0xc4, 0x6a) } else { Color32::from_rgb(0x2e, 0x7d, 0x32) }
}

/// Lighter than egui's error red, which drowns on the selection highlight.
fn red(v: &egui::Visuals) -> Color32 {
    if v.dark_mode { Color32::from_rgb(0xff, 0x8a, 0x80) } else { Color32::from_rgb(0xc6, 0x28, 0x28) }
}

/// Draws column C: the files that differ between A's and B's picks.
fn files(ui: &mut egui::Ui, repo: &mut Repo) {
    ui.heading("Files");
    ui.separator();
    let list = match &repo.files {
        Load::Ready(list) if !list.is_empty() => list,
        Load::Ready(_) => return drop(ui.weak("No differences.")),
        Load::Waiting => return drop(ui.weak("Pick a commit in A and B.")),
        Load::Loading => return drop(ui.spinner()),
        Load::Failed(e) => return drop(ui.colored_label(ui.visuals().error_fg_color, e)),
    };
    let v = ui.visuals().clone();
    let (added, removed) = list.iter().filter_map(|c| c.lines).fold((0, 0), |(a, r), (x, y)| (a + x, r + y));
    let plural = if list.len() == 1 { "" } else { "s" };
    ui.weak(format!("{} file{plural}, +{added} -{removed}", list.len()));
    let body = TextStyle::Body.resolve(ui.style());
    let mono = TextStyle::Monospace.resolve(ui.style());
    let row_h = ui.text_style_height(&TextStyle::Body) + 6.0;
    egui::ScrollArea::vertical().auto_shrink(false).show_rows(ui, row_h, list.len(), |ui, rows| {
        for c in &list[rows] {
            let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), row_h), Sense::click());
            if resp.clicked() {
                repo.file = Some(c.path.clone());
                // Showing all files, the view brings it to the top; showing one, it's replaced.
                if let Some(view) = &mut repo.view {
                    view.jump(&c.path);
                }
            }
            let picked = repo.file.as_ref() == Some(&c.path);
            resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::ListItem, true, picked, &c.path));
            let w = ui.style().interact_selectable(&resp, picked);
            let painter = ui.painter_at(rect);
            if picked || resp.hovered() {
                painter.rect_filled(rect, 2.0, w.weak_bg_fill);
            }
            let weak = if picked { w.text_color() } else { v.weak_text_color() };
            let mut counts = LayoutJob::default();
            match c.lines {
                Some((a, r)) => {
                    counts.append(&format!("+{a}"), 0.0, TextFormat::simple(mono.clone(), green(&v)));
                    counts.append(&format!("-{r}"), 6.0, TextFormat::simple(mono.clone(), red(&v)));
                }
                None => counts.append("bin", 0.0, TextFormat::simple(mono.clone(), weak)),
            }
            let counts = painter.layout_job(counts);
            let status = match c.status {
                'A' => green(&v),
                'D' => red(&v),
                'M' => v.warn_fg_color,
                'R' | 'C' => v.hyperlink_color,
                _ => weak,
            };
            let (dir, name) = c.path.rsplit_once('/').unwrap_or(("", &c.path));
            let mut path = LayoutJob::default();
            path.append(&c.status.to_string(), 0.0, TextFormat::simple(mono.clone(), status));
            path.append(name, 8.0, TextFormat::simple(body.clone(), w.text_color()));
            path.append(dir, 8.0, TextFormat::simple(body.clone(), weak));
            path.wrap = TextWrapping::truncate_at_width(rect.width() - counts.size().x - 20.0);
            painter.galley(rect.min + vec2(6.0, 3.0), painter.layout_job(path), Color32::PLACEHOLDER);
            painter.galley(rect.right_top() + vec2(-6.0 - counts.size().x, 3.0), counts, Color32::PLACEHOLDER);
            if let Some(old) = &c.old {
                resp.on_hover_text(format!("from {old}"));
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::{Harness, kittest::Queryable};

    /// Steps the UI until `done` holds; background git calls land between frames.
    fn wait(h: &mut Harness<App>, done: impl Fn(&Harness<App>) -> bool) {
        for _ in 0..500 {
            h.step();
            if done(h) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("UI never settled");
    }

    #[test]
    fn picking_commits_shows_the_diff() {
        let dir = git::tests::repo("ui", &[]);
        let commit = |text: &str, subject: &str| {
            std::fs::write(dir.join("code.rs"), text).unwrap();
            git::tests::run(&dir, &["add", "code.rs"]);
            git::tests::run(&dir, &["commit", "-q", "-m", subject]);
        };
        // Twenty unchanged lines between the changes, for the changes-only mode to fold away.
        let notes: String = (1..=20).map(|i| format!("// note {i}\n")).collect();
        let main = "fn main() {\n    let name = \"world\";\n    println!(\"Hello, {name}!\");\n    let unused = 1;\n}\n\n";
        commit(&format!("{main}{notes}\nfn helper(x: i32) -> i32 {{\n\tx + 1\n}}\n"), "one");
        let main = "fn main() {\n    let name = \"gitgoo\";\n    println!(\"Hello, {name}!\");\n    greet(name);\n}\n\n";
        let greet = "fn greet(who: &str) {\n    println!(\"hi {who}\");\n}\n\n";
        commit(&format!("{main}{greet}{notes}\nfn helper(x: i32) -> i32 {{\n\tx + 2\n}}\n"), "two");
        let mut h = Harness::builder().with_size([1600.0, 500.0]).build_eframe(|cc| App::new(cc, Some(dir.clone())));
        wait(&mut h, |h| h.query_all_by_label("two").count() == 2);
        h.get_all_by_label("one").next().unwrap().click();
        h.get_all_by_label("two").nth(1).unwrap().click();
        wait(&mut h, |h| loaded(h, 1));
        h.step();
        shot(&mut h, "diff");
        for (button, name, mode) in [("Changes only", "chunks", diff::Mode::Chunks), ("Inline", "inline", diff::Mode::Inline)] {
            h.get_by_label(button).click();
            h.run_steps(2);
            shot(&mut h, name);
            assert_eq!(h.state().mode, mode);
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn hovering_a_commit_shows_its_whole_message() {
        let dir = git::tests::repo("ui-hover", &["one"]);
        let subject = "Keep the second commit, which the narrow column cuts short";
        // Wrapped at 72 columns, as git messages are, so the shot shows whether the tooltip wraps it again.
        let body = "The first line of the body runs right up to the seventy-two column mark\nand carries on below.\n\n- a list item\n- another one";
        git::tests::run(&dir, &["commit", "-q", "--allow-empty", "-m", subject, "-m", body]);
        let mut h = Harness::builder().with_size([1600.0, 500.0]).build_eframe(|cc| App::new(cc, Some(dir.clone())));
        wait(&mut h, |h| h.query_all_by_label(subject).count() == 2);
        assert!(h.query_by_label(body).is_none());
        h.get_all_by_label(subject).next().unwrap().hover();
        wait(&mut h, |h| h.query_by_label(body).is_some());
        shot(&mut h, "message");
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Whether the view holds `n` files, all arrived.
    fn loaded(h: &Harness<App>, n: usize) -> bool {
        h.state().repo.as_ref().unwrap().view.as_ref().is_some_and(|v| v.loaded() == (n, n))
    }

    #[test]
    fn every_file_shows_at_once_and_clicking_one_jumps_to_it() {
        let dir = git::tests::repo("ui-all", &[]);
        let names = ["a.txt", "b.txt", "c.txt", "d.txt", "e.txt", "f.txt"];
        let commit = |changed: usize, subject: &str| {
            for (i, name) in names.iter().enumerate() {
                // The last file is short, so a jump to it needs room below.
                let n = if i == names.len() - 1 { 3 } else { 40 };
                let text: String =
                    (0..n).map(|k| if k == changed % n { format!("{name} changed\n") } else { format!("{name} {k}\n") }).collect();
                std::fs::write(dir.join(name), text).unwrap();
            }
            git::tests::run(&dir, &["add", "-A"]);
            git::tests::run(&dir, &["commit", "-q", "-m", subject]);
        };
        commit(0, "one");
        commit(20, "two");
        let mut h = Harness::builder().with_size([1600.0, 500.0]).build_eframe(|cc| App::new(cc, Some(dir.clone())));
        let top = |h: &Harness<App>| h.state().repo.as_ref().unwrap().view.as_ref().and_then(|v| v.file().map(str::to_owned));
        wait(&mut h, |h| h.query_all_by_label("two").count() == 2);
        h.get_all_by_label("one").next().unwrap().click();
        h.get_all_by_label("two").nth(1).unwrap().click();
        // No file picked, yet every file shows.
        wait(&mut h, |h| loaded(h, names.len()));
        h.get_by_label("Changes only").click();
        h.run_steps(2);
        shot(&mut h, "all");
        h.get_by_label("Side by side").click();
        h.run_steps(2);
        assert_eq!(top(&h).as_deref(), Some("a.txt"));
        assert_eq!(h.state().repo.as_ref().unwrap().file.as_deref(), Some("a.txt"), "C marks the file at the top");
        h.get_by_label("f.txt").click();
        h.run_steps(2);
        shot(&mut h, "all-end");
        assert_eq!(top(&h).as_deref(), Some("f.txt"));
        h.get_by_label("One file").click();
        wait(&mut h, |h| loaded(h, 1));
        assert_eq!(top(&h).as_deref(), Some("f.txt"), "one file shows the file the reader was on");
        h.get_by_label("c.txt").click();
        wait(&mut h, |h| loaded(h, 1) && top(h).as_deref() == Some("c.txt"));
        h.step();
        shot(&mut h, "one");
        h.get_by_label("All files").click();
        wait(&mut h, |h| loaded(h, names.len()));
        h.step();
        assert_eq!(top(&h).as_deref(), Some("c.txt"), "all files opens at the file the reader was on");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn picking_a_branch_switches_the_column_and_keeps_a_commit_it_contains() {
        let dir = crate::graph::tests::branchy("ui-picker");
        let mut h = Harness::builder().with_size([1600.0, 700.0]).build_eframe(|cc| App::new(cc, Some(dir.clone())));
        let a = |h: &Harness<App>| h.state().repo.as_ref().unwrap().sides[0].picked.clone();
        let loaded = |subject: &'static str| {
            move |h: &Harness<App>| h.state().repo.as_ref().unwrap().sides[0].commits.iter().any(|c| c.subject == subject)
        };
        wait(&mut h, |h| h.query_all_by_label("c3").count() == 2);
        h.get_all_by_label("c3").next().unwrap().click();
        h.step();
        let c3 = a(&h).unwrap();
        // Opens column A's picker through its header.
        let open_picker = |h: &mut Harness<App>, header: &str| {
            h.get_all_by_label(header).next().unwrap().click();
            wait(h, |h| h.state().repo.as_ref().unwrap().picker.as_ref().is_some_and(|p| p.graph.is_some()));
            h.step();
        };
        open_picker(&mut h, "main");
        shot(&mut h, "picker");
        h.get_by_label("open").click();
        wait(&mut h, loaded("o2"));
        assert_eq!(a(&h).as_deref(), Some(c3.as_str()), "open forked after c3, so it contains it");
        open_picker(&mut h, "open");
        h.get_by_label("a").click();
        wait(&mut h, loaded("a1"));
        h.step();
        assert_eq!(a(&h), None, "a forked before c3");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn open_recent_switches_repositories_and_forgets_missing_ones() {
        let one = git::tests::repo("ui-recent-one", &["first"]);
        let two = git::root(&git::tests::repo("ui-recent-two", &["second"])).unwrap();
        let gone = std::env::temp_dir().join(format!("gitgoo-{}-gone", std::process::id()));
        let name = |p: &std::path::Path| p.file_name().unwrap().to_string_lossy().into_owned();
        let mut h = Harness::builder().with_size([1400.0, 600.0]).build_eframe(|cc| App::new(cc, Some(one.clone())));
        wait(&mut h, |h| h.query_all_by_label("first").count() == 2);
        // As if opened in earlier runs; one folder has since gone.
        h.state_mut().recents.remember(&gone);
        h.state_mut().recents.remember(&two);
        h.get_by_label("Open recent").click();
        // A new window is drawn invisibly on its first frame, while egui measures it.
        h.run_steps(3);
        shot(&mut h, "recent");
        h.get_by_label("Forget").click();
        h.run_steps(2);
        assert!(h.query_by_label(&name(&gone)).is_none());
        h.get_by_label(&name(&two)).click();
        wait(&mut h, |h| h.query_all_by_label("second").count() == 2);
        assert!(!h.state().recent_open);
        assert_eq!(h.state().repo.as_ref().map(|r| name(&r.path)), Some(name(&two)));
        // The start screen lists them too.
        h.state_mut().repo = None;
        h.run_steps(2);
        shot(&mut h, "start");
        assert!(h.query_by_label(&name(&one)).is_some());
        std::fs::remove_dir_all(one).unwrap();
        std::fs::remove_dir_all(two).unwrap();
    }

    /// Not a check: renders a real repo's branch picker, and times the graph, for a person to look at.
    /// `GITGOO_REPO=path GITGOO_SHOTS=folder cargo test real_repo -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn real_repo() {
        let dir = PathBuf::from(std::env::var("GITGOO_REPO").expect("GITGOO_REPO"));
        let start = std::time::Instant::now();
        let g = graph::load(&dir).unwrap();
        eprintln!("{} lanes besides {} in {:?}", g.lanes.len(), g.trunk.name, start.elapsed());
        let mut h = Harness::builder().with_size([1600.0, 900.0]).build_eframe(|cc| App::new(cc, Some(dir.clone())));
        wait(&mut h, |h| !h.state().repo.as_ref().unwrap().sides[0].commits.is_empty());
        let branch = h.state().repo.as_ref().unwrap().sides[0].branch.clone();
        h.get_all_by_label(&branch).next().unwrap().click();
        wait(&mut h, |h| h.state().repo.as_ref().unwrap().picker.as_ref().is_some_and(|p| p.graph.is_some()));
        h.step();
        shot(&mut h, "real-picker");
    }

    /// With GITGOO_SHOTS set to a folder, saves what the UI shows there, for a person to look at.
    fn shot(h: &mut Harness<App>, name: &str) {
        if let Ok(dir) = std::env::var("GITGOO_SHOTS") {
            h.render().unwrap().save(std::path::Path::new(&dir).join(format!("{name}.png"))).unwrap();
        }
    }
}
