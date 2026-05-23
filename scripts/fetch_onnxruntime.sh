#!/usr/bin/env bash
# Fetch and stage the prebuilt onnxruntime static library that ort-sys expects.
# Run from the repo root: `bash scripts/fetch_onnxruntime.sh`
#
# Avoids the build script's built-in downloader which times out behind some
# proxies. The destination matches ORT_LIB_LOCATION in .cargo/config.toml.

set -euo pipefail

ORT_VERSION="ms@1.24.2"
TARGET="x86_64-unknown-linux-gnu"
URL="https://cdn.pyke.io/0/pyke:ort-rs/${ORT_VERSION}/${TARGET}.tar.lzma2"
DEST_DIR="vendor/onnxruntime"

mkdir -p "${DEST_DIR}"

if [[ -f "${DEST_DIR}/libonnxruntime.a" ]]; then
  echo "${DEST_DIR}/libonnxruntime.a already present — skipping download"
  exit 0
fi

echo "fetching ${URL}"
curl -L --max-time 600 -o /tmp/ort.tar.lzma2 "${URL}"

# pyke uses raw LZMA2 framing (not standard .xz). xz/lzma can't decompress it
# without the dictionary size hint, so we go through Python's lzma module.
python3 - <<'PY'
import lzma
data = open('/tmp/ort.tar.lzma2', 'rb').read()
dec = lzma.LZMADecompressor(format=lzma.FORMAT_RAW,
                            filters=[{'id': lzma.FILTER_LZMA2, 'dict_size': 1 << 26}])
open('/tmp/ort.tar', 'wb').write(dec.decompress(data))
PY

tar -xf /tmp/ort.tar -C "${DEST_DIR}"
rm -f /tmp/ort.tar /tmp/ort.tar.lzma2

ls -la "${DEST_DIR}"
echo "done"
