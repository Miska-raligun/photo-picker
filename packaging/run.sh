#!/usr/bin/env bash
# photo-pick launcher. Starts the local server (UI embedded in the binary,
# models bundled alongside) and opens it in your browser.
#
# Override the bind address with PHOTO_PICK_BIND, e.g.:
#   PHOTO_PICK_BIND=0.0.0.0:7777 ./run.sh
set -euo pipefail

DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BIND="${PHOTO_PICK_BIND:-127.0.0.1:7777}"
URL="http://${BIND}"

# Use the models shipped in this bundle (no network needed on first run).
export PHOTO_PICK_MODELS_DIR="${PHOTO_PICK_MODELS_DIR:-$DIR/models}"

# Open the browser shortly after the server comes up. Best-effort across
# Linux (xdg-open), macOS (open), and WSL (explorer.exe / powershell).
open_browser() {
  sleep 1.2
  if command -v xdg-open >/dev/null 2>&1; then xdg-open "$URL" >/dev/null 2>&1 || true
  elif command -v open >/dev/null 2>&1; then open "$URL" >/dev/null 2>&1 || true
  elif command -v explorer.exe >/dev/null 2>&1; then explorer.exe "$URL" >/dev/null 2>&1 || true
  elif command -v powershell.exe >/dev/null 2>&1; then powershell.exe -NoProfile Start "$URL" >/dev/null 2>&1 || true
  else echo "Open $URL in your browser."; fi
}
open_browser &

echo "Starting photo-pick at $URL  (Ctrl-C to stop)"
exec "$DIR/photo-pick-server"
