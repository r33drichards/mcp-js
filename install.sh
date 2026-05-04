#!/usr/bin/env bash
set -e

REPO="r33drichards/mcp-js"
INSTALL_DIR="/usr/local/bin"

# What to install: "server", "cli", or "all" (default: "all")
# Override with: MCP_V8_INSTALL=cli ./install.sh
INSTALL="${MCP_V8_INSTALL:-all}"

# Detect latest version if not specified
if [ -z "$MCP_V8_VERSION" ]; then
  MCP_V8_VERSION=$(curl -s "https://api.github.com/repos/$REPO/releases/latest" | grep '"tag_name":' | sed -E 's/.*"tag_name": "([^"]+)".*/\1/')
fi

if [ -z "$MCP_V8_VERSION" ]; then
  echo "Could not determine latest release version. Set MCP_V8_VERSION env var to override."
  exit 1
fi

echo "Installing mcp-v8 version: $MCP_V8_VERSION"

# Detect OS and ARCH
OS=$(uname -s)
ARCH=$(uname -m)

case "$OS" in
  Linux)
    case "$ARCH" in
      x86_64)
        PLATFORM="linux"
        ;;
      aarch64 | arm64)
        PLATFORM="linux-arm64"
        ;;
      *)
        echo "Unsupported Linux architecture: $ARCH"
        exit 1
        ;;
    esac
    ;;
  Darwin)
    if [ "$ARCH" = "arm64" ]; then
      PLATFORM="macos-arm64"
    else
      PLATFORM="macos"
    fi
    ;;
  *)
    echo "Unsupported OS: $OS"
    exit 1
    ;;
esac

# ── Helper: download, extract, and install a single binary ──────────────

install_binary() {
  local ASSET_NAME="$1"     # e.g.  server-mcp-v8-linux
  local INSTALL_AS="$2"     # e.g.  mcp-v8
  local BINARY_GZ="${ASSET_NAME}.gz"
  local DOWNLOAD_URL="https://github.com/$REPO/releases/download/$MCP_V8_VERSION/$BINARY_GZ"

  echo ""
  echo "Downloading $DOWNLOAD_URL"
  curl -L -o "$BINARY_GZ" "$DOWNLOAD_URL"

  echo "Extracting $BINARY_GZ ..."
  gunzip -f "$BINARY_GZ"

  if [ -w "$INSTALL_DIR" ]; then
    mv "$ASSET_NAME" "$INSTALL_DIR/$INSTALL_AS"
    chmod +x "$INSTALL_DIR/$INSTALL_AS"
  else
    sudo mv "$ASSET_NAME" "$INSTALL_DIR/$INSTALL_AS"
    sudo chmod +x "$INSTALL_DIR/$INSTALL_AS"
  fi

  echo "Installed $INSTALL_AS to $INSTALL_DIR/$INSTALL_AS"
}

# ── Install server ───────────────────────────────────────────────────────

if [ "$INSTALL" = "server" ] || [ "$INSTALL" = "all" ]; then
  install_binary "server-mcp-v8-$PLATFORM" "mcp-v8"
  echo "You can now run: mcp-v8"
fi

# ── Install CLI ──────────────────────────────────────────────────────────

if [ "$INSTALL" = "cli" ] || [ "$INSTALL" = "all" ]; then
  install_binary "mcp-v8-cli-$PLATFORM" "mcp-v8-cli"
  echo "You can now run: mcp-v8-cli --help"
fi
