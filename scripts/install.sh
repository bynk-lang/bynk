#!/bin/sh
# Install the Bynk toolchain (bynkc, bynk, bynkc-lsp) from a GitHub Release,
# without needing Rust or cargo.
#
#   curl -fsSL https://github.com/bynk-lang/bynk/releases/latest/download/install.sh | sh
#
# Options (environment variables):
#   BYNK_VERSION      release tag to install (e.g. v0.16.0); default: the latest
#   BYNK_INSTALL_DIR  where to put the binaries; default: ~/.local/bin
#
# The archive is verified against the release's SHA256SUMS before anything is
# installed. The Linux binaries need a reasonably recent glibc (they are built on
# ubuntu-latest). POSIX sh; needs curl or wget, tar, and sha256sum or shasum.
set -eu

repo="bynk-lang/bynk"
base="https://github.com/$repo/releases"
dest="${BYNK_INSTALL_DIR:-$HOME/.local/bin}"

say() { printf '%s\n' "$*"; }
die() { printf 'bynk-install: %s\n' "$*" >&2; exit 1; }
have() { command -v "$1" >/dev/null 2>&1; }

if have curl; then
  fetch() { curl -fsSL "$1" -o "$2"; }
  # The final URL after redirects: /releases/latest -> /releases/tag/<tag>.
  resolve() { curl -fsSL -o /dev/null -w '%{url_effective}' "$1"; }
elif have wget; then
  fetch() { wget -q "$1" -O "$2"; }
  resolve() { wget -q --max-redirect=5 -S --spider "$1" 2>&1 | sed -n 's/^ *Location: //p' | tail -n 1; }
else
  die "need curl or wget"
fi

case "$(uname -s)" in
  Linux)
    case "$(ldd --version 2>&1 || true)" in
      *musl*) die "the prebuilt Linux binaries need glibc (this looks like musl); use 'cargo install bynkc bynk' instead" ;;
    esac
    os="unknown-linux-gnu" ;;
  Darwin) os="apple-darwin" ;;
  *) die "unsupported OS '$(uname -s)' (on Windows use install.ps1)" ;;
esac
case "$(uname -m)" in
  x86_64 | amd64) arch="x86_64" ;;
  aarch64 | arm64) arch="aarch64" ;;
  *) die "unsupported architecture '$(uname -m)'" ;;
esac
target="$arch-$os"
case "$target" in
  x86_64-unknown-linux-gnu | aarch64-unknown-linux-gnu | x86_64-apple-darwin | aarch64-apple-darwin) ;;
  *) die "no prebuilt binaries for $target; use 'cargo install bynkc bynk' instead" ;;
esac

tag="${BYNK_VERSION:-}"
if [ -z "$tag" ]; then
  tag="$(resolve "$base/latest")"
  case "$tag" in */tag/*) tag="${tag##*/}" ;; *) die "could not determine the latest release (is there a published release?)" ;; esac
fi
case "$tag" in v*) ;; *) tag="v$tag" ;; esac

stage="bynk-$tag-$target"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT INT TERM

say "Installing Bynk $tag ($target) to $dest"
fetch "$base/download/$tag/$stage.tar.gz" "$tmp/$stage.tar.gz" || die "could not download $stage.tar.gz (is $tag a release?)"
fetch "$base/download/$tag/SHA256SUMS" "$tmp/SHA256SUMS" || die "could not download SHA256SUMS"

want="$(awk -v f="$stage.tar.gz" '$2 == f { print $1 }' "$tmp/SHA256SUMS")"
[ -n "$want" ] || die "$stage.tar.gz is not listed in SHA256SUMS"
if have sha256sum; then
  got="$(sha256sum "$tmp/$stage.tar.gz" | awk '{ print $1 }')"
elif have shasum; then
  got="$(shasum -a 256 "$tmp/$stage.tar.gz" | awk '{ print $1 }')"
else
  die "need sha256sum or shasum to verify the download"
fi
[ "$want" = "$got" ] || die "checksum mismatch for $stage.tar.gz (expected $want, got $got)"

tar -xzf "$tmp/$stage.tar.gz" -C "$tmp"
mkdir -p "$dest"
for bin in bynkc bynk bynkc-lsp; do
  # Install via a temp name + rename, so replacing a running binary is safe.
  cp "$tmp/$stage/$bin" "$dest/.$bin.new"
  chmod 755 "$dest/.$bin.new"
  mv -f "$dest/.$bin.new" "$dest/$bin"
done

say "Installed bynkc, bynk and bynkc-lsp to $dest"
case ":$PATH:" in
  *":$dest:"*) say "Run 'bynk doctor' to check your setup." ;;
  *) say "Add it to your PATH, e.g.:  export PATH=\"$dest:\$PATH\""
     say "Then run 'bynk doctor' to check your setup." ;;
esac
