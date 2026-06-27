#!/bin/sh
# Install the latest charles-mcp release binary.
#   curl -fsSL https://raw.githubusercontent.com/cbxss/charles-mcp/main/install.sh | sh
# Override the install dir with BIN_DIR=/somewhere.
set -e

repo="cbxss/charles-mcp"
case "$(uname -s)-$(uname -m)" in
  Darwin-arm64)        target="aarch64-apple-darwin" ;;
  Darwin-x86_64)       target="x86_64-apple-darwin" ;;
  Linux-x86_64|Linux-amd64) target="x86_64-unknown-linux-gnu" ;;
  *) echo "no prebuilt binary for $(uname -s)-$(uname -m); build from source with: cargo install --git https://github.com/$repo" >&2; exit 1 ;;
esac

bin_dir="${BIN_DIR:-$HOME/.local/bin}"
url="https://github.com/$repo/releases/latest/download/charles-mcp-$target.tar.gz"

mkdir -p "$bin_dir"
echo "Downloading charles-mcp ($target) -> $bin_dir"
curl -fsSL "$url" | tar xz -C "$bin_dir"
chmod +x "$bin_dir/charles-mcp"

echo "Installed $bin_dir/charles-mcp"
echo
echo "Register it with Claude Code:"
echo "  claude mcp add charles -- $bin_dir/charles-mcp"
