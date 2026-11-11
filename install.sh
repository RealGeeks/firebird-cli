#!/bin/sh
# Install the firebird-cli CLI from its GitHub release archives.
#
# Verifies the sha256 that the Release workflow publishes beside each archive
# and, when the GitHub CLI is installed and logged in, the archive's
# build-provenance attestation.
set -eu

usage() {
  cat <<'USAGE'
Install the firebird-cli CLI from its GitHub release archives.

  sh install.sh [--version vX.Y.Z] [--dir DIR]

Piped from curl, the flags belong to sh, not to this script, so pass them
after `-s --` or use the environment variables:

  curl -fsSL $RAW/install.sh | sh
  curl -fsSL $RAW/install.sh | sh -s -- --version v0.3.0
  curl -fsSL $RAW/install.sh | FIREBIRD_CLI_VERSION=v0.3.0 sh

`curl ... | sh --version v0.3.0` does NOT work: sh takes the flag itself.

Options (flag or environment):
  --version vX.Y.Z   FIREBIRD_CLI_VERSION      a tag; default: the latest release
  --dir DIR          FIREBIRD_CLI_INSTALL_DIR  default: /usr/local/bin if writable,
                                               else ~/.local/bin
                     FIREBIRD_CLI_REPO         default: realgeeks/firebird-cli
                     GH_TOKEN / GITHUB_TOKEN   optional; avoids GitHub's
                                               anonymous API rate limit
                     FIREBIRD_CLI_SKIP_PROVENANCE=1  skip the attestation check

With `gh` installed and logged in (or GH_TOKEN set), the archive must pass
`gh attestation verify` as a build of realgeeks/firebird-cli's release
workflow, whatever FIREBIRD_CLI_REPO says; without `gh` the script says it
could not check.
USAGE
}

# Everything runs from main, called on the last line, so a download cut off
# part-way through executes nothing.
main() {
REPO="${FIREBIRD_CLI_REPO:-realgeeks/firebird-cli}"
VERSION="${FIREBIRD_CLI_VERSION:-}"
INSTALL_DIR="${FIREBIRD_CLI_INSTALL_DIR:-}"
TOKEN="${GH_TOKEN:-${GITHUB_TOKEN:-}}"

while [ $# -gt 0 ]; do
  case "$1" in
    --version) [ $# -ge 2 ] || { echo "install.sh: --version needs a tag" >&2; exit 2; }
               VERSION="$2"; shift 2 ;;
    --version=*) VERSION="${1#--version=}"; shift ;;
    --dir) [ $# -ge 2 ] || { echo "install.sh: --dir needs a directory" >&2; exit 2; }
           INSTALL_DIR="$2"; shift 2 ;;
    --dir=*) INSTALL_DIR="${1#--dir=}"; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "install.sh: unknown option $1" >&2; usage >&2; exit 2 ;;
  esac
done

say() { printf '%s\n' "$*" >&2; }
die() { say "install.sh: $*"; exit 1; }

need() { command -v "$1" >/dev/null 2>&1 || die "$1 is required"; }
need curl
need tar

os=$(uname -s)
arch=$(uname -m)
case "$os" in
  Darwin) os_part="apple-darwin" ;;
  Linux) os_part="unknown-linux-musl" ;; # static: runs whatever the glibc
  MINGW*|MSYS*|CYGWIN*) die "on Windows run install.ps1 instead: irm https://raw.githubusercontent.com/realgeeks/firebird-cli/v0.1.0/install.ps1 | iex" ;; # x-release-please-version
  *) die "unsupported OS: $os" ;;
esac
case "$arch" in
  x86_64|amd64) arch_part="x86_64" ;;
  arm64|aarch64) arch_part="aarch64" ;;
  *) die "unsupported architecture: $arch" ;;
esac
target="$arch_part-$os_part"

auth_header=""
if [ -n "$TOKEN" ]; then
  auth_header="Authorization: Bearer $TOKEN"
fi

api() {
  # $1: API URL. Prints the body.
  if [ -n "$auth_header" ]; then
    curl --proto =https --tlsv1.2 -fsSL -H "$auth_header" -H "Accept: application/vnd.github+json" "$1"
  else
    curl --proto =https --tlsv1.2 -fsSL -H "Accept: application/vnd.github+json" "$1"
  fi
}

if [ -z "$VERSION" ]; then
  release_url="https://api.github.com/repos/$REPO/releases/latest"
else
  case "$VERSION" in v*) ;; *) VERSION="v$VERSION" ;; esac
  release_url="https://api.github.com/repos/$REPO/releases/tags/$VERSION"
fi

release_json=$(api "$release_url") || die "cannot read the release ($release_url). If GitHub is rate limiting you, set GH_TOKEN."
tag=$(printf '%s' "$release_json" | sed -n 's/^ *"tag_name": *"\([^"]*\)".*/\1/p' | head -1)
[ -n "$tag" ] || die "no tag_name in the release response"

name="firebird-cli-$tag-$target"
archive="$name.tar.gz"

# With a token, fetch through the API (which also works for a private fork);
# without one, the public download URL.
asset_api_url() {
  # GitHub lists each asset as url, id, node_id, name on consecutive lines.
  printf '%s\n' "$release_json" | grep -B 5 "\"name\": *\"$1\"" | sed -n 's/^ *"url": *"\(https:\/\/api\.github\.com\/repos\/[^"]*\/assets\/[0-9]*\)".*/\1/p' | tail -1
}
fetch_asset() {
  # $1: asset name, $2: destination
  if [ -n "$auth_header" ]; then
    url=$(asset_api_url "$1")
    [ -n "$url" ] || die "release $tag has no asset $1"
    curl --proto =https --tlsv1.2 -fsSL -H "$auth_header" -H "Accept: application/octet-stream" -o "$2" "$url"
  else
    curl --proto =https --tlsv1.2 -fsSL -o "$2" "https://github.com/$REPO/releases/download/$tag/$1"
  fi
}

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

say "Downloading firebird-cli $tag for $target..."
fetch_asset "$archive" "$tmp/$archive"
fetch_asset "$archive.sha256" "$tmp/$archive.sha256"

expected=$(cut -d' ' -f1 "$tmp/$archive.sha256")
if command -v sha256sum >/dev/null 2>&1; then
  actual=$(sha256sum "$tmp/$archive" | cut -d' ' -f1)
else
  actual=$(shasum -a 256 "$tmp/$archive" | cut -d' ' -f1)
fi
[ "$expected" = "$actual" ] || die "checksum mismatch for $archive (expected $expected, got $actual)"

# Build provenance: the sha256 comes from the same release as the archive,
# the attestation from Sigstore. Always against the official repository.
official="RealGeeks/firebird-cli"
signer="https://github.com/$official/.github/workflows/release.yml@refs/heads/main"
verify_cmd="gh attestation verify $archive --repo $official --cert-identity $signer"
if [ -n "${FIREBIRD_CLI_SKIP_PROVENANCE:-}" ]; then
  say "warning: FIREBIRD_CLI_SKIP_PROVENANCE is set; build provenance not checked"
elif command -v gh >/dev/null 2>&1 && { [ -n "$TOKEN" ] || gh auth status >/dev/null 2>&1; }; then
  gh attestation verify "$tmp/$archive" --repo "$official" --cert-identity "$signer" >/dev/null ||
    die "$archive failed its build-provenance check: it was not built by $official's release workflow, or gh could not reach GitHub. Nothing was installed. To see why: $verify_cmd (FIREBIRD_CLI_SKIP_PROVENANCE=1 skips the check)"
  say "Build provenance verified ($official release workflow)."
else
  say "Build provenance not verified (needs the GitHub CLI, logged in). To check it yourself:"
  say "  $verify_cmd"
fi

tar -xzf "$tmp/$archive" -C "$tmp"
[ -f "$tmp/$name/firebird-cli" ] || die "$archive did not contain $name/firebird-cli"

if [ -z "$INSTALL_DIR" ]; then
  if [ -d /usr/local/bin ] && [ -w /usr/local/bin ]; then
    INSTALL_DIR=/usr/local/bin
  else
    INSTALL_DIR="$HOME/.local/bin"
  fi
fi
mkdir -p "$INSTALL_DIR"
install -m 755 "$tmp/$name/firebird-cli" "$INSTALL_DIR/firebird-cli"

say "Installed $INSTALL_DIR/firebird-cli ($("$INSTALL_DIR/firebird-cli" --version))"
case ":$PATH:" in
  *":$INSTALL_DIR:"*) ;;
  *) say "Add it to your PATH:  export PATH=\"$INSTALL_DIR:\$PATH\"" ;;
esac
say "Next: firebird-cli auth login --site www.yoursite.com --api-key-stdin   (then: firebird-cli guide)"
}

main "$@"
