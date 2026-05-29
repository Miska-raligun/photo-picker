#!/usr/bin/env bash
# Download the ONNX models photo-pick needs (CLIP vision encoder + YuNet face
# detector) into a target directory and verify their SHA-256. Used by the
# release workflow to bundle models for offline use, and usable standalone:
#
#   bash scripts/fetch_models.sh ./models
#
# The hashes here MUST match the ModelDescriptor entries in
# crates/core/src/models/{clip.rs,scoring/face_yunet.rs}.

set -euo pipefail

DEST="${1:-models}"
mkdir -p "$DEST"

# name|filename|url|sha256
MODELS=(
  "clip|clip-vit-b32-vision-quantized.onnx|https://huggingface.co/Xenova/clip-vit-base-patch32/resolve/main/onnx/vision_model_quantized.onnx|583fd1110a514667812fee7d684952aaf82a99b959760c8d7dca7e0ab9839299"
  "yunet|face_detection_yunet_2023mar.onnx|https://github.com/opencv/opencv_zoo/raw/main/models/face_detection_yunet/face_detection_yunet_2023mar.onnx|8f2383e4dd3cfbb4553ea8718107fc0423210dc964f9f4280604804ed2552fa4"
)

for entry in "${MODELS[@]}"; do
  IFS='|' read -r name filename url sha <<< "$entry"
  out="$DEST/$filename"
  if [[ -f "$out" ]] && echo "$sha  $out" | sha256sum -c --status; then
    echo "$name: already present and verified — skipping"
    continue
  fi
  echo "$name: downloading $url"
  curl -fL --retry 3 --max-time 900 -o "$out" "$url"
  echo "$sha  $out" | sha256sum -c -
  echo "$name: OK"
done

echo "models staged in $DEST"
