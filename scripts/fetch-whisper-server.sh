#!/usr/bin/env bash
# Builds whisper.cpp's whisper-server into src-tauri/binaries/whisper/, where
# the app expects to find it.
#
# Unlike scripts/fetch-llama-server.sh this compiles from source, because
# whisper.cpp publishes no macOS binaries — its releases carry Ubuntu, Windows
# and an xcframework, and nothing for a Mac command line. Expect a couple of
# minutes on the first run.
#
# TODO(packaging): same unsolved problem as llama-server. A packaged .app
# resolves the binary from resource_dir(), but tauri.conf.json bundles nothing,
# so a distributed build cannot find either sidecar. Fixing that means bundling
# both as resources (or externalBin), building them statically enough to
# survive code signing, and notarizing. One task, both sidecars.
set -euo pipefail

TAG="v1.9.2"
cd "$(dirname "$0")/.."
DEST="src-tauri/binaries/whisper"

if [ "$(uname -s)" != "Darwin" ]; then
  echo "this script only handles macOS; whisper.cpp ships Linux/Windows binaries" >&2
  exit 1
fi

for tool in cmake git; do
  command -v "$tool" >/dev/null 2>&1 || {
    echo "$tool is required to build whisper.cpp (brew install $tool)" >&2
    exit 1
  }
done

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

echo "Cloning whisper.cpp ${TAG}"
git clone --depth 1 --branch "$TAG" https://github.com/ggml-org/whisper.cpp "$TMP/whisper.cpp"

# Metal is what makes transcription fast enough to sit between speaking and a
# reply; without it this runs on the CPU and the wait becomes noticeable.
cmake -S "$TMP/whisper.cpp" -B "$TMP/build" \
  -DCMAKE_BUILD_TYPE=Release \
  -DGGML_METAL=ON \
  -DWHISPER_BUILD_EXAMPLES=ON \
  -DWHISPER_BUILD_TESTS=OFF \
  -DBUILD_SHARED_LIBS=OFF > "$TMP/cmake.log" 2>&1 || {
    echo "cmake configure failed; see below" >&2
    tail -20 "$TMP/cmake.log" >&2
    exit 1
  }

echo "Building (this takes a few minutes)"
cmake --build "$TMP/build" --config Release --target whisper-server -j "$(sysctl -n hw.ncpu)" \
  > "$TMP/build.log" 2>&1 || {
    echo "build failed; see below" >&2
    tail -30 "$TMP/build.log" >&2
    exit 1
  }

BIN="$(find "$TMP/build" -name whisper-server -type f -perm -u+x | head -1)"
if [ -z "$BIN" ]; then
  echo "whisper-server not found after a successful build" >&2
  exit 1
fi

rm -rf "$DEST"
mkdir -p "$DEST"
cp "$BIN" "$DEST/"
# Static libs are linked in, but Metal needs its shader source at runtime
# unless it was embedded; copy it if the build produced one.
find "$TMP/build" -name "*.metal" -o -name "*.metallib" | while read -r shader; do
  cp "$shader" "$DEST/" 2>/dev/null || true
done
chmod +x "$DEST/whisper-server"

echo "whisper-server -> $DEST/"
echo "The model is downloaded by the app on first use — see Settings → Models."
