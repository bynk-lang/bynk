#!/usr/bin/env bash
# Install a prebuilt Rust dev tool from its GitHub release, downloaded through
# the authenticated API (gh-release-fetch.sh, which says why). Replaces
# taiki-e/install-action, whose downloads are anonymous: its token goes only
# to its cargo-binstall fallback, never to its own asset fetches.
#
#   install-tool.sh <tool>[@<version>]
#
# <version> is exact (0.2.127), a line (0.9 = newest 0.9.x), or absent (newest).
# The binaries go next to `cargo` (~/.cargo/bin), as install-action put them.
# Only the tools and platforms CI uses are mapped; add a case to add one.
set -euo pipefail

die() { echo "::error::install-tool: $*" >&2; exit 1; }
spec="${1:?usage: install-tool.sh <tool>[@<version>]}"
tool="${spec%%@*}"
want=""; [[ "$spec" == *@* ]] && want="${spec#*@}"
fetch="$(dirname "$0")/gh-release-fetch.sh"

case "${RUNNER_OS:-}-${RUNNER_ARCH:-}" in
  Linux-X64) os=linux ;;
  macOS-ARM64) os=macos-arm64 ;;
  macOS-X64) os=macos-x64 ;;
  Windows-X64) os=windows ;;
  *) die "unsupported runner ${RUNNER_OS:-?}-${RUNNER_ARCH:-?}" ;;
esac
exe=""; [[ "$os" == windows ]] && exe=".exe"

# Per tool: the repo, its tag for a version (tag_of), the asset name for a
# version + target (asset_of), the binaries the archive holds, and the target
# triple for this runner.
case "$tool" in
  nextest|cargo-nextest)
    repo=nextest-rs/nextest; prefix="cargo-nextest-"; bins=(cargo-nextest)
    case "$os" in
      linux) target=x86_64-unknown-linux-gnu ;;
      macos-*) target=universal-apple-darwin ;;
      windows) target=x86_64-pc-windows-msvc ;;
    esac
    asset_of() { echo "cargo-nextest-$1-$target.tar.gz"; } ;;
  cargo-audit)
    repo=rustsec/rustsec; prefix="cargo-audit/v"; bins=(cargo-audit)
    case "$os" in
      linux) target=x86_64-unknown-linux-gnu ;;
      macos-arm64) target=aarch64-apple-darwin ;;
      windows) target=x86_64-pc-windows-msvc ;;
      *) die "cargo-audit has no $os build" ;;
    esac
    asset_of() { [[ "$os" == windows ]] && echo "cargo-audit-$target-v$1.zip" || echo "cargo-audit-$target-v$1.tgz"; } ;;
  cargo-llvm-cov)
    repo=taiki-e/cargo-llvm-cov; prefix="v"; bins=(cargo-llvm-cov)
    case "$os" in
      linux) target=x86_64-unknown-linux-gnu ;;
      macos-*) target=universal-apple-darwin ;;
      windows) target=x86_64-pc-windows-msvc ;;
    esac
    asset_of() { echo "cargo-llvm-cov-$target.tar.gz"; } ;;
  wasm-bindgen|wasm-bindgen-cli)
    repo=wasm-bindgen/wasm-bindgen; prefix=""; bins=(wasm-bindgen wasm-bindgen-test-runner wasm2es6js)
    case "$os" in
      linux) target=x86_64-unknown-linux-musl ;;
      macos-arm64) target=aarch64-apple-darwin ;;
      windows) target=x86_64-pc-windows-msvc ;;
      *) die "wasm-bindgen has no $os build" ;;
    esac
    asset_of() { echo "wasm-bindgen-$1-$target.tar.gz"; } ;;
  *) die "unknown tool $tool (add it to scripts/ci/install-tool.sh)" ;;
esac

# Resolve the version: an exact x.y.z is used as-is, anything shorter is the
# newest release on that line.
if [[ "$want" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  tag="$prefix$want"
else
  line="$prefix$want"; [[ -n "$want" ]] && line="$line."
  # An empty prefix and version would match every tag; wasm-bindgen is
  # always pinned, so require one there.
  [[ -n "$line" ]] || die "$tool needs an exact version"
  tag=$("$fetch" --latest "$repo" "$line")
fi
version="${tag#"$prefix"}"
asset="$(asset_of "$version")"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
"$fetch" "$repo" "$tag" "$asset" "$work"
mkdir -p "$work/x"
case "$asset" in
  *.zip) unzip -q "$work/$asset" -d "$work/x" ;;
  *) tar -xzf "$work/$asset" -C "$work/x" ;;
esac

cargo_path="$(command -v cargo)" || die "cargo is not on PATH"
cargo_bin="$(dirname "$cargo_path")"
for b in "${bins[@]}"; do
  found="$(find "$work/x" -type f -name "$b$exe" | head -n1)"
  [[ -n "$found" ]] || die "$asset holds no $b$exe"
  install -m 755 "$found" "$cargo_bin/$b$exe"
  echo "installed $b $version -> $cargo_bin/$b$exe"
done
