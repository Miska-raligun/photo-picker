# photo-pick

Intelligent photo culling for burst shots — local, offline, single binary.

## Run

```bash
./run.sh
```

This starts a local server and opens the UI in your browser
(http://127.0.0.1:7777). Press Ctrl-C in the terminal to stop it.

Everything is self-contained:
- The web UI is embedded in the `photo-pick-server` binary.
- The ONNX models (CLIP vision encoder + YuNet face detector) ship in
  `models/` next to the binary, so the first scan works without internet.

## Notes

- **Nothing is uploaded.** All analysis runs on your machine. The only time
  photo-pick reaches the network is the optional "Ask VLM why" feature, which
  you trigger explicitly and configure with your own API key.
- **Your originals are never modified by a scan.** Scans only analyze. Use the
  per-task **Export** action to copy/link the keepers somewhere, or the
  in-place **Apply** flow to move rejects to the system trash (recoverable).
- Bind to your LAN instead of localhost: `PHOTO_PICK_BIND=0.0.0.0:7777 ./run.sh`
- Point at a different models directory: `PHOTO_PICK_MODELS_DIR=/path ./run.sh`

## Requirements

Linux x86_64 with glibc (most modern distros). No other dependencies.
