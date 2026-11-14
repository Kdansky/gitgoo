//! Aligns two versions of a file into side-by-side rows. Knows nothing of the UI.

use std::ops::Range;
use std::time::{Duration, Instant};

use similar::{Algorithm, DiffTag, capture_diff_slices, capture_diff_slices_deadline};

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Kind {
    Same,
    Removed,
    Added,
    /// A removed line paired with the added line that replaced it.
    Changed,
}

pub struct Row {
    pub kind: Kind,
    /// Indices into `Diff::old` and `Diff::new`; None where that side has no line.
    pub old: Option<usize>,
    pub new: Option<usize>,
    /// For a changed pair, the byte ranges that differ, old side first.
    pub words: [Vec<Range<usize>>; 2],
}

pub struct Diff {
    pub old: Vec<String>,
    pub new: Vec<String>,
    pub rows: Vec<Row>,
    /// The longest line in chars, which bounds horizontal scrolling.
    pub width: usize,
}

/// How the diff view lays out a diff.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Mode {
    /// Side by side, the whole file.
    Full,
    /// Side by side, only changes and the lines around them.
    Chunks,
    /// One column, removed lines above the added lines that replace them.
    Inline,
}

/// Unchanged lines kept around each change in `Mode::Chunks`.
pub const CONTEXT: usize = 4;

/// One drawn row of the diff view.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Item {
    /// A diff row: both sides when side by side; inline, an unchanged line shown once.
    Pair(usize),
    /// Inline, one side of a changed diff row: 0 old, 1 new.
    Half(usize, usize),
    /// `n` unchanged rows left out, starting at row `from`.
    Gap { from: usize, n: usize },
}

impl Item {
    /// The diff row it stands for, which keeps the reader's place across modes.
    pub fn row(self) -> usize {
        match self {
            Item::Pair(i) | Item::Half(i, _) | Item::Gap { from: i, .. } => i,
        }
    }
}

/// The rows `mode` draws, top to bottom.
pub fn items(d: &Diff, mode: Mode) -> Vec<Item> {
    let rows = &d.rows;
    let n = rows.len();
    match mode {
        Mode::Full => (0..n).map(Item::Pair).collect(),
        Mode::Chunks => {
            let mut near = vec![false; n];
            for i in (0..n).filter(|&i| rows[i].kind != Kind::Same) {
                near[i.saturating_sub(CONTEXT)..(i + CONTEXT + 1).min(n)].fill(true);
            }
            let mut out = Vec::new();
            let mut i = 0;
            while i < n {
                let from = i;
                while i < n && !near[i] {
                    i += 1;
                }
                if i > from {
                    out.push(Item::Gap { from, n: i - from });
                } else {
                    out.push(Item::Pair(i));
                    i += 1;
                }
            }
            out
        }
        Mode::Inline => {
            let mut out = Vec::new();
            let mut i = 0;
            while i < n {
                if rows[i].kind == Kind::Same {
                    out.push(Item::Pair(i));
                    i += 1;
                    continue;
                }
                // A run of changes reads as a unit: all it removed, then all it added.
                let from = i;
                while i < n && rows[i].kind != Kind::Same {
                    i += 1;
                }
                out.extend((from..i).filter(|&j| rows[j].old.is_some()).map(|j| Item::Half(j, 0)));
                out.extend((from..i).filter(|&j| rows[j].new.is_some()).map(|j| Item::Half(j, 1)));
            }
            out
        }
    }
}

/// The file as text, or None if it's binary or not UTF-8. Binary is git's test: a NUL in the first 8000 bytes.
pub fn text(bytes: Vec<u8>) -> Option<String> {
    if bytes[..bytes.len().min(8000)].contains(&0) {
        return None;
    }
    String::from_utf8(bytes).ok()
}

pub fn diff(old: &str, new: &str) -> Diff {
    let old: Vec<String> = old.lines().map(expand_tabs).collect();
    let new: Vec<String> = new.lines().map(expand_tabs).collect();
    // Past the deadline similar settles for a coarser diff, rather than freezing on a pathological pair.
    let deadline = Instant::now() + Duration::from_secs(1);
    let row = |kind, old, new| Row { kind, old, new, words: Default::default() };
    let mut rows = Vec::new();
    for op in capture_diff_slices_deadline(Algorithm::Patience, &old, &new, Some(deadline)) {
        let (tag, o, n) = op.as_tag_tuple();
        match tag {
            DiffTag::Equal => rows.extend(o.zip(n).map(|(o, n)| row(Kind::Same, Some(o), Some(n)))),
            DiffTag::Delete => rows.extend(o.map(|o| row(Kind::Removed, Some(o), None))),
            DiffTag::Insert => rows.extend(n.map(|n| row(Kind::Added, None, Some(n)))),
            DiffTag::Replace => {
                let pairs = o.len().min(n.len());
                for (o, n) in o.clone().zip(n.clone()) {
                    rows.push(Row { kind: Kind::Changed, old: Some(o), new: Some(n), words: words(&old[o], &new[n]) });
                }
                rows.extend(o.skip(pairs).map(|o| row(Kind::Removed, Some(o), None)));
                rows.extend(n.skip(pairs).map(|n| row(Kind::Added, None, Some(n))));
            }
        }
    }
    let width = old.iter().chain(&new).map(|l| l.chars().count()).max().unwrap_or(0);
    Diff { old, new, rows, width }
}

/// Byte ranges of the tokens that differ between two lines. Lines sharing no visible token get none: highlighting every
/// word of a rewritten line says nothing the line colour doesn't.
fn words(old: &str, new: &str) -> [Vec<Range<usize>>; 2] {
    let (a, b) = (tokens(old), tokens(new));
    let sa: Vec<&str> = a.iter().map(|r| &old[r.clone()]).collect();
    let sb: Vec<&str> = b.iter().map(|r| &new[r.clone()]).collect();
    let mut out = [Vec::new(), Vec::new()];
    let mut shared = false;
    for op in capture_diff_slices(Algorithm::Myers, &sa, &sb) {
        let (tag, o, n) = op.as_tag_tuple();
        if tag == DiffTag::Equal {
            shared |= sa[o].iter().any(|t| !t.trim().is_empty());
            continue;
        }
        if !o.is_empty() {
            out[0].push(a[o.start].start..a[o.end - 1].end);
        }
        if !n.is_empty() {
            out[1].push(b[n.start].start..b[n.end - 1].end);
        }
    }
    if shared { out } else { Default::default() }
}

/// Splits a line into runs of word characters, runs of whitespace, and single other characters.
fn tokens(s: &str) -> Vec<Range<usize>> {
    let class = |c: char| {
        if c.is_alphanumeric() || c == '_' {
            0
        } else if c.is_whitespace() {
            1
        } else {
            2
        }
    };
    let mut out = Vec::new();
    let mut it = s.char_indices().peekable();
    while let Some((i, c)) = it.next() {
        let mut end = i + c.len_utf8();
        while class(c) != 2
            && let Some(&(j, d)) = it.peek()
            && class(d) == class(c)
        {
            end = j + d.len_utf8();
            it.next();
        }
        out.push(i..end);
    }
    out
}

/// Tabs become spaces up to the next stop of 4: the view is a monospace grid, and egui knows no tab stops.
fn expand_tabs(line: &str) -> String {
    if !line.contains('\t') {
        return line.to_owned();
    }
    let mut out = String::with_capacity(line.len() + 8);
    for c in line.chars() {
        if c == '\t' {
            let n = 4 - out.chars().count() % 4;
            out.extend(std::iter::repeat_n(' ', n));
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_align_both_sides() {
        let d = diff("a\nb\nc\nd\n", "a\nB\nc\nx\nd\n");
        let got: Vec<_> = d.rows.iter().map(|r| (r.kind, r.old, r.new)).collect();
        assert_eq!(
            got,
            [
                (Kind::Same, Some(0), Some(0)),
                (Kind::Changed, Some(1), Some(1)),
                (Kind::Same, Some(2), Some(2)),
                (Kind::Added, None, Some(3)),
                (Kind::Same, Some(3), Some(4)),
            ]
        );
        let d = diff("a\nb\n", "a\n");
        assert_eq!(d.rows[1].kind, Kind::Removed);
        assert_eq!((d.rows[1].old, d.rows[1].new), (Some(1), None));
    }

    #[test]
    fn changed_pairs_mark_only_the_differing_words() {
        let d = diff("let x = foo(1);\n", "let x = bar(1, 2);\n");
        let r = &d.rows[0];
        let pick = |s: &str, rs: &[Range<usize>]| rs.iter().map(|r| s[r.clone()].to_owned()).collect::<Vec<_>>();
        assert_eq!(pick(&d.old[0], &r.words[0]), ["foo"]);
        assert_eq!(pick(&d.new[0], &r.words[1]), ["bar", ", 2"]);
        // Nothing in common: the line colour says it all.
        let d = diff("alpha\n", "omega\n");
        assert!(d.rows[0].words.iter().all(Vec::is_empty));
    }

    #[test]
    fn modes_pick_and_order_rows() {
        let old: String = (0..20).map(|i| format!("{i}\n")).collect();
        let new: String = (0..20).filter(|&i| i != 15).map(|i| if i == 2 { "two\n".into() } else { format!("{i}\n") }).collect();
        let d = diff(&old, &new);
        assert_eq!(items(&d, Mode::Full).len(), 20);
        // Rows 0..=6 around the change at 2, a gap, then 11..=19 around the removal at 15.
        let chunks = items(&d, Mode::Chunks);
        assert_eq!(chunks[7], Item::Gap { from: 7, n: 4 });
        assert_eq!(chunks.len(), 7 + 1 + 9);
        assert_eq!(items(&d, Mode::Chunks).last(), Some(&Item::Pair(19)));
        let inline = items(&d, Mode::Inline);
        assert_eq!(&inline[1..4], [Item::Pair(1), Item::Half(2, 0), Item::Half(2, 1)]);
        assert!(inline.contains(&Item::Half(15, 0)) && !inline.contains(&Item::Half(15, 1)));
        let same = diff("a\nb\n", "a\nb\n");
        assert_eq!(items(&same, Mode::Chunks), [Item::Gap { from: 0, n: 2 }]);
    }

    #[test]
    fn text_rejects_binary_and_non_utf8() {
        assert_eq!(text(b"hi\n".to_vec()).as_deref(), Some("hi\n"));
        assert_eq!(text(b"x\0y".to_vec()), None);
        assert_eq!(text(vec![0xff, 0xfe, b'a']), None);
    }

    #[test]
    fn tabs_stop_every_four_columns() {
        assert_eq!(expand_tabs("\tx"), "    x");
        assert_eq!(expand_tabs("ab\tx"), "ab  x");
        assert_eq!(diff("\ta\r\n", "\ta\n").rows[0].kind, Kind::Same);
    }
}
