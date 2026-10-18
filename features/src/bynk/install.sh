#!/bin/sh
# Dev Container feature: the Bynk toolchain (#2103).
#
# Installs `bynk`, `bynkc` and `bynkc-lsp` from ONE GitHub Release of
# bynk-lang/bynk, so the driver, compiler and language server never skew,
# verifying the archive against that release's SHA256SUMS before anything is
# unpacked. Then, optionally, `wrangler@4` and `typescript@7` with npm.
#
# Node is not installed here: the feature declares `installsAfter` the official
# Node feature (DECISION B), and `bynk doctor` is what checks the result.
#
# Options arrive as upper-cased environment variables: VERSION, WRANGLER,
# TYPESCRIPT. Runs as root during the image build.
set -eu

REPO="bynk-lang/bynk"
VERSION="${VERSION:-latest}"
WRANGLER="${WRANGLER:-true}"
TYPESCRIPT="${TYPESCRIPT:-true}"
BIN_DIR="/usr/local/bin"

# The npm majors bynk is verified against: wrangler 4 (bynk_emit::WRANGLER_MIN)
# and TypeScript 7 (bynk_emit::TYPESCRIPT_MAJOR_TESTED).
WRANGLER_SPEC="wrangler@4"
TYPESCRIPT_SPEC="typescript@7"

die() {
    echo "bynk feature: error: $*" >&2
    exit 1
}

log() {
    echo "bynk feature: $*"
}

if [ "$(id -u)" -ne 0 ]; then
    die "this script must run as root (it installs into $BIN_DIR)"
fi

# The release matrix ships Linux binaries for these two targets only.
case "$(uname -m)" in
    x86_64 | amd64) TARGET="x86_64-unknown-linux-gnu" ;;
    aarch64 | arm64) TARGET="aarch64-unknown-linux-gnu" ;;
    *) die "unsupported architecture $(uname -m); Bynk releases cover linux x86_64 and aarch64" ;;
esac

# curl, ca-certificates and tar are on the devcontainers base images. Install
# them on a bare Debian/Ubuntu image; anything else must provide them.
missing_tools() {
    for tool in curl tar sha256sum; do
        command -v "$tool" >/dev/null 2>&1 || return 0
    done
    return 1
}
if missing_tools; then
    if command -v apt-get >/dev/null 2>&1; then
        log "installing curl, ca-certificates and tar"
        export DEBIAN_FRONTEND=noninteractive
        apt-get update -y
        apt-get install -y --no-install-recommends curl ca-certificates tar coreutils
        rm -rf /var/lib/apt/lists/*
    else
        die "curl, tar and sha256sum are required"
    fi
fi

# Resolve the release tag. `latest` follows the github.com redirect rather than
# the REST API, so an image build is not subject to the unauthenticated API
# rate limit.
if [ "$VERSION" = "latest" ]; then
    url=$(curl -fsSLI -o /dev/null -w '%{url_effective}' "https://github.com/$REPO/releases/latest") ||
        die "could not resolve the latest release of $REPO"
    TAG="${url##*/tag/}"
    case "$TAG" in
        v[0-9]*) ;;
        *) die "could not resolve the latest release of $REPO (got $url)" ;;
    esac
else
    case "$VERSION" in
        v*) TAG="$VERSION" ;;
        *) TAG="v$VERSION" ;;
    esac
fi
log "installing Bynk $TAG for $TARGET"

ASSET="bynk-$TAG-$TARGET.tar.gz"
BASE="https://github.com/$REPO/releases/download/$TAG"

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

curl -fsSL --retry 3 -o "$WORK/SHA256SUMS" "$BASE/SHA256SUMS" ||
    die "could not download $BASE/SHA256SUMS (is $TAG a published release?)"
curl -fsSL --retry 3 -o "$WORK/$ASSET" "$BASE/$ASSET" ||
    die "could not download $BASE/$ASSET"

# Verify before unpacking. SHA256SUMS lines are `<hex>  <basename>`.
expected=$(awk -v f="$ASSET" '$2 == f { print $1 }' "$WORK/SHA256SUMS")
[ -n "$expected" ] || die "$ASSET is not listed in $TAG's SHA256SUMS"
actual=$(sha256sum "$WORK/$ASSET" | awk '{ print $1 }')
[ "$expected" = "$actual" ] ||
    die "checksum mismatch for $ASSET: expected $expected, got $actual"
log "checksum verified ($ASSET)"

tar -xzf "$WORK/$ASSET" -C "$WORK"
STAGE="$WORK/bynk-$TAG-$TARGET"
for bin in bynk bynkc bynkc-lsp; do
    [ -f "$STAGE/$bin" ] || die "$ASSET has no $bin"
    install -m 0755 "$STAGE/$bin" "$BIN_DIR/$bin"
done

# The binaries are built on the release runner (Ubuntu 24.04) against its
# glibc, so an image with an older glibc may not run them. Fail here, with a
# clearer message than at first use.
"$BIN_DIR/bynk" --version >/dev/null 2>&1 ||
    die "the installed bynk does not run on this image; Bynk releases are built on Ubuntu 24.04, so use a base image with a glibc at least as new"
log "installed $("$BIN_DIR/bynk" --version)"

# npm tools. Node comes from the Node feature, whose PATH is in effect here
# because this feature installs after it.
npm_specs=""
[ "$WRANGLER" = "true" ] && npm_specs="$npm_specs $WRANGLER_SPEC"
[ "$TYPESCRIPT" = "true" ] && npm_specs="$npm_specs $TYPESCRIPT_SPEC"
if [ -n "$npm_specs" ]; then
    command -v npm >/dev/null 2>&1 ||
        die "npm not found. Add \"ghcr.io/devcontainers/features/node:1\": { \"version\": \"22\" } to your devcontainer.json features, or set \"wrangler\": false and \"typescript\": false"
    log "npm install -g$npm_specs"
    # shellcheck disable=SC2086 # the specs are deliberately word-split
    npm install -g --no-fund --no-audit $npm_specs
fi

log "done. Run \`bynk doctor\` to check the environment."
