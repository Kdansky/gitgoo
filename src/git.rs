//! Runs the git binary and parses its output. Knows nothing of the UI.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

pub type Result<T> = std::result::Result<T, String>;

pub struct Commit {
    pub id: String,
    pub author: String,
    /// Relative, as git words it: "3 days ago".
    pub when: String,
    pub subject: String,
}

pub(crate) fn git(dir: &Path, args: &[&str]) -> Result<String> {
    // Old commits may carry messages in other encodings; a stray byte shouldn't hide the whole list.
    Ok(String::from_utf8_lossy(&raw(dir, args)?).into_owned())
}

fn command(dir: &Path, args: &[&str]) -> Command {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(dir).args(args);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW: otherwise a GUI app flashes a console for every call.
        cmd.creation_flags(0x0800_0000);
    }
    cmd
}

fn raw(dir: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let out = command(dir, args).output().map_err(|e| format!("can't run git: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().trim_start_matches("fatal: ").to_owned());
    }
    Ok(out.stdout)
}

/// Whether `commit` is in `rev`'s history.
pub fn contains(repo: &Path, rev: &str, commit: &str) -> bool {
    raw(repo, &["merge-base", "--is-ancestor", commit, rev]).is_ok()
}

/// One `git cat-file --batch`, reading files by blob id for as long as it's kept: a diff of many files costs one process,
/// not two per file.
pub struct Cat {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
}

impl Cat {
    pub fn new(repo: &Path) -> Result<Self> {
        let child = command(repo, &["cat-file", "--batch"]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn();
        let mut child = child.map_err(|e| format!("can't run git: {e}"))?;
        let (input, output) = (child.stdin.take().unwrap(), BufReader::new(child.stdout.take().unwrap()));
        Ok(Self { child, input, output })
    }

    /// The blob `id`, byte for byte.
    pub fn blob(&mut self, id: &str) -> Result<Vec<u8>> {
        let io = |e: std::io::Error| format!("git cat-file: {e}");
        // git flushes each answer before reading the next question, so asking one at a time can't deadlock.
        writeln!(self.input, "{id}").map_err(io)?;
        let mut head = String::new();
        self.output.read_line(&mut head).map_err(io)?;
        // "<id> blob <size>", or "<id> missing".
        let f: Vec<&str> = head.split_whitespace().collect();
        let [_, kind, size] = f[..] else { return Err(format!("no blob {id}")) };
        let mut data = vec![0; size.parse::<usize>().map_err(|_| format!("git cat-file: {head}"))? + 1];
        self.output.read_exact(&mut data).map_err(io)?;
        data.pop();
        if kind != "blob" {
            return Err(format!("{id} is a {kind}, not a file"));
        }
        Ok(data)
    }
}

impl Drop for Cat {
    fn drop(&mut self) {
        _ = self.child.kill();
        _ = self.child.wait();
    }
}

pub fn root(dir: &Path) -> Result<PathBuf> {
    Ok(PathBuf::from(git(dir, &["rev-parse", "--show-toplevel"])?.trim_end()))
}

/// The checked-out branch, or "HEAD" when detached.
pub fn head(repo: &Path) -> String {
    let name = git(repo, &["symbolic-ref", "--short", "-q", "HEAD"]).unwrap_or_default();
    match name.trim() {
        "" => "HEAD".into(),
        name => name.into(),
    }
}

/// `n` commits reachable from `rev`, newest first, after skipping `skip`.
pub fn log(repo: &Path, rev: &str, skip: usize, n: usize) -> Result<Vec<Commit>> {
    let (skip, n) = (format!("--skip={skip}"), format!("-n{n}"));
    let out = git(repo, &["log", "-z", "--format=%H%x1f%an%x1f%ar%x1f%s", &skip, &n, rev, "--"])?;
    Ok(out
        .split('\0')
        .filter_map(|rec| {
            let mut f = rec.splitn(4, '\x1f');
            Some(Commit { id: f.next()?.into(), author: f.next()?.into(), when: f.next()?.into(), subject: f.next()?.into() })
        })
        .collect())
}

/// One file that differs between two commits.
#[derive(Clone)]
pub struct Change {
    /// A, M, D, R, C or T, as git reports it.
    pub status: char,
    pub path: String,
    /// The path at the older commit, for renames and copies.
    pub old: Option<String>,
    /// Lines added and removed; None for binary files.
    pub lines: Option<(usize, usize)>,
    /// The blob ids at the older and newer commit; None where the file doesn't exist.
    pub blobs: [Option<String>; 2],
}

/// Every file that differs between `a` and `b`, in git's path order.
pub fn changes(repo: &Path, a: &str, b: &str) -> Result<Vec<Change>> {
    let out = git(repo, &["diff", "--no-ext-diff", "--no-color", "--raw", "--no-abbrev", "--numstat", "-z", "-M", a, b, "--"])?;
    // Raw records come first, then numstat records for the same files in the same order.
    let (mut list, mut counts) = (Vec::new(), Vec::new());
    let mut t = out.split('\0');
    while let Some(rec) = t.next() {
        if let Some(raw) = rec.strip_prefix(':') {
            // ":100644 100644 de98044… a7bc997… R086", then one path, or two for a rename or copy. A side without the file
            // has an id of zeros.
            let f: Vec<&str> = raw.split(' ').collect();
            let status = f.last().and_then(|s| s.chars().next()).unwrap_or('M');
            let id = |i: usize| f.get(i).filter(|s| !s.bytes().all(|b| b == b'0')).map(|s| s.to_string());
            let first = t.next().unwrap_or_default().to_owned();
            let (path, old) = match status {
                'R' | 'C' => (t.next().unwrap_or_default().to_owned(), Some(first)),
                _ => (first, None),
            };
            list.push(Change { status, path, old, lines: None, blobs: [id(2), id(3)] });
        } else if !rec.is_empty() {
            // "2\t1\tpath"; binary files count "-\t-"; a rename has an empty path, then old and new as their own records.
            let mut f = rec.splitn(3, '\t');
            let (added, removed) = (f.next().and_then(|n| n.parse().ok()), f.next().and_then(|n| n.parse().ok()));
            if f.next() == Some("") {
                t.nth(1);
            }
            counts.push(added.zip(removed));
        }
    }
    for (c, n) in list.iter_mut().zip(counts) {
        c.lines = n;
    }
    Ok(list)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// git in `dir`. The user's global config may demand signing or lack an identity, so both are set here.
    pub(crate) fn cmd(dir: &Path) -> Command {
        let mut c = Command::new("git");
        c.args(["-c", "user.name=Tester", "-c", "user.email=t@example.com", "-c", "commit.gpgsign=false"]).current_dir(dir);
        c
    }

    pub(crate) fn run(dir: &Path, args: &[&str]) {
        assert!(cmd(dir).args(args).output().unwrap().status.success(), "git {args:?}");
    }

    /// A fresh repo in the temp dir with one commit per subject, oldest first.
    pub(crate) fn repo(name: &str, subjects: &[&str]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("gitgoo-{}-{name}", std::process::id()));
        _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        run(&dir, &["init", "-q", "-b", "main"]);
        for (i, s) in subjects.iter().enumerate() {
            std::fs::write(dir.join("f.txt"), format!("{i}\n")).unwrap();
            run(&dir, &["add", "f.txt"]);
            run(&dir, &["commit", "-q", "-m", s]);
        }
        dir
    }

    #[test]
    fn changes_cover_every_status() {
        let dir = repo("changes", &[]);
        let write = |path: &str, data: &[u8]| std::fs::write(dir.join(path), data).unwrap();
        let lines: String = (1..=50).map(|i| format!("{i}\n")).collect();
        write("a.txt", b"a\nb\nc\n");
        write("gone.txt", b"x\n");
        write("old.txt", lines.as_bytes());
        write("bin.dat", b"x\0y");
        run(&dir, &["add", "-A"]);
        run(&dir, &["commit", "-q", "-m", "one"]);
        write("a.txt", b"a\nB\nc\nd\n");
        std::fs::remove_file(dir.join("gone.txt")).unwrap();
        std::fs::create_dir(dir.join("sub")).unwrap();
        std::fs::rename(dir.join("old.txt"), dir.join("sub/new.txt")).unwrap();
        write("bin.dat", b"x\0z");
        write("c.txt", b"c\n");
        run(&dir, &["add", "-A"]);
        run(&dir, &["commit", "-q", "-m", "two"]);
        let list = changes(&dir, "HEAD~1", "HEAD").unwrap();
        let got: Vec<_> = list.iter().map(|c| (c.status, c.path.as_str(), c.old.as_deref(), c.lines)).collect();
        assert_eq!(
            got,
            [
                ('M', "a.txt", None, Some((2, 1))),
                ('M', "bin.dat", None, None),
                ('A', "c.txt", None, Some((1, 0))),
                ('D', "gone.txt", None, Some((0, 1))),
                ('R', "sub/new.txt", Some("old.txt"), Some((0, 0))),
            ]
        );
        let sides: Vec<_> = list.iter().map(|c| c.blobs.clone().map(|id| id.is_some())).collect();
        assert_eq!(sides, [[true, true], [true, true], [false, true], [true, false], [true, true]]);
        let mut cat = Cat::new(&dir).unwrap();
        let [old, new] = list[0].blobs.clone().map(Option::unwrap);
        assert_eq!(cat.blob(&old).unwrap(), b"a\nb\nc\n");
        assert_eq!(cat.blob(&new).unwrap(), b"a\nB\nc\nd\n");
        assert!(cat.blob(&"0".repeat(40)).is_err());
        assert_eq!(cat.blob(&list[1].blobs[1].clone().unwrap()).unwrap(), b"x\0z", "answers go on after a failed question");
        assert!(changes(&dir, "HEAD", "HEAD").unwrap().is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn log_pages_newest_first() {
        let dir = repo("log", &["one", "two", "three"]);
        let all = log(&dir, "main", 0, 10).unwrap();
        assert_eq!(all.iter().map(|c| c.subject.as_str()).collect::<Vec<_>>(), ["three", "two", "one"]);
        assert_eq!(all[0].author, "Tester");
        assert_eq!(all[0].id.len(), 40);
        let page = log(&dir, "main", 1, 1).unwrap();
        assert_eq!(page.len(), 1);
        assert_eq!(page[0].subject, "two");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn root_and_head() {
        let dir = repo("root", &["one"]);
        std::fs::create_dir(dir.join("sub")).unwrap();
        assert_eq!(root(&dir.join("sub")).unwrap().canonicalize().unwrap(), dir.canonicalize().unwrap());
        assert_eq!(head(&dir), "main");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn empty_repo_errors() {
        let dir = repo("empty", &[]);
        assert!(log(&dir, "main", 0, 10).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
