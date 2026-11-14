#!/bin/sh
# Builds the Linux release tarball into target/dist. The build runs in Debian bookworm: a binary links against the
# glibc it's built with, and bookworm's 2.36 runs on most distributions since 2022, where a rolling distro's wouldn't.
set -e
cd "$(dirname "$0")/../.."
version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
docker run --rm --user "$(id -u):$(id -g)" -e HOME=/tmp -e CARGO_HOME=/tmp/cargo -e CARGO_TARGET_DIR=/src/target/bookworm \
    -v "$HOME/.cargo/registry:/tmp/cargo/registry" -v "$PWD:/src" -w /src \
    rust:1.99-bookworm cargo build --release --locked
name="gitgoo-$version-linux-x86_64"
rm -rf "target/dist/$name" && mkdir -p "target/dist/$name"
cp target/bookworm/release/gitgoo assets/gitgoo.svg assets/gitgoo.png packaging/linux/gitgoo.desktop packaging/linux/install.sh \
    LICENSE README.md "target/dist/$name/"
tar -czf "target/dist/$name.tar.gz" -C target/dist "$name"
echo "target/dist/$name.tar.gz"
