#!/bin/sh
# tulving installer: download the latest release binary for this
# platform into ~/.local/bin. No root, no daemon, nothing else touched.
#
#   curl -fsSL https://raw.githubusercontent.com/modiqo/tulving/main/install.sh | sh
#
# Environment:
#   TULVING_INSTALL_DIR   destination (default ~/.local/bin)
#   TULVING_VERSION       tag to install (default: latest release)

set -eu

REPO="modiqo/tulving"
DEST="${TULVING_INSTALL_DIR:-$HOME/.local/bin}"

os=$(uname -s)
arch=$(uname -m)
case "$os/$arch" in
  Darwin/arm64)  target="aarch64-apple-darwin" ;;
  Darwin/x86_64) target="x86_64-apple-darwin" ;;
  Linux/x86_64)  target="x86_64-unknown-linux-gnu" ;;
  Linux/aarch64 | Linux/arm64) target="aarch64-unknown-linux-gnu" ;;
  *)
    echo "tulving: no prebuilt binary for $os/$arch." >&2
    echo "Build from source instead: cargo install --git https://github.com/$REPO tulving-cli" >&2
    exit 1
    ;;
esac

if [ -n "${TULVING_VERSION:-}" ]; then
  tag="$TULVING_VERSION"
else
  tag=$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" |
    sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' | head -1)
  [ -n "$tag" ] || { echo "tulving: cannot resolve the latest release" >&2; exit 1; }
fi

url="https://github.com/$REPO/releases/download/$tag/tulving-$tag-$target.tar.gz"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

echo "Downloading tulving $tag for $target ..."
curl -fsSL "$url" -o "$tmp/tulving.tar.gz"
tar xzf "$tmp/tulving.tar.gz" -C "$tmp"

mkdir -p "$DEST"
install -m 0755 "$tmp/tulving" "$DEST/tulving"

echo "✓ installed $("$DEST/tulving" --version) to $DEST/tulving"
case ":$PATH:" in
  *":$DEST:"*) ;;
  *) echo "  note: $DEST is not on PATH" ;;
esac
echo
echo "Next: register the per-user timer (no daemon):"
echo "  tulving init"
echo "Then keep something running:"
echo "  tulving every morning --why \"...\" -- <command>"
