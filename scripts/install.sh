#!/usr/bin/env sh
set -eu

install_dir="${INSTALL_DIR:-$HOME/.local/bin}"
binary="target/release/leanmgr"

if [ ! -x "$binary" ]; then
  cargo build --release
fi

mkdir -p "$install_dir"
cp "$binary" "$install_dir/leanmgr"

printf 'Installed leanmgr to %s\n' "$install_dir/leanmgr"
