# photo-pick

Local two-stage culling for burst-mode photography.

A Rust pipeline that ingests a folder of photos (JPEG or RAW), groups
near-duplicate bursts, scores each photo on a handful of dimensions, and
picks the keepers. Optionally calls a vision-language model (OpenAI,
Anthropic, or any OpenAI-compatible service like SiliconFlow) to explain
why one frame beat another. Ships as one Rust binary serving an embedded
React UI.

Everything runs on your machine. No upload, no cloud, no telemetry —
except the explicit VLM calls you trigger with your own key.

## What it does

- **Stage A — bursts**: groups photos taken close in time whose visual
  content is nearly identical (CLIP cosine ≥ 0.95 by default). Within
  each burst, keeps the top K1 by *technical* score: exposure, white
  balance, sharpness, noise (ISO-aware).
- **Stage B — compositions**: re-groups the Stage A picks by visual
  composition similarity (CLIP cosine ≥ 0.93). Within each composition
  group, keeps the top K2 by a *final* score that blends technical,
  aesthetic, composition, and face-bonus signals with scene-aware
  weights (portrait vs landscape vs mixed, decided automatically by
  face count and area).
- **Two output modes**:
  - default: copies / hardlinks selected files into an output directory
  - **in-place**: leaves the source untouched until you click **Apply**
    in the UI, which sends rejected files to the OS recycle bin
- **VLM explanations** (optional): asks a chosen VLM to rank the photos
  in a group from best to worst with one-sentence reasons; the UI
  overlays per-photo rank badges and inline reasons. Independent of
  the pipeline's verdict — useful for spotting disagreements.

## Status of scoring components

| Component | Status |
|---|---|
| Exposure, white balance, sharpness, noise | Real |
| Stage A burst clustering (CLIP cosine) | Real |
| Stage B composition clustering (CLIP cosine) | Real |
| Face detection (YuNet, opencv_zoo) | Real (bbox + 5 keypoints; eye-open / smile pending) |
| Aesthetic score | Heuristic (luma range + hue diversity + saturation). Real CLIP-IQA pending. |
| Composition score | Heuristic (Laplacian saliency + rule-of-thirds + size + edge clipping). |
| VLM explain | Real (OpenAI-compatible + Anthropic Messages API) |

## Tech stack

- **Core**: Rust 1.95, ort 2.0 (ONNX Runtime), CLIP ViT-B/32, YuNet
- **Server**: axum 0.7, tokio, rust-embed
- **UI**: Vite 8 + React 19 + TypeScript + Tailwind v4 + shadcn/ui +
  sonner + lucide-react
- **Cache**: SQLite (rusqlite, bundled)
- **Trash**: `trash` crate (cross-platform OS recycle bin)
- **RAW**: hand-rolled embedded-JPEG extractor (Apache 2.0 / MIT
  dependency tree only — no LGPL rawler/rawloader)

## Supported formats

- **JPEG** — full
- **RAW** — TIFF-container formats with embedded JPEG preview:
  CR2, NEF, ARW, DNG, PEF, ORF. CR3 / RAF / HEIC not yet supported.

## Build

### Prerequisites

- Rust stable (currently tested on 1.95)
- Node.js 22+, pnpm 10+
- Python 3 (for the one-shot ONNX Runtime decompression, see below)

### One-time setup

```bash
# 1) Fetch the prebuilt onnxruntime static lib (~85MB).
#    Builds via pyke's CDN; if your network blocks that,
#    scripts/fetch_onnxruntime.sh has a curl fallback.
bash scripts/fetch_onnxruntime.sh

# 2) Install + build the React UI
cd web
pnpm install
pnpm build
cd ..

# 3) Build the Rust server
cargo build --release --bin photo-pick-server
```

The first scan downloads the CLIP and YuNet ONNX models to
`~/.cache/photo-pick/models/` (~85MB + ~230KB, SHA-256 pinned).

## Run

```bash
./target/release/photo-pick-server
# → http://127.0.0.1:7777
```

Optional environment variables:

| Var | Purpose |
|---|---|
| `PHOTO_PICK_BIND` | bind address, default `127.0.0.1:7777` |
| `OPENAI_API_KEY` | key for the OpenAI-compatible VLM provider |
| `OPENAI_BASE_URL` | full chat-completions endpoint, default OpenAI |
| `OPENAI_MODEL` | model id, default `gpt-4o` |
| `ANTHROPIC_API_KEY` | key for Anthropic Messages API |
| `ANTHROPIC_MODEL` | default `claude-opus-4-7` |
| `RUST_LOG` | tracing filter, e.g. `info,photo_pick_core=debug` |

You can also configure the VLM per-browser via the in-app Settings dialog
(gear icon top-right). Configuration there is saved to localStorage and
overrides the server's env vars on a per-request basis.

A CLI binary is also shipped for headless use:

```bash
./target/release/photo-pick scan <source_dir> -o <output_dir> --k1 3 --k2 1
./target/release/photo-pick scan --help
```

## WSL note

On WSL2, the server's filesystem browser defaults to `/mnt` so Windows
drive letters appear at the top level. External / removable drives may
need explicit mounting:

```bash
sudo mount -t drvfs D: /mnt/d
```

## Project layout

```
crates/
  core/      pipeline, scoring, models, vlm, cache
  cli/       photo-pick (CLI)
  server/    photo-pick-server (axum) + embedded React build
web/         Vite + React + TS (UI source)
scripts/    fetch_onnxruntime.sh, make_fixtures.py
tests/      fixtures (synthetic JPEGs, generated)
```

## Privacy

API keys you enter in the Settings dialog live in your browser's
localStorage. Anyone with access to that browser profile can read them.
Don't enable custom-key mode on shared machines.

## License

MIT OR Apache-2.0 (workspace default).
