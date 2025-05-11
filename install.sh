#!/usr/bin/env bash
set -e

REPO="r33drichards/mcp-js"
INSTALL_DIR="/usr/local/bin"

# Detect latest version if not specified
if [ -z "$MCP_V8_VERSION" ]; then
  MCP_V8_VERSION=$(curl -s "https://api.github.com/repos/$REPO/releases/latest" | grep -oP '"tag_name": "\K(.*)(?=")')
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
    PLATFORM="linux"
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

BINARY_NAME="server-mcp-v8-$PLATFORM"
BINARY_GZ="$BINARY_NAME.gz"
DOWNLOAD_URL="https://github.com/$REPO/releases/download/$MCP_V8_VERSION/$BINARY_GZ"

echo "Downloading $DOWNLOAD_URL"
curl -L -o "$BINARY_GZ" "$DOWNLOAD_URL"

echo "Extracting binary..."
gunzip -f "$BINARY_GZ"

# Find install dir
if [ -w "$INSTALL_DIR" ]; then
  TARGET="$INSTALL_DIR/mcp-v8"
  mv "$BINARY_NAME" "$TARGET"
  chmod +x "$TARGET"
else
  TARGET="$INSTALL_DIR/mcp-v8"
  sudo mv "$BINARY_NAME" "$TARGET"
  sudo chmod +x "$TARGET"
fi

echo "Installed mcp-v8 to $TARGET"
echo "You can now run: mcp-v8"