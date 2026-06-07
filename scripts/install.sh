#!/usr/bin/env bash
#
# Cross-platform install / update script for leanmgr.
#
# Works on macOS, Linux, and Windows (Git Bash / MSYS / WSL).
# Prefers Homebrew when available; otherwise builds from source.
#
# Usage:
#   scripts/install.sh                 # auto: brew first, else source build
#   curl -fsSL https://raw.githubusercontent.com/FrankieeW/leanmgr/main/scripts/install.sh | bash
#
# Environment overrides:
#   LEANMGR_NO_BREW=1   Skip Homebrew even if installed.
#   LEANMGR_VERSION     Git tag for the cargo fallback (default: v0.1.0).
#   INSTALL_DIR         Target dir for the local-build path (default: ~/.local/bin).

set -euo pipefail

REPO="FrankieeW/leanmgr"
TAP="frankieew/tap/leanmgr"
VERSION="${LEANMGR_VERSION:-v0.1.0}"
INSTALL_DIR="${INSTALL_DIR:-$HOME/.local/bin}"

log()  { printf '\033[1;34m==>\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33mwarning:\033[0m %s\n' "$*" >&2; }
die()  { printf '\033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }

have() { command -v "$1" >/dev/null 2>&1; }

detect_os() {
  case "$(uname -s)" in
    Darwin) echo macos ;;
    Linux) echo linux ;;
    MINGW* | MSYS* | CYGWIN*) echo windows ;;
    *) echo unknown ;;
  esac
}

exe_suffix() {
  [ "$(detect_os)" = windows ] && echo ".exe" || echo ""
}

install_via_brew() {
  log "Homebrew detected; using brew (preferred path)."
  if brew list leanmgr >/dev/null 2>&1; then
    log "leanmgr already installed; updating..."
    brew update >/dev/null 2>&1 || warn "brew update failed; continuing with cached formulae"
    brew upgrade "$TAP" || warn "brew upgrade reported nothing to do (already latest?)"
  else
    log "Installing leanmgr via brew..."
    brew install "$TAP"
  fi
}

install_via_local_build() {
  local repo_root="$1"
  local suffix binary
  suffix="$(exe_suffix)"
  binary="$repo_root/target/release/leanmgr$suffix"

  have cargo || die "cargo not found. Install Rust from https://rustup.rs or use Homebrew."
  log "Building from local checkout (cargo build --release)..."
  (cd "$repo_root" && cargo build --release)

  mkdir -p "$INSTALL_DIR"
  cp "$binary" "$INSTALL_DIR/leanmgr$suffix"
  log "Installed leanmgr to $INSTALL_DIR/leanmgr$suffix"
  warn "Ensure $INSTALL_DIR is on your PATH."
}

install_via_cargo_git() {
  have cargo || die "cargo not found. Install Rust from https://rustup.rs or use Homebrew."
  log "Installing from source via cargo (git tag $VERSION)..."
  cargo install --locked --force --git "https://github.com/$REPO" --tag "$VERSION"
  warn "Ensure Cargo's bin dir (e.g. ~/.cargo/bin) is on your PATH."
}

main() {
  local os repo_root
  os="$(detect_os)"
  log "Detected platform: $os"

  if [ -z "${LEANMGR_NO_BREW:-}" ] && have brew; then
    install_via_brew
  else
    [ -n "${LEANMGR_NO_BREW:-}" ] && log "LEANMGR_NO_BREW set; skipping Homebrew."
    have brew || log "Homebrew not found; falling back to a source build."

    # Prefer a local checkout when this script is run from inside the repo.
    repo_root="$(cd "$(dirname "${BASH_SOURCE[0]:-$0}")/.." 2>/dev/null && pwd || true)"
    if [ -n "$repo_root" ] && [ -f "$repo_root/Cargo.toml" ]; then
      install_via_local_build "$repo_root"
    else
      install_via_cargo_git
    fi
  fi

  if have leanmgr; then
    log "Done: $(leanmgr --version)"
  else
    warn "leanmgr is installed but not yet on PATH. Open a new shell or update PATH."
  fi
}

main "$@"
