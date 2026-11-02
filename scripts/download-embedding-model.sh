#!/bin/bash
# Download the Jina Embeddings v2 Base Code model (INT8-quantized ONNX) for Devscriptor.
#
# Devscriptor uses the dynamically quantized ONNX export
# (`onnx/model_quantized.onnx`, ~160 MB, 768-dim), and runs its matrix products
# in f32 over the weights dequantized at load (~1.3 GB resident).
#
# Devscriptor downloads these files itself the first time the model is needed.
# This script fetches the same files, at the same pinned commit, checked against
# the same SHA-256 hashes: to provision the cache ahead of time (a machine
# without network access, a build without the `model-download` feature), or to
# stage the mirror's copy. The layout is the cache's:
#
#   <target>/
#   ├── onnx/model_quantized.onnx
#   ├── tokenizer.json
#   ├── config.json
#   ├── tokenizer_config.json
#   └── special_tokens_map.json
#
# The model mirror workflow (.github/workflows/model-mirror.yml) stages them
# with this script and publishes them side by side, as the assets of a GitHub
# release (model_quantized.onnx beside tokenizer.json).

set -euo pipefail

MODEL_NAME="jinaai/jina-embeddings-v2-base-code"
# The commit and the checksums Devscriptor pins for these files
# (crates/core/src/embeddings/model_fetch.rs: PINNED_HF_REVISION and
# REQUIRED_FILE_SHA256), which it checks every download against, from Hugging
# Face or the mirror: staged from `main`, a later push would make the mirror's
# copy one Devscriptor refuses.
MODEL_REVISION="516f4baf13dec4ddddda8631e019b5737c8bc250"

# Target directory. Defaults to the cache Devscriptor loads from — the user cache
# directory: ~/Library/Caches on macOS, else $XDG_CACHE_HOME or ~/.cache; set
# DEVSCRIPTOR_MODEL_DIR to stage the files elsewhere, such as for the mirror.
if [[ "$(uname -s)" == "Darwin" ]]; then
    CACHE_HOME="$HOME/Library/Caches"
else
    CACHE_HOME="${XDG_CACHE_HOME:-$HOME/.cache}"
fi
TARGET_DIR="${DEVSCRIPTOR_MODEL_DIR:-$CACHE_HOME/devscriptor/models/jina-embeddings-v2-base-code}"

BASE_URL="https://huggingface.co/$MODEL_NAME/resolve/$MODEL_REVISION"

echo "==============================================="
echo "Downloading Jina Embeddings v2 Base Code (INT8-quantized ONNX)"
echo "==============================================="
echo "Model:  $MODEL_NAME (onnx/model_quantized.onnx)"
echo "Target: $TARGET_DIR"
echo ""

mkdir -p "$TARGET_DIR/onnx"

# sha256_of <file>: the file's SHA-256, by sha256sum or shasum.
sha256_of() {
    { sha256sum "$1" 2>/dev/null || shasum -a 256 "$1" 2>/dev/null; } | awk '{print $1}'
}

# download_file <url> <output> <desc> [<sha256>]: download to `<output>.part`,
# check it against <sha256> where one is pinned, and only then move it into
# place. Devscriptor takes a file that is there as present, so a download cut
# short, or of other bytes, must not be left as one.
download_file() {
    local url="$1" output="$2" desc="$3" expected="${4:-}" actual
    echo "Downloading $desc..."
    if command -v curl >/dev/null 2>&1; then
        curl -L --fail --progress-bar "$url" -o "$output.part" || {
            rm -f "$output.part"
            echo "ERROR: Failed to download $desc from $url" >&2
            return 1
        }
    elif command -v wget >/dev/null 2>&1; then
        wget --progress=bar:force --show-progress "$url" -O "$output.part" 2>&1 || {
            rm -f "$output.part"
            echo "ERROR: Failed to download $desc from $url" >&2
            return 1
        }
    else
        echo "ERROR: Neither curl nor wget is available. Please install one." >&2
        exit 1
    fi
    if [ -n "$expected" ]; then
        actual=$(sha256_of "$output.part")
        if [ "$actual" != "$expected" ]; then
            rm -f "$output.part"
            echo "ERROR: SHA-256 mismatch for $desc (expected $expected, got ${actual:-none})" >&2
            return 1
        fi
    fi
    mv "$output.part" "$output"
    echo "✓ $desc"
}

# The files Devscriptor requires (REQUIRED_FILES in model_fetch.rs).
echo "Downloading INT8-quantized ONNX model (~160MB)..."
download_file "$BASE_URL/onnx/model_quantized.onnx" "$TARGET_DIR/onnx/model_quantized.onnx" "quantized ONNX model" \
    ed45870251c9f0cf656e78aab0d37a23489066df8a222bb1c8caf8a45f2cb16d
download_file "$BASE_URL/tokenizer.json"           "$TARGET_DIR/tokenizer.json"           "tokenizer.json" \
    b01c78a902aa4facb2f47f95449f48e2f7bbfea5d2472ee2f6ce92323c6f86e5
download_file "$BASE_URL/config.json"              "$TARGET_DIR/config.json"              "config.json" \
    e426aa684c7f9a95c5f020aa855faf93a24f065f5fad0c9e17b124670cabdea6
download_file "$BASE_URL/tokenizer_config.json"    "$TARGET_DIR/tokenizer_config.json"    "tokenizer_config.json" \
    f477aeb15ff9f78d3c1ddf2361d2b0b8b20cf55220f839f29a37f3a18efddd89
# Optional, and pinned by no checksum (see OPTIONAL_FILES in model_fetch.rs).
download_file "$BASE_URL/special_tokens_map.json"  "$TARGET_DIR/special_tokens_map.json"  "special_tokens_map.json"

echo ""
echo "==============================================="
echo "Download complete"
echo "==============================================="
find "$TARGET_DIR" -type f -exec ls -lh {} \; 2>/dev/null | awk '{print "  " $9 " (" $5 ")"}'
echo ""
du -sh "$TARGET_DIR" 2>/dev/null || true
echo ""
echo "Devscriptor will now load the model from this directory without contacting"
echo "Hugging Face. To serve the files from a mirror of your own, put them side by"
echo "side in one directory (model_quantized.onnx, out of onnx/, beside"
echo "tokenizer.json) and point DEVSCRIPTOR_MODEL_MIRROR_URL at it."
