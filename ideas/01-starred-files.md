# 01 Starred files

The note, as written in `todo.md`:

> The ability to right-click a file and mark it as "important" which will then put a star on every commit that changed the file
> in question. Right clicking such a commit lists the files that are starred, and allows the star removal.

## What it asks

- Right-clicking a file in C opens a menu with one entry: "Mark important", or "Not important" once it is.
  [Assumption: the menu's wording, and that unstarring from C works too, not only from a commit.]
- Every commit in A and B that changed a starred file shows a star. [Assumption: a star is a mark on the row, drawn at the end
  of the subject line; the note says nothing about where.] [Assumption: C stars its starred files as well, so the reader sees
  which ones are.]
- Right-clicking a starred commit lists the starred files it changed; picking one removes that file's star. [Assumption: the
  star belongs to the file, so removing it there removes it from every commit, not just that one.] [Assumption: an unstarred
  commit opens no menu.]
- [Assumption: stars are kept per repository across restarts.]
- [Assumption: a star follows a path, not a file's history: commits from before a rename don't get it. `--follow` takes one
  path, so following renames would mean one git call per starred file.]

## Where it lands

- `Repo` (`src/app.rs`) gets the starred paths and, per side, a map from commit id to the starred files it changed.
- One worker call per side: `git log -z --format=%H --name-only <branch> -- <starred paths>`. The pathspec limits the names
  listed to starred ones, so the output is the map as is. It covers the whole branch, loaded or not, so paging doesn't matter.
  It goes through the repo's `Msg` channel tagged with the branch and the star set; an answer for an older set is dropped.
  Rerun when a star changes or a column switches branch.
- `git.rs` gets the parse and a test against a throwaway repo, like `changes_cover_every_status`.
- `history()` and `files()` in `src/app.rs` draw the star and open the menus (`resp.context_menu`).
- Storage: a map from repo path to starred paths in eframe storage under its own key, written when it changes, as
  `recent.rs` does for the recents list.
- DESIGN.md: a line under Layout for C and A/B, and the git command in the table.

## Traps to check

- Merges: path-limited `git log` shows no diff for a merge by default, so a merge that changed a starred file (a conflict
  resolution) lists no names. `--diff-merges=first-parent` would name what it changed against the trunk side.
  [Assumption: that's the change a reviewer means.]
- Is there a star glyph in the fonts? The app bundles a monospace font, and egui's defaults may lack U+2605. Painting the
  star as a shape, the way `icon_button` in `src/diffview.rs` draws its icons, avoids the question.
- A big history with many starred paths makes the call slow. It runs off the UI thread, so stars just show up a beat late.

## Size

More than an afternoon: two context menus, a worker call, storage, drawing in two columns, a git test and a UI test.
