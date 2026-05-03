#!/usr/bin/env sh
set -eu

repo="${CPL_REPO:-kharkilirov1/cognitive-project-layer}"
version="${VERSION:-latest}"
install_dir="${CPL_INSTALL_DIR:-$HOME/.local/bin}"

need_cmd() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "error: required command '$1' not found" >&2
    exit 1
  fi
}

fetch() {
  url="$1"
  output="$2"
  if command -v curl >/dev/null 2>&1; then
    curl -fsSL "$url" -o "$output"
  elif command -v wget >/dev/null 2>&1; then
    wget -q "$url" -O "$output"
  else
    echo "error: curl or wget is required" >&2
    exit 1
  fi
}

fetch_text() {
  url="$1"
  if command -v curl >/dev/null 2>&1; then
    curl -fsSL "$url"
  elif command -v wget >/dev/null 2>&1; then
    wget -q "$url" -O -
  else
    echo "error: curl or wget is required" >&2
    exit 1
  fi
}

need_cmd tar
need_cmd sed
need_cmd uname

if [ "$version" = "latest" ]; then
  echo "==> Resolving latest release"
  tag="$(fetch_text "https://api.github.com/repos/$repo/releases/latest" | sed -n 's/.*"tag_name":[[:space:]]*"\([^"]*\)".*/\1/p' | head -n 1)"
else
  case "$version" in
    v*) tag="$version" ;;
    *) tag="v$version" ;;
  esac
fi

if [ -z "${tag:-}" ]; then
  echo "error: could not resolve release tag" >&2
  exit 1
fi

os="$(uname -s)"
arch="$(uname -m)"

case "$os" in
  Linux)
    platform="linux"
    ;;
  Darwin)
    platform="macos"
    ;;
  *)
    echo "error: unsupported OS '$os'. Use install.ps1 on Windows." >&2
    exit 1
    ;;
esac

case "$arch" in
  x86_64 | amd64)
    target="$platform-x86_64"
    ;;
  arm64 | aarch64)
    if [ "$platform" = "linux" ]; then
      echo "error: linux-aarch64 prebuilt assets are not published yet" >&2
      exit 1
    fi
    target="$platform-aarch64"
    ;;
  *)
    echo "error: unsupported architecture '$arch'" >&2
    exit 1
    ;;
esac

asset="cognitive-project-layer-$tag-$target.tar.gz"
url="https://github.com/$repo/releases/download/$tag/$asset"
tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/cpl-install.XXXXXX")"
archive="$tmp_dir/$asset"

cleanup() {
  rm -rf "$tmp_dir"
}
trap cleanup EXIT INT TERM

echo "==> Downloading $asset"
fetch "$url" "$archive"

echo "==> Extracting archive"
tar -xzf "$archive" -C "$tmp_dir"

src_dir="$(find "$tmp_dir" -type f -name cpl -perm -u+x -exec dirname {} \; | head -n 1)"
if [ -z "$src_dir" ] || [ ! -x "$src_dir/cpl" ] || [ ! -x "$src_dir/cpl-mcp" ]; then
  echo "error: archive did not contain executable cpl and cpl-mcp binaries" >&2
  exit 1
fi

echo "==> Installing to $install_dir"
mkdir -p "$install_dir"
cp "$src_dir/cpl" "$install_dir/cpl"
cp "$src_dir/cpl-mcp" "$install_dir/cpl-mcp"
chmod 755 "$install_dir/cpl" "$install_dir/cpl-mcp"

echo "==> Installed"
if ! "$install_dir/cpl" --version 2>/dev/null; then
  echo "cpl installed; this release does not support --version"
fi
if ! "$install_dir/cpl-mcp" --version 2>/dev/null; then
  echo "cpl-mcp installed; this release does not support --version"
fi

case ":$PATH:" in
  *":$install_dir:"*) ;;
  *)
    echo ""
    echo "Add this to your shell profile if needed:"
    echo "  export PATH=\"$install_dir:\$PATH\""
    ;;
esac
