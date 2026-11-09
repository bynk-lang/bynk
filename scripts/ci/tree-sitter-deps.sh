#!/usr/bin/env bash
# `npm ci` in tree-sitter-bynk/, with the tree-sitter CLI binary downloaded
# through the authenticated API (gh-release-fetch.sh, which says why) instead
# of by tree-sitter-cli's postinstall, whose install.js fetches it anonymously
# from github.com/tree-sitter/tree-sitter/releases/download and has no way to
# take a token.
#
#   tree-sitter-deps.sh [--wasm]
#
# --wasm also fetches the wasi-sdk and binaryen that `tree-sitter build --wasm`
# would otherwise download anonymously, at the versions this CLI release pins,
# and exports TREE_SITTER_WASI_SDK_PATH / TREE_SITTER_BINARYEN_PATH (to
# GITHUB_ENV) so the CLI uses them.
set -euo pipefail

die() { echo "::error::tree-sitter-deps: $*" >&2; exit 1; }
here="$(cd "$(dirname "$0")" && pwd)"
fetch="$here/gh-release-fetch.sh"
cd "$here/../../tree-sitter-bynk"

# Every install script is skipped here: tree-sitter-cli's is the anonymous
# download replaced below, and the package's own (`install`: node-gyp-build,
# the native binding) is run explicitly after it.
npm ci --ignore-scripts

cli="node_modules/tree-sitter-cli"
version="$(node -p "require('./$cli/package.json').version")"
case "${RUNNER_OS:-}-${RUNNER_ARCH:-}" in
  Linux-X64) plat=linux-x64; arch_os=x86_64-linux ;;
  macOS-ARM64) plat=macos-arm64; arch_os=arm64-macos ;;
  macOS-X64) plat=macos-x64; arch_os=x86_64-macos ;;
  Windows-X64) plat=windows-x64; arch_os=x86_64-windows ;;
  *) die "unsupported runner ${RUNNER_OS:-?}-${RUNNER_ARCH:-?}" ;;
esac
exe=tree-sitter; [[ "$plat" == windows-* ]] && exe=tree-sitter.exe

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
# The same asset install.js fetches, unpacked to where cli.js runs it.
"$fetch" tree-sitter/tree-sitter "v$version" "tree-sitter-$plat.gz" "$work"
gunzip -c "$work/tree-sitter-$plat.gz" > "$cli/$exe"
chmod 755 "$cli/$exe"
"$cli/$exe" --version

npm run install

if [[ "${1:-}" == "--wasm" ]]; then
  : "${GITHUB_ENV:?--wasm exports to GITHUB_ENV}"
  : "${RUNNER_TEMP:?--wasm unpacks into RUNNER_TEMP}"
  # The versions the CLI's loader pins (crates/loader/*-version at its tag),
  # and the URLs it builds from them, so this fetches what it would.
  pin() {
    curl -fsSL --proto '=https' --tlsv1.2 --retry 5 --retry-all-errors \
      -H "Authorization: Bearer ${GH_TOKEN:?}" -H "Accept: application/vnd.github.raw" \
      "https://api.github.com/repos/tree-sitter/tree-sitter/contents/crates/loader/$1?ref=v$version" \
      | tr -d '[:space:]'
  }
  wasi="$(pin wasi-sdk-version)"
  binaryen="$(pin binaryen-version)"
  [[ -n "$wasi" && -n "$binaryen" ]] || die "could not read the loader's wasi-sdk/binaryen pins at v$version"

  sdk="$RUNNER_TEMP/tree-sitter-wasi-sdk"
  "$fetch" WebAssembly/wasi-sdk "wasi-sdk-${wasi%.*}" "wasi-sdk-$wasi-$arch_os.tar.gz" "$work"
  mkdir -p "$sdk"
  tar -xzf "$work/wasi-sdk-$wasi-$arch_os.tar.gz" -C "$sdk" --strip-components=1

  opt="$RUNNER_TEMP/tree-sitter-binaryen"
  b_arch_os="${arch_os/arm64-linux/aarch64-linux}"
  "$fetch" WebAssembly/binaryen "version_$binaryen" "binaryen-version_$binaryen-$b_arch_os.tar.gz" "$work"
  mkdir -p "$opt"
  tar -xzf "$work/binaryen-version_$binaryen-$b_arch_os.tar.gz" -C "$opt" --strip-components=1

  [[ -x "$sdk/bin/clang" ]] || die "wasi-sdk $wasi has no bin/clang"
  [[ -x "$opt/bin/wasm-opt" ]] || die "binaryen $binaryen has no bin/wasm-opt"
  {
    echo "TREE_SITTER_WASI_SDK_PATH=$sdk"
    echo "TREE_SITTER_BINARYEN_PATH=$opt"
  } >> "$GITHUB_ENV"
  echo "wasi-sdk $wasi and binaryen $binaryen ready for tree-sitter build --wasm"
fi
