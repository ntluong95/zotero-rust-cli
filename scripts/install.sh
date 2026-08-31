#!/bin/sh
# zotero-cli bootstrap installer for macOS and Linux
# https://github.com/ntluong95/zotero-rust-cli
#
# Supported targets:
#   macOS arm64:           aarch64-apple-darwin
#   macOS x86_64:          x86_64-apple-darwin
#   Linux x86_64:          x86_64-unknown-linux-gnu
#   Linux arm64/aarch64:   aarch64-unknown-linux-gnu
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/ntluong95/zotero-rust-cli/main/scripts/install.sh -o /tmp/install-zotero-cli.sh
#   sh /tmp/install-zotero-cli.sh
#
# Environment variables:
#   ZOTERO_CLI_INSTALL_DIR  Custom install directory (default: ~/.local/bin)
#   ZOTERO_CLI_VERSION      Release version to install (default: latest stable)
#   ZOTERO_CLI_REPO         GitHub repository (default: ntluong95/zotero-rust-cli)
#   ZOTERO_CLI_BASE_URL     Custom base download URL (for mirrors / testing)

set -eu

REPO="${ZOTERO_CLI_REPO:-ntluong95/zotero-rust-cli}"
INSTALL_DIR="${ZOTERO_CLI_INSTALL_DIR:-$HOME/.local/bin}"
VERSION="${ZOTERO_CLI_VERSION:-}"
BASE_URL="${ZOTERO_CLI_BASE_URL:-}"

# Parse optional command-line flags
while [ $# -gt 0 ]; do
  case "$1" in
    -d|--dir|--install-dir)
      INSTALL_DIR="$2"
      shift 2
      ;;
    -v|--version)
      VERSION="$2"
      shift 2
      ;;
    -h|--help)
      echo "Usage: install.sh [OPTIONS]"
      echo ""
      echo "Options:"
      echo "  -d, --dir DIR        Install directory (default: ~/.local/bin)"
      echo "  -v, --version VER    Version to install (default: latest stable)"
      echo "  -h, --help           Show this help message"
      exit 0
      ;;
    *)
      if [ -z "${CUSTOM_DIR_SET:-}" ]; then
        INSTALL_DIR="$1"
        CUSTOM_DIR_SET=1
        shift
      else
        echo "Error: Unknown argument: $1" >&2
        exit 1
      fi
      ;;
  esac
done

# 1. Detect operating system
OS_RAW="$(uname -s)"
case "$OS_RAW" in
  Darwin)
    OS="apple-darwin"
    ;;
  Linux)
    OS="unknown-linux-gnu"
    ;;
  *)
    echo "Error: Unsupported operating system: $OS_RAW." >&2
    echo "Supported operating systems: macOS (Darwin), Linux." >&2
    exit 1
    ;;
esac

# 2. Detect architecture
ARCH_RAW="$(uname -m)"
case "$ARCH_RAW" in
  arm64|aarch64)
    ARCH="aarch64"
    ;;
  x86_64|amd64)
    ARCH="x86_64"
    ;;
  *)
    echo "Error: Unsupported architecture: $ARCH_RAW on $OS_RAW." >&2
    echo "Supported architectures: x86_64, aarch64/arm64." >&2
    exit 1
    ;;
esac

TARGET="${ARCH}-${OS}"

# 3. Determine release URLs
if [ -z "$BASE_URL" ]; then
  if [ -n "$VERSION" ]; then
    case "$VERSION" in
      v*) TAG="$VERSION" ;;
      *)  TAG="v$VERSION" ;;
    esac
    BASE_URL="https://github.com/${REPO}/releases/download/${TAG}"
  else
    BASE_URL="https://github.com/${REPO}/releases/latest/download"
  fi
fi

# 4. Check for download utilities
download_file() {
  url="$1"
  dest="$2"
  if command -v curl >/dev/null 2>&1; then
    curl -fsSL "$url" -o "$dest"
  elif command -v wget >/dev/null 2>&1; then
    wget -qO "$dest" "$url"
  else
    echo "Error: curl or wget is required to download zotero-cli." >&2
    return 1
  fi
}

# 5. Create temporary directory with cleanup trap
TMPDIR="$(mktemp -d 2>/dev/null || mktemp -d -t 'zotero-cli-install')"
cleanup() {
  rm -rf "$TMPDIR"
}
trap cleanup EXIT INT TERM

# 6. Download SHA256SUMS
SHA_FILE="$TMPDIR/SHA256SUMS"
if ! download_file "${BASE_URL}/SHA256SUMS" "$SHA_FILE"; then
  echo "Error: Failed to download SHA256SUMS from ${BASE_URL}/SHA256SUMS" >&2
  exit 1
fi

# 7. Download release archive (try alias first, then versioned if specified)
ALIAS_NAME="zotero-cli-${TARGET}.tar.gz"
VERSIONED_NAME=""
if [ -n "$VERSION" ]; then
  case "$VERSION" in
    v*) VERSIONED_NAME="zotero-cli-${VERSION}-${TARGET}.tar.gz" ;;
    *)  VERSIONED_NAME="zotero-cli-v${VERSION}-${TARGET}.tar.gz" ;;
  esac
fi

ARCHIVE_PATH="$TMPDIR/$ALIAS_NAME"
MATCH_NAME="$ALIAS_NAME"

if ! download_file "${BASE_URL}/${ALIAS_NAME}" "$ARCHIVE_PATH"; then
  if [ -n "$VERSIONED_NAME" ] && download_file "${BASE_URL}/${VERSIONED_NAME}" "$TMPDIR/$VERSIONED_NAME"; then
    ARCHIVE_PATH="$TMPDIR/$VERSIONED_NAME"
    MATCH_NAME="$VERSIONED_NAME"
  else
    echo "Error: Failed to download release archive for target ${TARGET} from ${BASE_URL}" >&2
    exit 1
  fi
fi

# 8. Verify SHA-256 checksum
EXPECTED_HASH=$(awk -v asset="$MATCH_NAME" '
  $2 == asset || $2 == ("*" asset) || $2 ~ ("/" asset "$") { print $1 }
' "$SHA_FILE" | head -n 1)

if [ -z "$EXPECTED_HASH" ]; then
  # If alias was downloaded, also check if the versioned name is in SHA256SUMS
  if [ -n "$VERSIONED_NAME" ]; then
    EXPECTED_HASH=$(awk -v asset="$VERSIONED_NAME" '
      $2 == asset || $2 == ("*" asset) || $2 ~ ("/" asset "$") { print $1 }
    ' "$SHA_FILE" | head -n 1)
  fi
fi

if [ -z "$EXPECTED_HASH" ]; then
  echo "Error: Checksum for $MATCH_NAME not found in SHA256SUMS." >&2
  exit 1
fi

if command -v sha256sum >/dev/null 2>&1; then
  ACTUAL_HASH=$(sha256sum "$ARCHIVE_PATH" | awk '{print $1}')
elif command -v shasum >/dev/null 2>&1; then
  ACTUAL_HASH=$(shasum -a 256 "$ARCHIVE_PATH" | awk '{print $1}')
else
  echo "Error: Neither sha256sum nor shasum is available for checksum verification." >&2
  exit 1
fi

if [ "$ACTUAL_HASH" != "$EXPECTED_HASH" ]; then
  echo "Error: Checksum verification failed for $MATCH_NAME" >&2
  echo "  Expected: $EXPECTED_HASH" >&2
  echo "  Actual:   $ACTUAL_HASH" >&2
  exit 1
fi

# 9. Extract archive
EXTRACT_DIR="$TMPDIR/extracted"
mkdir -p "$EXTRACT_DIR"
if ! tar -xzf "$ARCHIVE_PATH" -C "$EXTRACT_DIR"; then
  echo "Error: Failed to extract release archive." >&2
  exit 1
fi

SRC_BIN="$(find "$EXTRACT_DIR" -type f -name "zotero-cli" | head -n 1 || true)"
if [ -z "$SRC_BIN" ] || [ ! -f "$SRC_BIN" ]; then
  echo "Error: Could not locate zotero-cli executable inside the extracted archive." >&2
  exit 1
fi

# 10. Install binary to target directory atomically
mkdir -p "$INSTALL_DIR"
DEST_BIN="$INSTALL_DIR/zotero-cli"
TMP_DEST="$INSTALL_DIR/.zotero-cli.tmp.$$"

cp "$SRC_BIN" "$TMP_DEST"
chmod 755 "$TMP_DEST"
mv -f "$TMP_DEST" "$DEST_BIN"

# Also install cli-anything-zotero alias if present in archive
SRC_ALIAS="$(find "$EXTRACT_DIR" -type f -name "cli-anything-zotero" | head -n 1 || true)"
if [ -n "$SRC_ALIAS" ] && [ -f "$SRC_ALIAS" ]; then
  TMP_ALIAS="$INSTALL_DIR/.cli-anything-zotero.tmp.$$"
  cp "$SRC_ALIAS" "$TMP_ALIAS"
  chmod 755 "$TMP_ALIAS"
  mv -f "$TMP_ALIAS" "$INSTALL_DIR/cli-anything-zotero"
fi

# 11. Verify execution
if ! INSTALLED_VERSION="$("$DEST_BIN" --version 2>/dev/null)"; then
  echo "Error: Installed binary failed to execute at $DEST_BIN." >&2
  exit 1
fi

# 12. Check PATH and report
case ":$PATH:" in
  *":$INSTALL_DIR:"*) IN_PATH=1 ;;
  *) IN_PATH=0 ;;
esac

echo "$INSTALLED_VERSION installed successfully"
echo "Path: $DEST_BIN"
echo ""
if [ "$IN_PATH" -eq 0 ]; then
  echo "Add this directory to PATH:"
  echo "  export PATH=\"$INSTALL_DIR:\$PATH\""
  echo ""
fi
echo "Next:"
echo "  zotero-cli --json app doctor"
