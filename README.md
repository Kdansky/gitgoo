# gitgoo

A git review tool. Pick any two commits, on any two branches, and see everything that differs between them.

Most tools show one commit's changes. gitgoo compares two arbitrary points: skip commits, or put one branch's head against another's.

## Using it

Double-click the binary and pick a repository folder, or run `gitgoo path/to/repo`.

- **A, B**: two commit histories. The branch name above each opens a map of the branches, drawn leaving and rejoining main; click one to switch.
- **C**: the files that differ between the commits picked in A and B, with line counts.
- **D, E**: the picked file, old (A) on the left and new (B) on the right, with changed words marked.

The toolbar switches the diff between the whole file side by side, only the changes, and inline.

Only UTF-8 text is shown; binary files say so.

## Building

```sh
cargo build --release
cargo test
```

Needs Rust 1.95 or later, and `git` on the PATH. Built for Linux, macOS and Windows; so far tried on Linux. The UI tests render offscreen through wgpu.

[DESIGN.md](DESIGN.md) covers how it works and what comes next.

## License

MIT
