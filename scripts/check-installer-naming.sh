#!/bin/sh
# Static guard: the release asset names that release.yml's `binaries` job
# produces must still be the ones install.sh, install.ps1 and the
# [package.metadata.binstall] tables expect. A rename there would otherwise
# break all three install paths silently, until someone installs.
set -eu

cd "$(dirname "$0")/.."

fail=0
need() { # need <file> <fixed string> <why>
  if ! grep -qF -- "$2" "$1"; then
    printf 'installer-naming: %s no longer contains %s (%s)\n' "$1" "$2" "$3" >&2
    fail=1
  fi
}

rel=.github/workflows/release.yml
need "$rel" 'stage="bynk-${GITHUB_REF_NAME}-${{ matrix.target }}"' 'unix archive stem'
need "$rel" 'tar czf "$stage.tar.gz" "$stage"' 'unix archive'
need "$rel" '$stage = "bynk-$env:GITHUB_REF_NAME-${{ matrix.target }}"' 'windows archive stem'
need "$rel" '-DestinationPath "$stage.zip"' 'windows archive'
need "$rel" '> SHA256SUMS' 'checksum file'
need "$rel" 'cp "$bin/bynkc" "$bin/bynk" "$bin/bynkc-lsp"' 'unix archive contents'
need "$rel" 'Copy-Item "$bin/bynkc.exe","$bin/bynk.exe","$bin/bynkc-lsp.exe"' 'windows archive contents'

need scripts/install.sh 'stage="bynk-$tag-$target"' 'archive stem'
need scripts/install.sh '$stage.tar.gz' 'archive suffix'
need scripts/install.sh 'SHA256SUMS' 'checksum file'
need scripts/install.ps1 '$stage = "bynk-$tag-$target"' 'archive stem'
need scripts/install.ps1 '$stage.zip' 'archive suffix'
need scripts/install.ps1 'SHA256SUMS' 'checksum file'

# Every target the scripts name must be built by the release matrix.
for t in x86_64-unknown-linux-gnu aarch64-unknown-linux-gnu \
         x86_64-apple-darwin aarch64-apple-darwin x86_64-pc-windows-msvc; do
  need "$rel" "target: $t" 'release matrix target'
done
for t in x86_64-unknown-linux-gnu aarch64-unknown-linux-gnu \
         x86_64-apple-darwin aarch64-apple-darwin; do
  need scripts/install.sh "$t" 'supported target'
done
need scripts/install.ps1 "x86_64-pc-windows-msvc" 'supported target'

# binstall: same layout in every installable crate.
url='pkg-url = "{ repo }/releases/download/v{ version }/bynk-v{ version }-{ target }{ archive-suffix }"'
bindir='bin-dir = "bynk-v{ version }-{ target }/{ bin }{ binary-ext }"'
for c in bynk bynkc bynk-lsp; do
  need "$c/Cargo.toml" "$url" 'binstall pkg-url'
  need "$c/Cargo.toml" "$bindir" 'binstall bin-dir'
  need "$c/Cargo.toml" 'pkg-fmt = "tgz"' 'binstall default format'
  need "$c/Cargo.toml" '[package.metadata.binstall.overrides.x86_64-pc-windows-msvc]' 'binstall windows override'
done

[ "$fail" = 0 ] && echo "Installer asset naming matches release.yml."
exit "$fail"
