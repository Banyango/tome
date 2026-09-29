#!/bin/sh
# Install tome from a GitHub release.
#
#   curl -fsSL https://raw.githubusercontent.com/banyango/tome/main/install.sh | sh
#
# TOME_INSTALL_DIR  where to put `tome` (default: ~/.local/bin)
# TOME_VERSION      a release to install, like v0.1.0 (default: the latest)
set -eu

REPO="banyango/tome"

say() { printf '%s\n' "$*"; }
die() { printf 'install.sh: %s\n' "$*" >&2; exit 1; }

unsupported() {
    die "$1
tome ships builds for macOS and Linux on arm64 and x86_64. To build from source instead:
  git clone https://github.com/$REPO && cd tome && cargo build --release --features bundled"
}

need() { command -v "$1" >/dev/null 2>&1 || die "$1 is required but not installed"; }

need tar
need uname
if command -v curl >/dev/null 2>&1; then
    fetch() { curl -fsSL "$1" -o "$2"; }
elif command -v wget >/dev/null 2>&1; then
    fetch() { wget -q "$1" -O "$2"; }
else
    die "curl or wget is required but neither is installed"
fi

if command -v sha256sum >/dev/null 2>&1; then
    sha256() { sha256sum "$1" | cut -d ' ' -f 1; }
elif command -v shasum >/dev/null 2>&1; then
    sha256() { shasum -a 256 "$1" | cut -d ' ' -f 1; }
else
    die "sha256sum or shasum is required to check the download"
fi

case "$(uname -s)" in
    Darwin) os="apple-darwin" ;;
    Linux) os="unknown-linux-gnu" ;;
    *) unsupported "unsupported operating system: $(uname -s)" ;;
esac
case "$(uname -m)" in
    arm64 | aarch64) arch="aarch64" ;;
    x86_64 | amd64) arch="x86_64" ;;
    *) unsupported "unsupported architecture: $(uname -m)" ;;
esac
target="$arch-$os"

version="${TOME_VERSION:-}"
if [ -n "${TOME_DOWNLOAD_BASE:-}" ]; then
    # For testing against a local directory (file:// works with curl).
    base="$TOME_DOWNLOAD_BASE"
elif [ -n "$version" ]; then
    base="https://github.com/$REPO/releases/download/$version"
else
    version="latest"
    base="https://github.com/$REPO/releases/latest/download"
fi

# The tarball is named after the tag, so the latest release's name has to be
# looked up from its checksum list.
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT INT TERM

sums_url="$base/SHA256SUMS"
fetch "$sums_url" "$tmp/SHA256SUMS" || die "couldn't download $sums_url (does the release exist?)"

line="$(grep -e "-$target.tar.gz\$" "$tmp/SHA256SUMS" | head -n 1 || true)"
[ -n "$line" ] || die "release $version has no build for $target (looked in $sums_url)"
expected="$(printf '%s' "$line" | cut -d ' ' -f 1)"
file="$(printf '%s' "$line" | sed 's/^[^ ]* *\*\{0,1\}//')"

url="$base/$file"
say "Downloading $url"
fetch "$url" "$tmp/$file" || die "couldn't download $url"

actual="$(sha256 "$tmp/$file")"
if [ "$actual" != "$expected" ]; then
    die "checksum mismatch for $file (expected $expected, got $actual); nothing was installed"
fi

tar -xzf "$tmp/$file" -C "$tmp" || die "couldn't unpack $file"
bin="$(find "$tmp" -type f -name tome | head -n 1)"
[ -n "$bin" ] || die "$file has no tome binary in it"

dir="${TOME_INSTALL_DIR:-$HOME/.local/bin}"
mkdir -p "$dir" || die "couldn't create $dir (set TOME_INSTALL_DIR to somewhere you can write)"
# Copy beside the target, then rename, so a running tome isn't overwritten in place.
cp "$bin" "$dir/.tome.new" && chmod 755 "$dir/.tome.new" && mv -f "$dir/.tome.new" "$dir/tome" \
    || die "couldn't write $dir/tome"

say "Installed tome to $dir/tome"
case ":$PATH:" in
    *":$dir:"*) ;;
    *)
        say ""
        say "$dir isn't on your PATH. Add it, for example:"
        say "  export PATH=\"$dir:\$PATH\""
        ;;
esac
say "Run 'tome --help' to get started."
