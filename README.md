# gitgoo

A git review tool. Pick any two commits, on any two branches, and see everything that differs between them.

Most tools show one commit's changes. gitgoo compares two arbitrary points: skip commits, or put one branch's head against another's.

![gitgoo reviewing its own repo: two commit histories, the files between the picked commits, and Cargo.toml side by side](docs/screenshot.png)

## Installing

Linux, x86_64: download `gitgoo-<version>-linux-x86_64.tar.gz` from [Releases](https://github.com/Kdansky/gitgoo/releases), unpack it and run `./install.sh`. That puts `gitgoo` in `~/.local/bin` and adds it to the application menu. Needs glibc 2.35 or later (Ubuntu 22.04, Debian 12, Fedora 36 and newer) and `git`.

To remove it: `rm ~/.local/bin/gitgoo ~/.local/share/applications/gitgoo.desktop ~/.local/share/icons/hicolor/*/apps/gitgoo.*`

## Using it

Start the application and pick a repository folder, or run `gitgoo path/to/repo`. Repositories opened before are listed on the start screen and behind the toolbar's "Open recent".

- **First and second column**: two commit histories. Hover a commit for its whole message. The branch name above each opens a map of the branches, drawn leaving and rejoining main; click one to switch.
- **List of files**: the files that differ between the commits picked in A and B, with line counts. Clicking one jumps to it.
- **Two columns of text**: every file that differs, one after another, old (A) on the left and new (B) on the right, with changed words marked.

The toolbar switches between all files and one at a time, for very large diffs, and the diff between the whole file side by side, only the changes, and inline.

Shows UTF-8 files, no binaries or other formats.

## Building

```sh
cargo build --release
cargo test
```

Needs Rust 1.95 or later, and `git` on the PATH. 


## Testing

The UI tests render offscreen through wgpu. Only Linux was tested so far.

[DESIGN.md](DESIGN.md) covers how it works and what comes next.

## License

MIT
