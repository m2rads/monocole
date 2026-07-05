#!/usr/bin/env bash
# Fetches a pinned llama.cpp release build of llama-server (plus its dylibs)
# into src-tauri/binaries/llama/, where the app expects to find it.
#
# TODO(packaging): for production we'll likely want a static build (or
# externalBin + bundled dylibs with fixed rpaths) so the sidecar survives
# code signing/notarization. This dynamic release build is fine for dev.
set -euo pipefail

TAG="b9873"
cd "$(dirname "$0")/.."
DEST="src-tauri/binaries/llama"

case "$(uname -sm)" in
  "Darwin arm64") ASSET="llama-${TAG}-bin-macos-arm64.tar.gz" ;;
  "Darwin x86_64") ASSET="llama-${TAG}-bin-macos-x64.tar.gz" ;;
  *) echo "unsupported platform: $(uname -sm)" >&2; exit 1 ;;
esac

URL="https://github.com/ggml-org/llama.cpp/releases/download/${TAG}/${ASSET}"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

echo "Downloading ${URL}"
curl -fL --progress-bar "$URL" -o "$TMP/llama.tar.gz"
tar -xzf "$TMP/llama.tar.gz" -C "$TMP"

# Release tarballs unpack to build/bin (binaries + dylibs side by side,
# linked via @rpath/@loader_path, so they must stay in one directory).
BIN_DIR="$(dirname "$(find "$TMP" -name llama-server -type f | head -1)")"
if [ -z "$BIN_DIR" ]; then
  echo "llama-server not found in release archive" >&2
  exit 1
fi

rm -rf "$DEST"
mkdir -p "$DEST"
cp "$BIN_DIR/llama-server" "$DEST/"
find "$BIN_DIR" -name "*.dylib" -exec cp {} "$DEST/" \;
chmod +x "$DEST/llama-server"

echo "Installed to $DEST:"
ls -lh "$DEST"
"$DEST/llama-server" --version
