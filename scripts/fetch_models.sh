#!/usr/bin/env bash
# Download the ONNX models photo-pick needs (CLIP vision encoder + YuNet face
# detector) into a target directory and verify their SHA-256. Used by the
# release workflow to bundle models for offline use, and usable standalone:
#
#   bash scripts/fetch_models.sh ./models
#
# The hashes here MUST match the ModelDescriptor entries in
# crates/core/src/models/{clip.rs,scoring/face_yunet.rs}.
# Portable across Linux (sha256sum), macOS (shasum), and Git-Bash on Windows.

set -euo pipefail

DEST="${1:-models}"
mkdir -p "$DEST"

# name|filename|url|sha256
MODELS=(
  "clip|clip-vit-b32-vision-quantized.onnx|https://huggingface.co/Xenova/clip-vit-base-patch32/resolve/main/onnx/vision_model_quantized.onnx|583fd1110a514667812fee7d684952aaf82a99b959760c8d7dca7e0ab9839299"
  "yunet|face_detection_yunet_2023mar.onnx|https://github.com/opencv/opencv_zoo/raw/main/models/face_detection_yunet/face_detection_yunet_2023mar.onnx|8f2383e4dd3cfbb4553ea8718107fc0423210dc964f9f4280604804ed2552fa4"
)

verify_sha() { # expected, file -> 0 if match
  local expected="$1" file="$2" actual
  if command -v sha256sum >/dev/null 2>&1; then
    actual=$(sha256sum "$file" | awk '{print $1}')
  elif command -v shasum >/dev/null 2>&1; then
    actual=$(shasum -a 256 "$file" | awk '{print $1}')
  else
    echo "warn: no sha256 tool found; skipping verification" >&2
    return 0
  fi
  [ "$actual" = "$expected" ]
}

for entry in "${MODELS[@]}"; do
  IFS='|' read -r name filename url sha <<< "$entry"
  out="$DEST/$filename"
  if [[ -f "$out" ]] && verify_sha "$sha" "$out"; then
    echo "$name: already present and verified — skipping"
    continue
  fi
  echo "$name: downloading $url"
  curl -fL --retry 3 --max-time 900 -o "$out" "$url"
  if ! verify_sha "$sha" "$out"; then
    echo "$name: SHA-256 mismatch — refusing to use $out" >&2
    exit 1
  fi
  echo "$name: OK"
done

echo "models staged in $DEST"
