//! RAW preview extraction via the `rawler` crate (LGPL-2.1).
//!
//! Why this exists: our pipeline only needs a downscaled thumbnail per photo,
//! and every modern camera embeds a JPEG preview inside its RAW file. The
//! catch is *where* — Nikon hides the full-res preview in a NEF-specific
//! MakerNote IFD, Sony in a SubIFD chain, Canon CR3 in an ISO BMFF box, etc.
//! Our old EXIF-walk-then-byte-scan fallback worked for some and silently
//! dropped others (the user's batch of `DSC_29xx.NEF` files all failed with
//! "no embedded JPEG preview found"). `rawler` knows the per-vendor layouts,
//! so a single `preview_image` call gets the largest available preview from
//! pretty much any consumer RAW.
//!
//! We deliberately stick to the embedded preview path — full RAW demosaicing
//! is ~10–100× slower and unnecessary for culling. If a RAW genuinely has no
//! preview at all, the caller can fall back further.

use crate::error::{Error, Result};
use image::DynamicImage;
use rawler::decoders::{Decoder, RawDecodeParams};
use rawler::rawsource::RawSource;
use std::path::Path;

/// Extract the largest embedded preview (or, failing that, the smaller
/// embedded thumbnail) from a RAW file.
///
/// Returns the rendered `DynamicImage` at the preview's native resolution —
/// the caller is expected to downscale to its target [`ThumbnailSpec`].
/// Errors only when there is no embedded preview AND no thumbnail; in that
/// case the caller can choose to fall back to a slower path (full demosaic
/// via `rawler::Decoder::full_image`) or skip the file.
pub fn extract_embedded_preview(path: &Path) -> Result<DynamicImage> {
    let source = RawSource::new(path).map_err(|e| Error::Config(format!(
        "rawler open {}: {e}",
        path.display()
    )))?;
    let decoder = rawler::get_decoder(&source).map_err(|e| Error::Config(format!(
        "rawler decoder {}: {e}",
        path.display()
    )))?;

    // preview_image is usually the largest (full-res or close); thumbnail_image
    // is the tiny ~160px one. Try preview first, fall back to thumbnail. The
    // `RawDecodeParams::default()` selects image_index=0 — the primary image
    // (multi-image RAFs / dual-pixel etc. would expose more, not relevant here).
    let params = RawDecodeParams::default();
    if let Ok(Some(img)) = decoder.preview_image(&source, &params) {
        return Ok(img);
    }
    if let Ok(Some(img)) = decoder.thumbnail_image(&source, &params) {
        return Ok(img);
    }
    Err(Error::Config(format!(
        "{}: rawler found no embedded preview/thumbnail",
        path.display()
    )))
}
