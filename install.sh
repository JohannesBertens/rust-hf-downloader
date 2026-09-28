#!/bin/sh
# rust-hf-downloader installer / upgrader for Linux and macOS (POSIX sh).
#
# One-liner (installs the newest release, or upgrades an existing install):
#   curl -fsSL https://github.com/JohannesBertens/rust-hf-downloader/releases/latest/download/install.sh | sh
#
# With options (note the `sh -s --`):
#   curl -fsSL .../install.sh | sh -s -- --version v2.8.0   # pin a version
#   curl -fsSL .../install.sh | sh -s -- --install-dir /usr/local/bin
#   curl -fsSL .../install.sh | sh -s -- --dry-run           # print the plan
#   curl -fsSL .../install.sh | sh -s -- --uninstall
#
# Equivalent env overrides: VERSION, INSTALL_DIR, RHD_DOWNLOAD_BASE.
# RHD_DOWNLOAD_BASE replaces the GitHub release base URL entirely — useful
# for mirrors and for testing (e.g. a file:///path/to/fake-release dir).
#
# Design notes:
# - Uses the stable asset names published by CI. The
#   `releases/latest/download/<asset>` URL is a GitHub CDN redirect with no
#   API rate limit, so "newest version" resolution needs no api.github.com
#   call and no jq.
# - Verifies the SHA256 checksum from the same release's SHA256SUMS.
# - Cargo-takeover: if a `cargo install rust-hf-downloader` copy exists in
#   $CARGO_HOME/bin (~/.cargo/bin), the installer upgrades it IN PLACE (same
#   location cargo put it), handing `cargo uninstall` over first so cargo's
#   install records stay clean (the Cargo Book forbids editing
#   .crates.toml/.crates2.json by hand). Otherwise, if ~/.cargo/bin exists
#   and is on PATH it is preferred (cargo-binstall convention: the canonical
#   bin dir for Rust CLI tools, typically at the FRONT of PATH because rustup
#   prepends it) — this prevents cargo copies from shadowing release copies.
# - No sudo: installs to ~/.local/bin by default (override with
#   --install-dir). The install is an atomic same-directory rename, so an
#   upgrade never leaves a truncated binary behind.
# - After installing, re-resolves `rust-hf-downloader` the way the shell
#   would (first PATH match) and warns if a different copy shadows the new
#   one (volta-style check).
set -eu

REPO="JohannesBertens/rust-hf-downloader"
BIN_NAME="rust-hf-downloader"
VERSION="${VERSION:-}"
INSTALL_DIR="${INSTALL_DIR:-}"
DRY_RUN=0
FORCE=0
UNINSTALL=0
BASE_OVERRIDE="${RHD_DOWNLOAD_BASE:-}"

say() { printf '%s\n' "$*"; }
warn() { printf 'install.sh: warning: %s\n' "$*" >&2; }
err() { printf 'install.sh: error: %s\n' "$*" >&2; exit 1; }

on_path() { # on_path <dir> — is <dir> an entry of $PATH?
  case ":$PATH:" in *":$1:"*) return 0 ;; *) return 1 ;; esac
}

usage() {
  cat <<'EOF'
rust-hf-downloader installer (Linux/macOS)

Installs or upgrades rust-hf-downloader from GitHub Releases.

Options:
  --version vX.Y.Z    Install a specific release tag (default: newest)
  --install-dir DIR   Installation directory (default: ~/.local/bin)
  --dry-run           Print what would be done and exit
  --force             Reinstall even if the same version is present
  --uninstall         Remove the installed binary
  -h, --help          Show this help

Environment:
  VERSION             Same as --version
  INSTALL_DIR         Same as --install-dir
  RHD_DOWNLOAD_BASE   Override the release download base URL (mirrors/testing)
EOF
}

while [ $# -gt 0 ]; do
  case "$1" in
    --version)
      [ $# -ge 2 ] || err "--version requires a value"
      VERSION="$2"; shift 2 ;;
    --version=*)
      VERSION="${1#*=}"; shift ;;
    --install-dir|--prefix)
      [ $# -ge 2 ] || err "$1 requires a value"
      INSTALL_DIR="$2"; shift 2 ;;
    --install-dir=*|--prefix=*)
      INSTALL_DIR="${1#*=}"; shift ;;
    --dry-run) DRY_RUN=1; shift ;;
    --force) FORCE=1; shift ;;
    --uninstall) UNINSTALL=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) err "unknown option: $1 (see --help)" ;;
  esac
done

[ -n "${HOME:-}" ] || err 'HOME is not set; cannot determine default install dir'

# --- Default install dir: cargo takeover / cargo-binstall convention --------
CARGO_BIN="${CARGO_HOME:-$HOME/.cargo}/bin"
TAKEOVER_CARGO=0
if [ -z "$INSTALL_DIR" ]; then
  if [ -x "$CARGO_BIN/$BIN_NAME" ]; then
    # A `cargo install rust-hf-downloader` copy lives here: upgrade it in
    # place so there stays exactly one binary, where PATH already points.
    INSTALL_DIR="$CARGO_BIN"
    TAKEOVER_CARGO=1
  elif [ -d "$CARGO_BIN" ] && on_path "$CARGO_BIN"; then
    # cargo-binstall convention: Rust CLI binaries live in $CARGO_HOME/bin.
    INSTALL_DIR="$CARGO_BIN"
  else
    INSTALL_DIR="$HOME/.local/bin"
  fi
fi
DEST="$INSTALL_DIR/$BIN_NAME"

# --- Platform detection ------------------------------------------------------
OS="$(uname -s)"
ARCH="$(uname -m)"
case "$OS" in
  Linux) KERNEL_SUFFIX="unknown-linux-gnu" ;;
  Darwin) KERNEL_SUFFIX="apple-darwin" ;;
  *)
    err "unsupported OS: $OS (supported: Linux, macOS — on Windows use install.ps1)" ;;
esac
case "$ARCH" in
  x86_64|amd64) TRIPLE="x86_64-$KERNEL_SUFFIX" ;;
  aarch64|arm64) TRIPLE="aarch64-$KERNEL_SUFFIX" ;;
  *) err "unsupported architecture: $ARCH (supported: x86_64/amd64, aarch64/arm64)" ;;
esac

ASSET="$BIN_NAME-$TRIPLE.tar.gz"

# --- Resolve the release download base ----------------------------------------
if [ -n "$BASE_OVERRIDE" ]; then
  BASE="$BASE_OVERRIDE"
elif [ -n "$VERSION" ]; then
  case "$VERSION" in
    v*) ;;
    *) VERSION="v$VERSION" ;;
  esac
  BASE="https://github.com/$REPO/releases/download/$VERSION"
else
  BASE="https://github.com/$REPO/releases/latest/download"
fi

# --- Uninstall ----------------------------------------------------------------
if [ "$UNINSTALL" = 1 ]; then
  if [ -f "$DEST" ] || [ -L "$DEST" ]; then
    [ "$DRY_RUN" = 1 ] && { say "dry-run: would remove $DEST"; exit 0; }
    rm -f "$DEST"
    say "removed $DEST"
  else
    say "nothing to uninstall: $DEST does not exist"
  fi
  exit 0
fi

say "plan: install $BIN_NAME ($TRIPLE)"
say "  from: $BASE"
say "  to:   $DEST"
[ "$TAKEOVER_CARGO" = 1 ] && say "  note: taking over the cargo-installed copy in $CARGO_BIN (in-place upgrade)"
if [ "$DRY_RUN" = 1 ]; then
  say "dry-run: stopping before any download"
  exit 0
fi

# --- Helpers -------------------------------------------------------------------
fetch() { # fetch <url> <dest>
  if command -v curl >/dev/null 2>&1; then
    curl -fsSL "$1" -o "$2"
  elif command -v wget >/dev/null 2>&1; then
    wget -q -O "$2" "$1"
  else
    err "neither curl nor wget is available; install one and retry"
  fi
}

file_sha256() { # file_sha256 <file> -> digest on stdout
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | cut -d' ' -f1
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$1" | cut -d' ' -f1
  else
    return 127
  fi
}

# --- Download + verify -----------------------------------------------------------
TMP="$(mktemp -d 2>/dev/null || mktemp -d -t rhd-install)"
cleanup() { rm -rf "$TMP"; }
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

fetch "$BASE/SHA256SUMS" "$TMP/SHA256SUMS"
WANT="$(awk -v f="$ASSET" '$2 == f || $2 == "*" f { print $1; exit }' "$TMP/SHA256SUMS")"
[ -n "$WANT" ] || err "SHA256SUMS at $BASE has no entry for $ASSET (unsupported platform for this release?)"

fetch "$BASE/$ASSET" "$TMP/$ASSET"
GOT="$(file_sha256 "$TMP/$ASSET")" || err "no SHA256 tool found (need sha256sum or shasum)"
[ "$GOT" = "$WANT" ] || err "checksum mismatch for $ASSET
  expected: $WANT
  actual:   $GOT"
say "checksum ok ($WANT)"

tar -xzf "$TMP/$ASSET" -C "$TMP"
[ -f "$TMP/$BIN_NAME" ] || err "archive did not contain $BIN_NAME"
chmod +x "$TMP/$BIN_NAME"

# --- Upgrade check ----------------------------------------------------------------
NEW_VER="$("$TMP/$BIN_NAME" --version 2>/dev/null || true)"
if [ -f "$DEST" ]; then
  OLD_VER="$("$DEST" --version 2>/dev/null || true)"
  if [ -n "$NEW_VER" ] && [ "$OLD_VER" = "$NEW_VER" ] && [ "$FORCE" != 1 ]; then
    say "already up to date: $OLD_VER at $DEST (use --force to reinstall)"
    exit 0
  fi
  [ -n "$OLD_VER" ] && [ -n "$NEW_VER" ] && say "upgrading: $OLD_VER -> $NEW_VER"
fi

# --- Install (atomic rename within the destination directory) ----------------------
if [ "$TAKEOVER_CARGO" = 1 ]; then
  # Let cargo forget its install record via the sanctioned path (the Cargo
  # Book forbids editing .crates.toml/.crates2.json by hand). Best effort:
  # without cargo on PATH we simply overwrite the binary.
  if command -v cargo >/dev/null 2>&1; then
    say "running 'cargo uninstall rust-hf-downloader' to release cargo's install record"
    cargo uninstall rust-hf-downloader >/dev/null 2>&1 || true
  else
    warn "cargo not found on PATH — replacing the binary without cleaning cargo's install record"
  fi
fi

if ! mkdir -p "$INSTALL_DIR" 2>/dev/null; then
  err "cannot create $INSTALL_DIR — set INSTALL_DIR to a writable directory
  (for a system-wide install, put the binary there yourself with appropriate
  privileges, e.g. INSTALL_DIR=/usr/local/bin run via sudo)"
fi
[ -w "$INSTALL_DIR" ] || err "$INSTALL_DIR is not writable (set INSTALL_DIR or fix permissions)"

cp "$TMP/$BIN_NAME" "$DEST.new"
mv -f "$DEST.new" "$DEST"

say "installed: $DEST"
[ -n "$NEW_VER" ] && "$DEST" --version

# --- Volta-style shadow check: what will the shell actually resolve? ---------------
RESOLVED="$(command -v "$BIN_NAME" 2>/dev/null || true)"
if [ -n "$RESOLVED" ] && [ "$RESOLVED" != "$DEST" ]; then
  warn "another $BIN_NAME is earlier on your PATH:
  $RESOLVED
  shadows the newly installed $DEST — typing '$BIN_NAME' will run the OTHER one.
  Remove the old copy (e.g. 'cargo uninstall $BIN_NAME', 'brew uninstall ...',
  or delete the file) or reorder your PATH so $INSTALL_DIR comes first."
elif [ -z "$RESOLVED" ]; then
  case ":$PATH:" in
    *":$INSTALL_DIR:"*) ;;
    *)
      say "NOTE: $INSTALL_DIR is not on your PATH. Add it to your shell profile:"
      say "  export PATH=\"\$PATH:$INSTALL_DIR\""
      ;;
  esac
fi
say "done — run '$BIN_NAME' to start (re-run this one-liner any time to upgrade)"
