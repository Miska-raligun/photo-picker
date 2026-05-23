#!/usr/bin/env python3
"""Generate synthetic JPEG fixtures with EXIF DateTimeOriginal for smoke-testing.

Produces three "bursts" (visually similar within each burst, distinct between)
plus a few isolated frames, with realistic timestamps.

Run: python3 scripts/make_fixtures.py tests/fixtures/sample
"""
import datetime as dt
import os
import sys
import struct
from pathlib import Path

from PIL import Image, ImageDraw, TiffImagePlugin
from PIL.ExifTags import Base as ExifBase


def draw_scene(w: int, h: int, scene: str, jitter: int = 0) -> Image.Image:
    """Render a scene; `jitter` shifts elements slightly so phash is similar but not identical."""
    img = Image.new("RGB", (w, h), color=(20, 30, 40))
    d = ImageDraw.Draw(img)
    if scene == "park":
        d.rectangle((0, h * 2 // 3, w, h), fill=(40, 110, 50))           # grass
        d.ellipse((w * 0.4 + jitter, h * 0.2, w * 0.55 + jitter, h * 0.35), fill=(250, 220, 100))  # sun
        d.rectangle((w * 0.1 + jitter, h * 0.4, w * 0.3 + jitter, h * 0.7), fill=(80, 50, 30))     # tree trunk
        d.ellipse((w * 0.05 + jitter, h * 0.25, w * 0.4 + jitter, h * 0.55), fill=(30, 100, 40))   # foliage
    elif scene == "street":
        d.rectangle((0, h // 2, w, h), fill=(60, 60, 65))                # road
        d.rectangle((w * 0.1, h * 0.3, w * 0.4, h * 0.7), fill=(150, 100, 80))  # building 1
        d.rectangle((w * 0.45, h * 0.2, w * 0.7, h * 0.7), fill=(120, 130, 140))
        d.ellipse((w * 0.5 + jitter, h * 0.55, w * 0.55 + jitter, h * 0.65), fill=(255, 240, 200))
    elif scene == "indoor":
        img = Image.new("RGB", (w, h), color=(180, 160, 140))            # warm interior
        d = ImageDraw.Draw(img)
        d.rectangle((w * 0.2 + jitter, h * 0.3, w * 0.8 + jitter, h * 0.9), fill=(80, 50, 40))     # table
        d.ellipse((w * 0.45, h * 0.4, w * 0.55, h * 0.5), fill=(255, 250, 200))                    # candle
    return img


def make_exif(captured_at: dt.datetime, subsec_ms: int) -> Image.Exif:
    dt_str = captured_at.strftime("%Y:%m:%d %H:%M:%S")
    sub = f"{subsec_ms:03d}"
    exif = Image.Exif()
    exif[ExifBase.Make.value] = "PhotoPick"
    exif[ExifBase.Model.value] = "Fixture"
    exif[ExifBase.DateTime.value] = dt_str
    # ExifIFD sub-block
    exif_ifd = {
        ExifBase.DateTimeOriginal.value: dt_str,
        ExifBase.DateTimeDigitized.value: dt_str,
        ExifBase.SubsecTimeOriginal.value: sub,
        ExifBase.SubsecTimeDigitized.value: sub,
        ExifBase.ISOSpeedRatings.value: 400,
        ExifBase.ExposureTime.value: TiffImagePlugin.IFDRational(1, 500),
    }
    exif.get_ifd(0x8769).update(exif_ifd)
    return exif


def write_jpeg(path: Path, img: Image.Image, captured_at: dt.datetime, subsec_ms: int) -> None:
    exif = make_exif(captured_at, subsec_ms)
    img.save(path, "JPEG", quality=88, exif=exif)


def main():
    out = Path(sys.argv[1] if len(sys.argv) > 1 else "tests/fixtures/sample")
    out.mkdir(parents=True, exist_ok=True)

    W, H = 800, 600

    # Burst 1: park, 10 frames at ~100ms apart
    base = dt.datetime(2026, 5, 23, 10, 0, 0)
    for i in range(10):
        ts = base + dt.timedelta(milliseconds=100 * i)
        img = draw_scene(W, H, "park", jitter=i)
        write_jpeg(out / f"park_burst_{i:02d}.jpg", img, ts.replace(microsecond=0), (i * 100) % 1000)

    # Burst 2: street, 5 frames at ~50ms apart, 30 minutes later
    base = dt.datetime(2026, 5, 23, 10, 30, 0)
    for i in range(5):
        ts = base + dt.timedelta(milliseconds=50 * i)
        img = draw_scene(W, H, "street", jitter=i)
        write_jpeg(out / f"street_burst_{i:02d}.jpg", img, ts.replace(microsecond=0), (i * 50) % 1000)

    # Burst 3: indoor, 7 frames at ~200ms apart, an hour later
    base = dt.datetime(2026, 5, 23, 11, 30, 0)
    for i in range(7):
        ts = base + dt.timedelta(milliseconds=200 * i)
        img = draw_scene(W, H, "indoor", jitter=i)
        write_jpeg(out / f"indoor_burst_{i:02d}.jpg", img, ts.replace(microsecond=0), (i * 200) % 1000)

    # Isolated: 3 separate scenes, minutes apart
    for i, scene in enumerate(["park", "street", "indoor"]):
        ts = dt.datetime(2026, 5, 23, 12, 0, 0) + dt.timedelta(minutes=i * 5)
        img = draw_scene(W, H, scene, jitter=50 * (i + 1))
        write_jpeg(out / f"solo_{scene}_{i}.jpg", img, ts, 0)

    n = len(list(out.glob("*.jpg")))
    print(f"wrote {n} fixtures to {out}")


if __name__ == "__main__":
    main()
