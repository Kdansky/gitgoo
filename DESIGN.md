# gitgoo

A read-only code-review tool for git repos. Pick any two commits, on any two branches, and see everything that differs between them.

Most tools show one commit's change set. gitgoo compares two arbitrary points: skip commits, or put one branch's head against another's.

## Layout

Five columns:

| A | B | C | D | E |
|---|---|---|---|---|
| history, left | history, right | changed files | files at A | files at B |

- **A, B**: a branch's commits, newest first. The branch name heads the column; clicking it opens the branch picker.
- **C**: files that differ between the commits picked in A and B, with status and +/− line counts. Clicking one brings it to the top of D and E. The file at the top is the one marked, so the mark follows the reader.
- **D, E**: every file that differs, one after another, as in GitLab's compare view: side by side, aligned, scrolling together. Each file starts with a header row naming it, with its line counts. The old file is always A's commit, on the left; the new one B's. Above the columns, each side names the file at the top and its commit.

A toolbar spans the window above the columns: "Open recent" leftmost, then a pair switching between all files and one, then a group switching the diff view between three modes:

1. Side by side, the whole file.
2. Side by side, only the changes and 4 lines around each; the stretches left out show as one "n unchanged lines" row.
3. Inline: D and E become one column with both line numbers, each run of changes showing everything it removed, then everything it added.

The mode holds across files. Switching keeps the reader's place.

All files reads and diffs every file that differs; for a diff too large for that, "One file" shows only the file picked in C, and clicking another file replaces it. Switching between the two keeps the file the reader was on. Each repository opens showing all files, whatever was picked last.

The branch picker is a modal map. The trunk (whatever origin's HEAD names, else main or master) runs along the top; every other branch gets a row below, most recently changed first. Time runs left to right, events spaced evenly rather than by date. A branch's line drops from where it left the trunk and runs to its tip; a merged one, dimmed, curves back up where it was merged, even if it got there inside another branch. A branch that only points at a trunk commit is a dashed drop. No commit dots: only names, and hovering one shows its last commit's date, its commit count, and when it left and rejoined the trunk. Typing filters; Enter picks the top match. Remote branches show unless a local branch has the name; several remotes carrying one branch at one commit show once, origin preferred.

Switching a column's branch keeps its commit if the new branch contains it, and the file if it still differs.

## Starting

Double-clicking the binary passes no arguments, so gitgoo opens the OS folder picker on a start screen. Any folder inside a repo opens that repo. An argument (`gitgoo path/to/repo`) opens it directly. A folder that isn't a repo leaves the start screen up with git's reason.

gitgoo remembers the last 20 repositories opened, newest first. The start screen lists them, and so does the dialog behind the toolbar's leftmost button, "Open recent", which can also browse for another folder. A repository whose folder has since moved or gone shows as missing, and can only be forgotten; any entry can be. The list lives in eframe's storage (`~/.local/share/gitgoo` on Linux, Application Support on macOS, AppData on Windows), written as soon as it changes. That storage also keeps the diff mode, the window's size and the column widths.

## Diff semantics

`git diff X Y`, X picked in A, Y in B: the complete difference between the two snapshots, whatever lies between them. Not the merge-base diff (`X...Y`). Comparing two branch heads therefore shows both sides' work; that's intended.

Only UTF-8 text is shown. A binary or non-UTF-8 file gets one inline line: "binary file not supported". Git's binary test: a NUL in the first 8000 bytes.

## Stack

Rust, egui via eframe. One native binary each for Linux, macOS, Windows.

Read-only by design, so egui's weak text editing costs nothing, while its virtualized `ScrollArea::show_rows` suits an app made of long lists. iced would win only if editing came into scope.

| Crate | For |
|---|---|
| `eframe` | window, UI |
| `syntect` + `two-face` | highlighting. `two-face` adds bat's syntaxes; syntect's own set lacks TypeScript, TOML and others |
| `similar` | line alignment, word-level changes |

Git is the `git` binary, not git2 or gitoxide: it honours the user's config and handles every repo layout.

| Need | Command |
|---|---|
| branch picker | `git for-each-ref --sort=-committerdate`; `git name-rev` finds the trunk commit that merged each merged branch, whatever route it took; `git merge-base` where each left |
| A, B | `git log`, paged as the list scrolls |
| C | `git diff --raw --no-abbrev --numstat -z -M X Y` |
| D, E | one `git cat-file --batch` per view, fed the blob ids C's raw diff names |

`-z` everywhere, so git never quotes unusual paths.

## Modules

```
src/
  main.rs       arguments, window
  app.rs        state, layout
  graph.rs      how branches leave and rejoin the trunk; no egui
  picker.rs     draws the branch picker
  recent.rs     repositories opened before, and their dialog
  git.rs        runs git, parses output; no egui
  diff.rs       aligned rows, word changes; no egui
  highlight.rs  syntect on a worker thread, cached per blob
  diffview.rs   draws D and E
```

`git.rs` and `diff.rs` know nothing of the UI and are tested against throwaway repos. UI tests drive the whole app offscreen with `egui_kittest`, finding rows by their accessibility labels; `GITGOO_SHOTS=folder cargo test` saves what the UI tests see; `GITGOO_REPO=path GITGOO_SHOTS=folder cargo test real_repo -- --ignored` renders a real repo's picker.

Nothing blocks a frame. Git calls and highlighting run on worker threads and send results back; a result tagged with a request the UI has since moved past is dropped. Each open repository has its own channel, so work for one since replaced lands nowhere. So does each diff view: its worker diffs the files top first and sends each when done, so the first show while the rest load. A replaced view drops its channel, and its worker stops at the next file.

## Diff view

- Diff the two full files into rows: left line, right line, or one side empty. Modified rows carry word ranges.
- One `show_rows` over the rows, each painted into two clipped halves. The sides can't drift: they're one widget. One horizontal offset scrolls both.
- No wrapping: `show_rows` needs uniform row height. Wrapping would need precomputed heights.
- Highlight each full file, not the diff: a line inside a multi-line comment is a comment only because of the lines above it.
- Bundled monospace font, line numbers per side, dimmed filler rows, word-level colour, an overview strip of all changes (click to jump), next/previous change keys, whitespace toggle, light/dark from the OS.
- What each mode draws is a list of items picked from the rows (`diff::items`): a row, one side of a row, or a gap. The view paints whichever list it's given.
- Several files make one list: per file a blank row, a header row, then its items, all one height, so one `show_rows` covers them. A file still loading, binary or empty is one note row. A lone file gets no header.
- A file arriving above the reader, or a mode change, keeps the line at the top where it is. A short last file gets blank room below it, so a jump can still bring it to the top.

## Milestones

1. Window, five resizable panes, repo from the working directory or an argument, A and B show the current branch's history.
2. Branch picker.
3. File list.
4. Diff view: aligned rows, line numbers, colours.
5. Syntax and word-level highlighting.
6. Overview strip, keyboard navigation, branches and mode remembered per repo.

Later: uncommitted and staged changes as entries atop each list; marking files reviewed; per-line origin across the range ("changed in commit abc123"); expanding a left-out stretch in place.

## Platforms

- Windows: git is spawned with `CREATE_NO_WINDOW`, or each call flashes a console. The binary sets `windows_subsystem = "windows"`.
- Builds: Linux here, macOS on the Mac. No CI.
- Linux releases: `packaging/linux/release.sh` builds in a Debian bookworm container and packs a tarball with an `install.sh`. A binary needs the glibc it was built against; bookworm's makes it run on distributions since 2022, where a rolling distro's wouldn't.
- Icon: `assets/gitgoo.svg`, rendered to the PNG the binary embeds as its window icon. Wayland ignores window icons: the desktop finds the icon through the launcher whose `StartupWMClass` matches the app id, `gitgoo`, so `install.sh` installs that launcher.
