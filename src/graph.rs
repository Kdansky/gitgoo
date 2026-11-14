//! How each branch relates to the trunk, for the branch picker. Knows nothing of the UI.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::git::{self, Result};

pub struct Branch {
    /// "refs/heads/main": unambiguous where a tag shares the name.
    pub refname: String,
    /// "main", or "origin/feature" for a remote one.
    pub name: String,
    pub tip: String,
    pub time: i64,
    /// The tip's: "2026-11-10, 4 days ago".
    pub date: String,
    pub remote: bool,
}

/// A trunk commit where a branch left or rejoined it.
#[derive(Clone)]
pub struct Point {
    pub time: i64,
    /// "3 weeks ago"
    pub when: String,
}

pub struct Lane {
    pub branch: Branch,
    pub fork: Point,
    /// Where it was merged into the trunk.
    pub merge: Option<Point>,
    /// Commits from leaving the trunk to the tip. 0 for a branch that only points at a trunk commit.
    pub commits: usize,
}

pub struct Graph {
    pub trunk: Branch,
    /// Most recently changed first.
    pub lanes: Vec<Lane>,
}

pub fn load(repo: &Path) -> Result<Graph> {
    let mut all = branches(repo)?;
    if all.is_empty() {
        return Err("no branches".into());
    }
    let trunk = all.remove(trunk(repo, &all));
    let merged: HashSet<String> =
        git::git(repo, &["for-each-ref", "--merged", &trunk.refname, "--format=%(refname)", "refs/heads", "refs/remotes"])?
            .lines()
            .map(str::to_owned)
            .collect();
    // For a merged branch, name-rev's path from the trunk ("main~3^2") leads to the trunk commit that merged it in.
    let mut joins = HashMap::new();
    let tips: Vec<&Branch> = all.iter().filter(|b| merged.contains(&b.refname)).collect();
    // In chunks, as Windows caps a command line at 32k characters.
    for chunk in tips.chunks(200) {
        let mut args = vec!["name-rev", "--name-only", "--refs", &trunk.refname];
        args.extend(chunk.iter().map(|b| b.tip.as_str()));
        for (b, name) in chunk.iter().zip(git::git(repo, &args)?.lines()) {
            joins.insert(b.refname.clone(), join(name));
        }
    }
    let n = all.len().div_ceil(8).max(1);
    let found: Vec<Option<(String, Option<usize>, usize)>> = std::thread::scope(|s| {
        let work = |b: &Branch| -> Option<(String, Option<usize>, usize)> {
            let base = |a: &str| git::git(repo, &["merge-base", a, &b.tip]).ok().map(|h| h.trim().to_owned());
            let (fork, merge) = match joins.get(&b.refname) {
                None => (base(&trunk.refname)?, None),
                Some(&(_, false)) => (b.tip.clone(), None),
                Some(&(k, true)) => (base(&format!("{}~{}", trunk.refname, k + 1))?, Some(k)),
            };
            let commits = git::git(repo, &["rev-list", "--count", &format!("{fork}..{}", b.tip)]).ok()?.trim().parse().ok()?;
            Some((fork, merge, commits))
        };
        let threads: Vec<_> = all.chunks(n).map(|c| s.spawn(move || c.iter().map(work).collect::<Vec<_>>())).collect();
        threads.into_iter().flat_map(|t| t.join().unwrap()).collect()
    });
    // The trunk's first-parent chain, newest first, so a merge's step count indexes it.
    let deepest = found.iter().flatten().filter_map(|f| f.1).max();
    let chain: Vec<Point> = match deepest {
        Some(k) => git::git(repo, &["log", "--first-parent", "--format=%H%x1f%ct%x1f%cr", &format!("-n{}", k + 1), &trunk.refname])?
            .lines()
            .filter_map(point)
            .map(|(_, p)| p)
            .collect(),
        None => Vec::new(),
    };
    let mut forks: Vec<&str> = found.iter().flatten().map(|f| f.0.as_str()).collect();
    forks.sort_unstable();
    forks.dedup();
    let mut fork_points = HashMap::new();
    for chunk in forks.chunks(200) {
        let mut args = vec!["show", "-s", "--format=%H%x1f%ct%x1f%cr"];
        args.extend(chunk);
        fork_points.extend(git::git(repo, &args)?.lines().filter_map(point));
    }
    let lanes = all
        .into_iter()
        .zip(found)
        .filter_map(|(branch, f)| {
            let (fork, merge, commits) = f?;
            let fork = fork_points.get(&fork)?.clone();
            let merge = merge.and_then(|k| chain.get(k)).cloned();
            Some(Lane { branch, fork, merge, commits })
        })
        .collect();
    Ok(Graph { trunk, lanes })
}

/// A line of "hash\x1ftime\x1frelative time".
fn point(line: &str) -> Option<(String, Point)> {
    let mut f = line.splitn(3, '\x1f');
    Some((f.next()?.to_owned(), Point { time: f.next()?.parse().ok()?, when: f.next()?.to_owned() }))
}

/// Local branches, and remote ones no local branch shares a name with, one per commit, origin's preferred; most recently
/// changed first.
fn branches(repo: &Path) -> Result<Vec<Branch>> {
    let format = "--format=%(refname)%1f%(refname:short)%1f%(objectname)%1f%(committerdate:unix)%1f\
                  %(committerdate:short), %(committerdate:relative)%1f%(symref)";
    let out = git::git(repo, &["for-each-ref", "--sort=-committerdate", format, "refs/heads", "refs/remotes"])?;
    let all: Vec<Branch> = out
        .lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split('\x1f').collect();
            // A symref is origin/HEAD, an alias rather than a branch.
            let [refname, name, tip, time, date, ""] = f[..] else { return None };
            let remote = refname.starts_with("refs/remotes/");
            Some(Branch {
                refname: refname.into(),
                name: name.into(),
                tip: tip.into(),
                time: time.parse().ok()?,
                date: date.into(),
                remote,
            })
        })
        .collect();
    let local: HashSet<String> = all.iter().filter(|b| !b.remote).map(|b| b.name.clone()).collect();
    // A remote branch as (name without the remote, tip): several remotes often carry one branch at one commit.
    let key = |b: &Branch| b.name.split_once('/').map(|(_, n)| (n.to_owned(), b.tip.clone()));
    let origin: HashSet<_> = all.iter().filter(|b| b.remote && b.name.starts_with("origin/")).filter_map(key).collect();
    let mut seen = HashSet::new();
    Ok(all
        .into_iter()
        .filter(|b| {
            let Some(k) = key(b).filter(|_| b.remote) else { return true };
            !local.contains(&k.0) && (b.name.starts_with("origin/") || !origin.contains(&k)) && seen.insert(k)
        })
        .collect())
}

/// The branch the others grow from: whatever origin's HEAD names, else main or master, else the checked-out branch.
fn trunk(repo: &Path, all: &[Branch]) -> usize {
    let origin = git::git(repo, &["symbolic-ref", "-q", "--short", "refs/remotes/origin/HEAD"]).unwrap_or_default();
    let origin = origin.trim();
    let local = origin.split_once('/').map_or("", |(_, n)| n);
    let head = git::head(repo);
    [local, origin, "main", "master", head.as_str()]
        .iter()
        .find_map(|n| all.iter().position(|b| !n.is_empty() && b.name == *n))
        .unwrap_or(0)
}

/// From name-rev's "main~3^2~1": the first-parent steps from the trunk's tip before the first second-parent hop, and
/// whether there is one. Without one the commit sits on the trunk itself.
fn join(name: &str) -> (usize, bool) {
    let mut k = 0;
    let mut rest = name.find(['~', '^']).map_or("", |i| &name[i..]);
    while let Some(op) = rest.chars().next() {
        let digits = rest[1..].find(|c: char| !c.is_ascii_digit()).unwrap_or(rest.len() - 1);
        let n: usize = rest[1..1 + digits].parse().unwrap_or(1);
        if op == '^' && n > 1 {
            return (k, true);
        }
        k += n;
        rest = &rest[1 + digits..];
    }
    (k, false)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::git::tests::run;

    /// A repo whose branches cover every shape: merged, open, merged via another branch, and a bare pointer.
    pub(crate) fn branchy(name: &str) -> std::path::PathBuf {
        let dir = crate::git::tests::repo(name, &[]);
        // A minute apart, as real history is: the picker lays events out by time.
        let minute = std::cell::Cell::new(0);
        let at = |args: &[&str]| {
            minute.set(minute.get() + 1);
            let date = format!("2026-01-01T{:02}:{:02}:00", minute.get() / 60, minute.get() % 60);
            let mut c = crate::git::tests::cmd(&dir);
            let ok = c.args(args).env("GIT_AUTHOR_DATE", &date).env("GIT_COMMITTER_DATE", &date).output().unwrap().status.success();
            assert!(ok, "git {args:?}");
        };
        let commit = |file: &str, msg: &str| {
            std::fs::write(dir.join(file), msg).unwrap();
            run(&dir, &["add", "-A"]);
            at(&["commit", "-q", "-m", msg]);
        };
        let merge = |branch: &str| at(&["merge", "-q", "--no-ff", "-m", &format!("merge {branch}"), branch]);
        let checkout = |args: &[&str]| run(&dir, &[&["checkout", "-q"][..], args].concat());
        commit("m", "c1");
        run(&dir, &["branch", "pointer"]);
        commit("m", "c2");
        checkout(&["-b", "a"]);
        commit("a", "a1");
        checkout(&["main"]);
        commit("m", "c3");
        checkout(&["-b", "open"]);
        commit("o", "o1");
        commit("o", "o2");
        checkout(&["main"]);
        merge("a");
        checkout(&["-b", "outer"]);
        commit("e", "e1");
        checkout(&["-b", "inner"]);
        commit("d", "d1");
        checkout(&["outer"]);
        merge("inner");
        checkout(&["main"]);
        merge("outer");
        dir
    }

    #[test]
    fn join_reads_name_rev_paths() {
        assert_eq!(join("main"), (0, false));
        assert_eq!(join("main~6"), (6, false));
        assert_eq!(join("main~3^2"), (3, true));
        assert_eq!(join("remotes/origin/main~1^2^2~4"), (1, true));
        assert_eq!(join("main^^2"), (1, true));
    }

    #[test]
    fn lanes_leave_and_rejoin_the_trunk() {
        let dir = branchy("graph");
        let g = load(&dir).unwrap();
        assert_eq!(g.trunk.name, "main");
        let lane = |name: &str| g.lanes.iter().find(|l| l.branch.name == name).unwrap();
        let (a, open, inner, outer, pointer) = (lane("a"), lane("open"), lane("inner"), lane("outer"), lane("pointer"));
        assert_eq!((a.commits, a.merge.is_some()), (1, true));
        assert_eq!((open.commits, open.merge.is_some()), (2, false));
        // inner reached the trunk through outer's merge.
        assert_eq!((inner.commits, inner.merge.is_some()), (2, true));
        assert_eq!((outer.commits, outer.merge.is_some()), (3, true));
        assert_eq!((pointer.commits, pointer.merge.is_some()), (0, false));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
