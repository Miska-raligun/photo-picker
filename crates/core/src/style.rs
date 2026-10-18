//! Lightroom style presets learned from reference photos.
//!
//! The user picks reference photos with a look they like; we fit a small set
//! of Lightroom / Camera Raw develop settings and write them as a preset
//! `.xmp` to import into LR and apply (and tweak) by hand:
//!
//! - **Master tone curve** (`crs:ToneCurvePV2012`)
//! - **Red / green / blue curves** (`crs:ToneCurvePV2012Red` …) — relative
//!   colour shifts that behave the same on RAW and JPEG, unlike the
//!   white-balance sliders (absolute Kelvin on RAW).
//! - **Saturation** (`crs:Saturation`)
//! - **HSL / Color Mixer** (`crs:HueAdjustmentRed` …) and **Color Grading**
//!   wheels (`crs:SplitToningShadowHue` …) — paired mode only, and only
//!   written when fitted.
//!
//! The preset contains only these settings, so applying it leaves exposure,
//! white balance and everything else untouched.
//!
//! Two fitting modes, because what can be learned depends on what we can see:
//!
//! - **Paired** ([`StylePreset::fit_paired`]): each reference is an edited
//!   version of a photo we also have unedited (the user graded a few shots of
//!   a shoot). Same content on both sides, so the full transform is
//!   identifiable. When the edit keeps the original's framing, pixels are
//!   compared position by position ([`PairedPixels`]): curves are fitted on
//!   near-neutral pixels (which HSL can't touch, so per-colour changes don't
//!   leak into them), then colour grading and per-band HSL on the rest.
//!   Per-tone casts are tried both as RGB curves and as grading wheels, and
//!   the one that fits the coloured pixels better is kept. Cropped edits fall
//!   back to quantile matching of the pooled histograms (curves and
//!   saturation only). On test grades with selective colour changes, held-out
//!   photos land 20–30 % closer to the true edit than with curves alone; HSL
//!   can only learn colours the references contain.
//! - **Unpaired** ([`StylePreset::fit_unpaired`]): references are unrelated
//!   photos. Comparing distributions across *different* scenes mistakes
//!   content for style (a bright reference "brightens" every target, a warm
//!   interior "warms" it), which in testing made results worse than doing
//!   nothing. So only content-independent traits are transferred — faded
//!   blacks / rolled-off whites agreed on by every reference, a damped
//!   contrast change, and the *difference* in colour between shadows,
//!   midtones and highlights (split toning) — and saturation is left alone.
//!   The result is a rough approximation of strong looks, not a match.
//!
//! Either way this is statistics, not a pixel-exact copy: LR renders RAW files
//! from scratch rather than from the camera JPEG these numbers come from, and
//! its curve interpolation differs slightly from [`StylePreset::apply_preview`].
//! LR's HSL and grading maths is unpublished; ours is a model of it, so those
//! sliders land close in spirit but not exactly in strength. The `.cube`
//! export bakes our model itself and matches the preview.

use image::{DynamicImage, RgbImage};
use serde::{Deserialize, Serialize};

const HIST_BINS: usize = 256;
/// Upper luma bounds of the shadow and midtone zones (rest is highlights).
const ZONE_EDGES: [f32; 2] = [0.33, 0.67];
/// A pixel counts as near-neutral for cast measurement below this HSV-style
/// saturation `(max - min) / max`.
const NEUTRAL_MAX_SAT: f32 = 0.3;
/// Pixels sampled per image for statistics (evenly strided).
const SAMPLES_PER_IMAGE: usize = 160_000;
/// Pixels per image kept verbatim for simulating the fitted curves when
/// solving for saturation.
const KEPT_PER_IMAGE: usize = 2_000;
const KEPT_MAX: usize = 60_000;
/// Percentiles matched by the paired curves.
const MATCH_PERCENTILES: [f32; 9] = [0.005, 0.05, 0.15, 0.30, 0.50, 0.70, 0.85, 0.95, 0.995];
/// Slope limits for matched curves, so sparse histograms can't produce a
/// posterizing cliff or a flat plateau.
const SLOPE_RANGE: (f32, f32) = (0.3, 3.0);
/// Largest black lift / white roll-off the unpaired curve will apply.
const MAX_FADE: f32 = 0.2;
/// Floor / ceiling for the expected black / white level of ungraded photos.
const BLACK_PRIOR: f32 = 0.02;
const WHITE_PRIOR: f32 = 0.98;
/// Bounds on the (undamped) unpaired contrast ratio.
const CONTRAST_RANGE: (f32, f32) = (0.7, 1.4);
/// Curve x positions of the shadow / midtone / highlight split-tone points.
const CAST_POINTS: [f32; 3] = [0.18, 0.50, 0.82];
/// Largest per-channel split-tone offset (fraction of full scale).
const MAX_CAST: f32 = 0.08;
/// Minimum share of sampled pixels a zone needs before its cast is trusted.
const MIN_ZONE_SHARE: f64 = 0.002;
const SATURATION_RANGE: i32 = 60;

/// Pooled colour statistics of a set of photos. Every image carries equal
/// weight regardless of its size.
#[derive(Debug, Clone, Default)]
pub struct StyleStats {
    luma_hist: Vec<f64>,
    chan_hist: [Vec<f64>; 3],
    /// Per zone: Σ over images of (Σ per-pixel RGB deviation from gray / samples).
    cast_sum: [[f64; 3]; 3],
    /// Per zone: Σ over images of (neutral pixels in zone / samples).
    cast_share: [f64; 3],
    chroma_sum: f64,
    /// Per image: luma at the 0.5 % and 99.5 % points.
    blacks: Vec<f32>,
    whites: Vec<f32>,
    kept: Vec<[u8; 3]>,
    images: usize,
}

impl StyleStats {
    pub fn new() -> Self {
        let mut s = Self::default();
        s.ensure_hists();
        s
    }

    fn ensure_hists(&mut self) {
        if self.luma_hist.is_empty() {
            self.luma_hist = vec![0.0; HIST_BINS];
            self.chan_hist = [vec![0.0; HIST_BINS], vec![0.0; HIST_BINS], vec![0.0; HIST_BINS]];
        }
    }

    pub fn image_count(&self) -> usize {
        self.images
    }

    /// Add one photo (any size; a ~500–1000 px thumbnail is plenty).
    pub fn add_image(&mut self, img: &DynamicImage) {
        let rgb = img.to_rgb8();
        let n_px = (rgb.width() as usize) * (rgb.height() as usize);
        if n_px == 0 {
            return;
        }
        self.ensure_hists();
        let step = (n_px / SAMPLES_PER_IMAGE).max(1);
        let keep_step = (n_px / KEPT_PER_IMAGE).max(1);
        let raw = rgb.as_raw();

        let mut hist = vec![0.0f64; HIST_BINS];
        let mut chan = [[0u32; HIST_BINS]; 3];
        let mut cast = [[0.0f64; 3]; 3];
        let mut cast_n = [0u32; 3];
        let mut chroma = 0.0f64;
        let mut samples = 0u32;
        let mut i = 0;
        while i < n_px {
            let px = [raw[3 * i], raw[3 * i + 1], raw[3 * i + 2]];
            for c in 0..3 {
                chan[c][px[c] as usize] += 1;
            }
            let [r, g, b] = px.map(|c| c as f32 / 255.0);
            let y = luma(r, g, b);
            hist[((y * 255.0).round() as usize).min(HIST_BINS - 1)] += 1.0;
            let (mx, mn) = (r.max(g).max(b), r.min(g).min(b));
            chroma += (mx - mn) as f64;
            if mx > 0.02 && (mx - mn) / mx < NEUTRAL_MAX_SAT {
                let z = zone(y);
                let gray = (r + g + b) / 3.0;
                for (c, v) in [r, g, b].iter().enumerate() {
                    cast[z][c] += (v - gray) as f64;
                }
                cast_n[z] += 1;
            }
            if i % keep_step == 0 && self.kept.len() < KEPT_MAX {
                self.kept.push(px);
            }
            samples += 1;
            i += step;
        }

        let s = samples as f64;
        self.blacks.push(quantile_of(&hist, 0.005));
        self.whites.push(quantile_of(&hist, 0.995));
        for (acc, h) in self.luma_hist.iter_mut().zip(hist.iter()) {
            *acc += h / s;
        }
        for (dst, src) in self.chan_hist.iter_mut().zip(chan.iter()) {
            for (acc, h) in dst.iter_mut().zip(src.iter()) {
                *acc += *h as f64 / s;
            }
        }
        for ((sum, share), (zone_cast, n)) in
            self.cast_sum.iter_mut().zip(self.cast_share.iter_mut()).zip(cast.iter().zip(cast_n))
        {
            for (acc, v) in sum.iter_mut().zip(zone_cast) {
                *acc += v / s;
            }
            *share += n as f64 / s;
        }
        self.chroma_sum += chroma / s;
        self.images += 1;
    }

    /// Merge statistics gathered on another thread.
    pub fn merge(&mut self, other: StyleStats) {
        if other.images == 0 {
            return;
        }
        self.ensure_hists();
        for (a, b) in self.luma_hist.iter_mut().zip(other.luma_hist.iter()) {
            *a += b;
        }
        for c in 0..3 {
            for (a, b) in self.chan_hist[c].iter_mut().zip(other.chan_hist[c].iter()) {
                *a += b;
            }
        }
        for z in 0..3 {
            for c in 0..3 {
                self.cast_sum[z][c] += other.cast_sum[z][c];
            }
            self.cast_share[z] += other.cast_share[z];
        }
        self.chroma_sum += other.chroma_sum;
        self.blacks.extend(other.blacks);
        self.whites.extend(other.whites);
        let room = KEPT_MAX.saturating_sub(self.kept.len());
        self.kept.extend(other.kept.into_iter().take(room));
        self.images += other.images;
    }

    /// Luma value (0–1) at cumulative share `p` of the pooled histogram.
    fn quantile(&self, p: f32) -> f32 {
        quantile_of(&self.luma_hist, p)
    }

    fn chan_quantile(&self, c: usize, p: f32) -> f32 {
        quantile_of(&self.chan_hist[c], p)
    }

    /// Mean RGB deviation from gray of near-neutral pixels in zone `z`, or
    /// `None` when too few pixels fall there to trust it.
    fn zone_cast(&self, z: usize) -> Option<[f32; 3]> {
        let share = self.cast_share[z];
        (share / self.images.max(1) as f64 >= MIN_ZONE_SHARE)
            .then(|| self.cast_sum[z].map(|v| (v / share) as f32))
    }

    fn mean_chroma(&self) -> f32 {
        (self.chroma_sum / self.images.max(1) as f64) as f32
    }
}

/// Value (0–1) at cumulative share `p` of a 256-bin histogram.
fn quantile_of(hist: &[f64], p: f32) -> f32 {
    let total: f64 = hist.iter().sum();
    if total <= 0.0 {
        return p;
    }
    let target = p as f64 * total;
    let mut cum = 0.0;
    for (i, h) in hist.iter().enumerate() {
        if cum + h >= target && *h > 0.0 {
            let frac = ((target - cum) / h).clamp(0.0, 1.0);
            return ((i as f64 + frac) / hist.len() as f64) as f32;
        }
        cum += h;
    }
    1.0
}

fn luma(r: f32, g: f32, b: f32) -> f32 {
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

fn zone(y: f32) -> usize {
    if y < ZONE_EDGES[0] {
        0
    } else if y < ZONE_EDGES[1] {
        1
    } else {
        2
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum StyleError {
    #[error("no reference photos could be read")]
    NoReferences,
    #[error("no baseline photos to compare against")]
    NoBaseline,
}

/// Long edge of the aligned grids [`PairedPixels`] compares. Small enough that
/// slight misregistration (resampling, a 1–2 % crop) averages out.
const PAIR_GRID: u32 = 96;
/// Largest aspect-ratio mismatch between an edit and its original before the
/// pair is treated as cropped and left out of pixel matching.
const PAIR_MAX_ASPECT_DIFF: f32 = 0.03;
/// Minimum luma correlation between an aligned edit and its original; lower
/// means a crop, rotation or retouch moved content, so pixels don't line up.
const PAIR_MIN_CORRELATION: f32 = 0.8;
/// Pixels below this chroma (`|rgb − mean|`, 0–1 scale) have no reliable hue
/// and are left out of the HSL fit.
const HSL_MIN_CHROMA: f32 = 0.04;
/// LR hue bands, in hue degrees: red, orange, yellow, green, aqua, blue,
/// purple, magenta.
pub const HSL_BANDS: [&str; 8] = ["Red", "Orange", "Yellow", "Green", "Aqua", "Blue", "Purple", "Magenta"];
const HSL_CENTERS: [f32; 8] = [0.0, 30.0, 60.0, 120.0, 180.0, 240.0, 270.0, 300.0];
/// Hue rotation (degrees) of a ±100 HSL hue slider in our model.
const HSL_HUE_SPAN: f32 = 30.0;
/// Brightness change of a ±100 HSL luminance slider on a fully saturated
/// colour: ×(1 ± this).
const HSL_LUM_SPAN: f32 = 0.5;
/// Fitted sliders smaller than this are noise and dropped.
const SLIDER_DEADBAND: i32 = 3;
/// A band needs this share of the chroma-weighted pixel mass to be fitted.
const HSL_MIN_BAND_SHARE: f32 = 0.01;
/// RGB tint (Euclidean, 0–1 scale) of a colour-grading wheel at saturation 100.
const GRADE_MAX_TINT: f32 = 0.15;

/// One HSL / Color Mixer band: LR sliders, −100…100.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HslAdjust {
    pub hue: i32,
    pub saturation: i32,
    pub luminance: i32,
}

/// One colour-grading wheel: hue 0–359°, saturation 0–100.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GradeWheel {
    pub hue: i32,
    pub saturation: i32,
}

/// LR Color Grading shadows / midtones / highlights wheels.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ColorGrade {
    pub shadows: GradeWheel,
    pub midtones: GradeWheel,
    pub highlights: GradeWheel,
}

impl ColorGrade {
    fn wheels(&self) -> [GradeWheel; 3] {
        [self.shadows, self.midtones, self.highlights]
    }

    pub fn is_neutral(&self) -> bool {
        self.wheels().iter().all(|w| w.saturation == 0)
    }
}

/// Pixel correspondences between edited references and their originals,
/// for the selective-colour parts of a paired fit (HSL, colour grading) that
/// pooled histograms can't see.
#[derive(Debug, Clone, Default)]
pub struct PairedPixels {
    /// `(original, edited)`, 0–1 sRGB-encoded.
    pairs: Vec<([f32; 3], [f32; 3])>,
    images: usize,
}

impl PairedPixels {
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of photo pairs accepted so far.
    pub fn pair_count(&self) -> usize {
        self.images
    }

    /// Add an edit and its original. Returns `false` (and adds nothing) when
    /// the frames don't line up — cropped, rotated or retouched — since then
    /// pixels at the same position aren't the same thing.
    pub fn add_pair(&mut self, edited: &DynamicImage, original: &DynamicImage) -> bool {
        let aspect = |i: &DynamicImage| i.width() as f32 / i.height().max(1) as f32;
        if edited.width() == 0 || original.width() == 0 {
            return false;
        }
        if (aspect(edited) / aspect(original) - 1.0).abs() > PAIR_MAX_ASPECT_DIFF {
            return false;
        }
        let (w, h) = if original.width() >= original.height() {
            (PAIR_GRID, ((PAIR_GRID as f32 / aspect(original)).round() as u32).max(1))
        } else {
            (((PAIR_GRID as f32 * aspect(original)).round() as u32).max(1), PAIR_GRID)
        };
        let grid = |i: &DynamicImage| i.resize_exact(w, h, image::imageops::FilterType::Triangle).to_rgb8();
        let (e, o) = (grid(edited), grid(original));
        let to_f = |p: &image::Rgb<u8>| p.0.map(|c| c as f32 / 255.0);
        let pairs: Vec<([f32; 3], [f32; 3])> = o.pixels().zip(e.pixels()).map(|(a, b)| (to_f(a), to_f(b))).collect();

        let ys: Vec<(f32, f32)> =
            pairs.iter().map(|(a, b)| (luma(a[0], a[1], a[2]), luma(b[0], b[1], b[2]))).collect();
        if correlation(&ys) < PAIR_MIN_CORRELATION {
            return false;
        }
        self.pairs.extend(pairs);
        self.images += 1;
        true
    }
}

/// Pearson correlation of `(x, y)` samples; 0 when either side is flat.
fn correlation(v: &[(f32, f32)]) -> f32 {
    let n = v.len() as f64;
    if n < 2.0 {
        return 0.0;
    }
    let (mx, my) = v.iter().fold((0.0, 0.0), |(a, b), (x, y)| (a + *x as f64, b + *y as f64));
    let (mx, my) = (mx / n, my / n);
    let (mut sxy, mut sxx, mut syy) = (0.0, 0.0, 0.0);
    for (x, y) in v {
        let (dx, dy) = (*x as f64 - mx, *y as f64 - my);
        sxy += dx * dy;
        sxx += dx * dx;
        syy += dy * dy;
    }
    if sxx < 1e-9 || syy < 1e-9 {
        return 0.0;
    }
    (sxy / (sxx * syy).sqrt()) as f32
}

/// Fitted develop settings. Curves are LR point curves: `(input, output)` on
/// 0–255, ascending input, always including both endpoints.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StylePreset {
    pub name: String,
    pub tone_curve: Vec<(u8, u8)>,
    pub red_curve: Vec<(u8, u8)>,
    pub green_curve: Vec<(u8, u8)>,
    pub blue_curve: Vec<(u8, u8)>,
    /// LR Saturation slider, −100…100.
    pub saturation: i32,
    /// HSL / Color Mixer, one entry per [`HSL_BANDS`] band.
    #[serde(default)]
    pub hsl: [HslAdjust; 8],
    #[serde(default)]
    pub color_grade: ColorGrade,
}

impl StylePreset {
    /// Paired fit: `edited[i]` is an edited version of `originals[i]` (pooled
    /// into the two stats). `pixels` holds the aligned pairs among them; when
    /// it has any, per-hue (HSL) and per-tone (colour grading) changes the
    /// curves can't express are fitted on top. `strength` in `[0, 1]` scales
    /// every adjustment.
    pub fn fit_paired(
        name: &str,
        edited: &StyleStats,
        originals: &StyleStats,
        pixels: &PairedPixels,
        strength: f32,
    ) -> Result<Self, StyleError> {
        check_inputs(edited, originals)?;
        let s = strength.clamp(0.0, 1.0);
        // With aligned pixels the curves are fitted on near-neutral ones,
        // which HSL can't touch — so per-colour changes don't leak into the
        // curves (quantile matching bakes, say, "warmer skin" into the red
        // curve, which then warms everything red-ish in other photos).
        //
        // Per-tone colour casts fit two ways that agree on neutrals but not
        // on colours: RGB curves (per channel — film-style crossovers) or
        // colour-grading wheels (by brightness — split toning). The coloured
        // pixels tell them apart, so both are fitted and the closer kept.
        let Some(casts) = [CastModel::Curves, CastModel::Wheels]
            .into_iter()
            .filter_map(|m| {
                let full = pixel_curves(name, &pixels.pairs, 1.0, m)?;
                let fitted = with_selective(full, &pixels.pairs);
                Some((pixel_error(&fitted, &pixels.pairs), m, fitted))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))
        else {
            return Ok(Self::paired_curves(name, edited, originals, s));
        };
        let (_, model, full) = casts;
        // Curves refitted at the requested strength; HSL and grading were fitted
        // against the full-strength curves (at lower strength the curves leave
        // part of the tone change undone on purpose, which HSL mustn't make
        // up) and are scaled like the rest.
        let mut preset = pixel_curves(name, &pixels.pairs, s, model).unwrap_or_else(|| full.clone());
        preset.hsl = full.hsl.map(|b| HslAdjust {
            hue: scale_slider(b.hue, s),
            saturation: scale_slider(b.saturation, s),
            luminance: scale_slider(b.luminance, s),
        });
        let wheel = |w: GradeWheel| {
            let saturation = scale_slider(w.saturation, s);
            if saturation == 0 { GradeWheel::default() } else { GradeWheel { hue: w.hue, saturation } }
        };
        preset.color_grade = ColorGrade {
            shadows: wheel(full.color_grade.shadows),
            midtones: wheel(full.color_grade.midtones),
            highlights: wheel(full.color_grade.highlights),
        };
        Ok(preset)
    }

    /// Tone curve, RGB curves and saturation of a paired fit.
    fn paired_curves(name: &str, edited: &StyleStats, originals: &StyleStats, s: f32) -> Self {
        let luma_pairs: Vec<(f32, f32)> =
            MATCH_PERCENTILES.iter().map(|p| (originals.quantile(*p), edited.quantile(*p))).collect();
        let tone_curve = curve_from_pairs(&luma_pairs, s);

        // Channel curves map the master curve's output onto the edited
        // channel distributions (the master is monotone, so the quantile of
        // master(channel) is master(quantile of channel)).
        let master = curve_lut(&tone_curve);
        let channel = |c: usize| {
            let pairs: Vec<(f32, f32)> = MATCH_PERCENTILES
                .iter()
                .map(|p| {
                    let x = master[(originals.chan_quantile(c, *p) * 255.0).round() as usize];
                    (x as f32 / 255.0, edited.chan_quantile(c, *p))
                })
                .collect();
            curve_from_pairs(&pairs, s)
        };

        let mut preset = StylePreset {
            name: name.to_string(),
            tone_curve: tone_curve.clone(),
            red_curve: channel(0),
            green_curve: channel(1),
            blue_curve: channel(2),
            saturation: 0,
            hsl: Default::default(),
            color_grade: ColorGrade::default(),
        };
        preset.saturation = residual_saturation(&preset, edited, originals, s);
        preset
    }

    /// Unpaired fit from unrelated reference photos; see the module docs for
    /// what is (and deliberately isn't) transferred.
    pub fn fit_unpaired(
        name: &str,
        reference: &StyleStats,
        baseline: &StyleStats,
        strength: f32,
    ) -> Result<Self, StyleError> {
        check_inputs(reference, baseline)?;
        let s = strength.clamp(0.0, 1.0);
        let tone_curve = fade_contrast_curve(reference, baseline, s);

        // Split toning: each zone's cast minus the references' overall cast.
        // The overall part is mostly the reference *scenes* (warm interior,
        // blue hour) rather than the grade, and in testing transferring it
        // made results worse; the shadow/highlight difference is the grade.
        let mut casts = [[0.0f32; 3]; 3];
        let mut overall = [0.0f32; 3];
        let mut weight = 0.0f32;
        for (z, row) in casts.iter_mut().enumerate() {
            if let Some(c) = reference.zone_cast(z) {
                *row = c;
                let w = reference.cast_share[z] as f32;
                weight += w;
                for ch in 0..3 {
                    overall[ch] += c[ch] * w;
                }
            }
        }
        let mut offsets = [[0.0f32; 3]; 3];
        if weight > 0.0 {
            for z in 0..3 {
                if reference.zone_cast(z).is_some() {
                    for ch in 0..3 {
                        offsets[z][ch] =
                            ((casts[z][ch] - overall[ch] / weight) * s).clamp(-MAX_CAST, MAX_CAST);
                    }
                }
            }
        }
        let channel = |c: usize| {
            let mut pts = vec![(0.0, 0.0)];
            for (z, x) in CAST_POINTS.iter().enumerate() {
                pts.push((*x, (x + offsets[z][c]).clamp(0.0, 1.0)));
            }
            pts.push((1.0, 1.0));
            quantize_curve(&pts)
        };

        Ok(StylePreset {
            name: name.to_string(),
            tone_curve,
            red_curve: channel(0),
            green_curve: channel(1),
            blue_curve: channel(2),
            // Chroma differs between scenes far more than between grades;
            // transferring it hurt in every unpaired test. Per-hue changes
            // (HSL) are even more content-bound, so they're paired-only.
            saturation: 0,
            hsl: Default::default(),
            color_grade: ColorGrade::default(),
        })
    }

    /// Approximate what LR will do: master curve, channel curves, saturation
    /// around luma, HSL, then colour grading. For on-screen before/after
    /// previews only — LR's own HSL and grading math is unpublished, so those
    /// two stages are a model of it rather than a copy.
    pub fn apply_preview(&self, img: &DynamicImage) -> RgbImage {
        let (master, chans) = self.float_luts();
        let mut out = img.to_rgb8();
        for p in out.pixels_mut() {
            let rgb = self.apply_rgb_with(&master, &chans, p.0.map(|c| c as f32 / 255.0));
            p.0 = rgb.map(|c| (c * 255.0).round().clamp(0.0, 255.0) as u8);
        }
        out
    }

    /// The same transform as [`Self::apply_preview`] on one colour, in float
    /// (0–1, sRGB-encoded) without 8-bit rounding between stages. Used to bake
    /// LUTs, where per-stage quantisation would show up as banding.
    pub fn apply_rgb(&self, rgb: [f32; 3]) -> [f32; 3] {
        let (master, chans) = self.float_luts();
        self.apply_rgb_with(&master, &chans, rgb)
    }

    fn float_luts(&self) -> ([f32; 256], [[f32; 256]; 3]) {
        (
            curve_lut_f32(&self.tone_curve),
            [curve_lut_f32(&self.red_curve), curve_lut_f32(&self.green_curve), curve_lut_f32(&self.blue_curve)],
        )
    }

    fn apply_rgb_with(&self, master: &[f32; 256], chans: &[[f32; 256]; 3], rgb: [f32; 3]) -> [f32; 3] {
        let k = 1.0 + self.saturation as f32 / 100.0;
        let out = [0, 1, 2].map(|c| {
            let m = sample_lut(master, rgb[c] * 255.0);
            sample_lut(&chans[c], m) / 255.0
        });
        let y = luma(out[0], out[1], out[2]);
        self.apply_selective(out.map(|c| (y + (c - y) * k).clamp(0.0, 1.0)))
    }

    pub fn has_hsl(&self) -> bool {
        self.hsl.iter().any(|b| *b != HslAdjust::default())
    }

    /// HSL then colour grading, on a 0–1 colour.
    fn apply_selective(&self, rgb: [f32; 3]) -> [f32; 3] {
        let mut rgb = rgb;
        if self.has_hsl() {
            rgb = apply_hsl(&self.hsl, rgb);
        }
        if !self.color_grade.is_neutral() {
            let y = luma(rgb[0], rgb[1], rgb[2]).clamp(0.0, 1.0);
            let w = zone_weights(y);
            for (wz, wheel) in w.iter().zip(self.color_grade.wheels()) {
                let t = wheel_tint(wheel);
                for c in 0..3 {
                    rgb[c] += wz * t[c];
                }
            }
        }
        rgb.map(|c| c.clamp(0.0, 1.0))
    }

    /// Render the preset in one export format.
    pub fn render(&self, format: StyleFormat) -> StyleFile {
        let content = match format {
            StyleFormat::LrPreset => self.to_lr_preset_xmp(),
            StyleFormat::CubeLut => self.to_cube_lut(CUBE_SIZE),
        };
        StyleFile { format, file_name: format!("{}.{}", self.file_stem(), format.extension()), content }
    }

    /// Adobe/Resolve `.cube` 3D LUT of the same transform, sRGB in and out.
    /// Red varies fastest, then green, then blue, per the format.
    pub fn to_cube_lut(&self, size: usize) -> String {
        let size = size.clamp(2, 65);
        let master = curve_lut_f32(&self.tone_curve);
        let chans = [curve_lut_f32(&self.red_curve), curve_lut_f32(&self.green_curve), curve_lut_f32(&self.blue_curve)];
        let title = self.name.replace(['"', '\n', '\r'], " ");
        let mut out = format!(
            "# Created by photo-pick: {} learned from reference photos.\n\
             # Input and output are sRGB-encoded (display) values; apply it to an image in sRGB.\n\
             TITLE \"{title}\"\nLUT_3D_SIZE {size}\nDOMAIN_MIN 0.0 0.0 0.0\nDOMAIN_MAX 1.0 1.0 1.0\n",
            self.contents().join(", ")
        );
        let step = 1.0 / (size - 1) as f32;
        for b in 0..size {
            for g in 0..size {
                for r in 0..size {
                    let [ro, go, bo] = self.apply_rgb_with(&master, &chans, [r as f32 * step, g as f32 * step, b as f32 * step]);
                    out.push_str(&format!("{ro:.6} {go:.6} {bo:.6}\n"));
                }
            }
        }
        out
    }

    fn luts(&self) -> CurveLuts {
        CurveLuts {
            master: curve_lut(&self.tone_curve),
            rgb: [curve_lut(&self.red_curve), curve_lut(&self.green_curve), curve_lut(&self.blue_curve)],
        }
    }

    /// Lightroom Classic / Lightroom / Camera Raw preset. Import via the
    /// Presets panel (+ → Import Presets…).
    pub fn to_lr_preset_xmp(&self) -> String {
        let uuid = uuid::Uuid::new_v4().simple().to_string().to_uppercase();
        let name = xml_escape(&self.name);
        let alt = |v: &str| {
            format!("\n    <rdf:Alt>\n     <rdf:li xml:lang=\"x-default\">{v}</rdf:li>\n    </rdf:Alt>\n   ")
        };
        let seq = |pts: &[(u8, u8)]| {
            let items: String =
                pts.iter().map(|(x, y)| format!("     <rdf:li>{x}, {y}</rdf:li>\n")).collect();
            format!("\n    <rdf:Seq>\n{items}    </rdf:Seq>\n   ")
        };
        let saturation = signed(self.saturation);
        // HSL and grading are written only when fitted: a preset that carries
        // a setting resets it on apply, and zeros would wipe the user's own.
        let mut extra = String::new();
        if self.has_hsl() {
            for (band, adj) in HSL_BANDS.iter().zip(&self.hsl) {
                extra.push_str(&format!("   crs:HueAdjustment{band}=\"{}\"\n", signed(adj.hue)));
                extra.push_str(&format!("   crs:SaturationAdjustment{band}=\"{}\"\n", signed(adj.saturation)));
                extra.push_str(&format!("   crs:LuminanceAdjustment{band}=\"{}\"\n", signed(adj.luminance)));
            }
        }
        if !self.color_grade.is_neutral() {
            let g = &self.color_grade;
            for (key, v) in [
                ("SplitToningShadowHue", g.shadows.hue),
                ("SplitToningShadowSaturation", g.shadows.saturation),
                ("SplitToningHighlightHue", g.highlights.hue),
                ("SplitToningHighlightSaturation", g.highlights.saturation),
                ("SplitToningBalance", 0),
                ("ColorGradeMidtoneHue", g.midtones.hue),
                ("ColorGradeMidtoneSat", g.midtones.saturation),
                ("ColorGradeShadowLum", 0),
                ("ColorGradeMidtoneLum", 0),
                ("ColorGradeHighlightLum", 0),
                ("ColorGradeGlobalHue", 0),
                ("ColorGradeGlobalSat", 0),
                ("ColorGradeGlobalLum", 0),
                ("ColorGradeBlending", 50),
            ] {
                extra.push_str(&format!("   crs:{key}=\"{v}\"\n"));
            }
        }
        let description = {
            let mut c = self.contents().join(", ");
            if let Some(first) = c.get_mut(0..1) {
                first.make_ascii_uppercase();
            }
            format!("{c} learned from reference photos")
        };
        format!(
            r#"<x:xmpmeta xmlns:x="adobe:ns:meta/" x:xmptk="photo-pick">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
    xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
   crs:PresetType="Normal"
   crs:Cluster=""
   crs:UUID="{uuid}"
   crs:SupportsAmount="False"
   crs:SupportsColor="True"
   crs:SupportsMonochrome="True"
   crs:SupportsHighDynamicRange="True"
   crs:SupportsNormalDynamicRange="True"
   crs:SupportsSceneReferred="True"
   crs:SupportsOutputReferred="True"
   crs:CameraModelRestriction=""
   crs:Copyright=""
   crs:ContactInfo=""
   crs:Version="15.0"
   crs:Saturation="{saturation}"
{extra}   crs:ToneCurveName2012="Custom"
   crs:HasSettings="True">
   <crs:Name>{name_alt}</crs:Name>
   <crs:ShortName>{empty}</crs:ShortName>
   <crs:SortName>{empty}</crs:SortName>
   <crs:Group>{group}</crs:Group>
   <crs:Description>{desc}</crs:Description>
   <crs:ToneCurvePV2012>{master}</crs:ToneCurvePV2012>
   <crs:ToneCurvePV2012Red>{red}</crs:ToneCurvePV2012Red>
   <crs:ToneCurvePV2012Green>{green}</crs:ToneCurvePV2012Green>
   <crs:ToneCurvePV2012Blue>{blue}</crs:ToneCurvePV2012Blue>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>
"#,
            name_alt = alt(&name),
            empty = alt(""),
            group = alt("photo-pick"),
            desc = alt(&xml_escape(&description)),
            master = seq(&self.tone_curve),
            red = seq(&self.red_curve),
            green = seq(&self.green_curve),
            blue = seq(&self.blue_curve),
        )
    }

    /// What the preset adjusts, in plain words (for file descriptions).
    pub fn contents(&self) -> Vec<&'static str> {
        let mut v = vec!["tone curve", "RGB curves", "saturation"];
        if self.has_hsl() {
            v.push("HSL");
        }
        if !self.color_grade.is_neutral() {
            v.push("color grading");
        }
        v
    }

    /// A file-name stem derived from the preset name.
    pub fn file_stem(&self) -> String {
        let stem: String = self
            .name
            .chars()
            .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' || c == ' ' { c } else { '_' })
            .collect::<String>()
            .trim()
            .to_string();
        if stem.is_empty() { "photo-pick style".to_string() } else { stem }
    }
}

/// Edge length of exported 3D LUTs: 33³ is the common choice (Photoshop,
/// Resolve, Capture One all read it) and is ample for smooth curves.
const CUBE_SIZE: usize = 33;

/// Export formats for a fitted style. Supporting another app means adding a
/// variant here plus its arm in [`StylePreset::render`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StyleFormat {
    /// Lightroom Classic / Lightroom / Camera Raw preset (`.xmp`). Keeps the
    /// settings editable as sliders and curves; works on RAW.
    LrPreset,
    /// 3D LUT (`.cube`) for Photoshop, Affinity Photo, DaVinci Resolve,
    /// Capture One, darktable, video editors… Baked colour transform.
    CubeLut,
}

impl StyleFormat {
    pub const ALL: [StyleFormat; 2] = [StyleFormat::LrPreset, StyleFormat::CubeLut];

    pub fn extension(self) -> &'static str {
        match self {
            StyleFormat::LrPreset => "xmp",
            StyleFormat::CubeLut => "cube",
        }
    }
}

/// One rendered export.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StyleFile {
    pub format: StyleFormat,
    pub file_name: String,
    pub content: String,
}

fn check_inputs(reference: &StyleStats, baseline: &StyleStats) -> Result<(), StyleError> {
    if reference.images == 0 {
        return Err(StyleError::NoReferences);
    }
    if baseline.images == 0 {
        return Err(StyleError::NoBaseline);
    }
    Ok(())
}

/// Point curve through matched `(from, to)` quantile pairs, blended toward
/// identity by `s`, with lifted blacks / rolled-off whites carried to the
/// endpoints, made monotone and slope-bounded.
fn curve_from_pairs(pairs: &[(f32, f32)], s: f32) -> Vec<(u8, u8)> {
    let pairs: Vec<(f32, f32)> = pairs.iter().map(|(x, y)| (*x, x + s * (y - x))).collect();
    let (x0, y0) = pairs[0];
    let (xn, yn) = pairs[pairs.len() - 1];
    let start = (y0 - x0).clamp(0.0, 0.25);
    let end = 1.0 - (xn - yn).clamp(0.0, 0.25);

    let mut pts = vec![(0.0f32, start)];
    for (x, y) in pairs {
        if x - pts.last().unwrap().0 >= 0.03 && 1.0 - x >= 0.03 {
            pts.push((x, y));
        }
    }
    pts.push((1.0, end));
    for i in 1..pts.len() {
        let (px, py) = pts[i - 1];
        let dx = pts[i].0 - px;
        pts[i].1 = pts[i].1.clamp(py + dx * SLOPE_RANGE.0, py + dx * SLOPE_RANGE.1).min(1.0);
    }
    quantize_curve(&pts)
}

/// Unpaired master curve: black lift, white roll-off and a damped contrast
/// change. Midtone brightness is deliberately not transferred — that's
/// exposure, set per photo.
///
/// The lift is the *smallest* any reference shows over the expected black
/// level (and the roll-off the smallest under the expected white): a real
/// faded grade shows in every reference, while one dim scene without true
/// blacks or whites would otherwise wash out the whole preset. The expected
/// levels are the user's own photos' typical (median) ones, floored at
/// [`BLACK_PRIOR`] / [`WHITE_PRIOR`] — a foggy trip without true blacks
/// shouldn't read references without them as faded.
fn fade_contrast_curve(reference: &StyleStats, baseline: &StyleStats, s: f32) -> Vec<(u8, u8)> {
    let darkest = reference.blacks.iter().copied().fold(f32::INFINITY, f32::min);
    let brightest = reference.whites.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let black = median(&baseline.blacks).max(BLACK_PRIOR);
    let white = median(&baseline.whites).min(WHITE_PRIOR);
    let lift = (darkest - black).clamp(0.0, MAX_FADE);
    let roll = (white - brightest).clamp(0.0, MAX_FADE);
    let (y0, y1) = (s * lift, 1.0 - s * roll);

    let shape = |st: &StyleStats| {
        let outer = (st.quantile(0.95) - st.quantile(0.05)).max(1e-3);
        (st.quantile(0.75) - st.quantile(0.25)) / outer
    };
    let k = (shape(reference) / shape(baseline).max(1e-3)).clamp(CONTRAST_RANGE.0, CONTRAST_RANGE.1);
    let k = 1.0 + 0.5 * (k - 1.0) * s;

    let lin = |t: f32| y0 + t * (y1 - y0);
    quantize_curve(&[
        (0.0, lin(0.0)),
        (0.25, lin(0.5 - 0.25 * k)),
        (0.5, lin(0.5)),
        (0.75, lin(0.5 + 0.25 * k)),
        (1.0, lin(1.0)),
    ])
}

fn median(v: &[f32]) -> f32 {
    let mut v = v.to_vec();
    v.sort_by(f32::total_cmp);
    v.get(v.len() / 2).copied().unwrap_or(0.5)
}

/// Saturation that closes the remaining chroma gap once the curves are
/// applied (contrast changes chroma too), simulated on the kept pixels.
fn residual_saturation(preset: &StylePreset, target: &StyleStats, source: &StyleStats, s: f32) -> i32 {
    let source_chroma = source.mean_chroma();
    if source_chroma <= 1e-3 {
        return 0;
    }
    // How much the curves alone change chroma, measured on the kept pixels
    // before vs after (both on the same subset — comparing the subset with the
    // full-image mean would read sampling bias as a saturation change).
    let curves_gain = if source.kept.is_empty() {
        1.0
    } else {
        let luts = preset.luts();
        let chroma = |p: [u8; 3]| (p[0].max(p[1]).max(p[2]) - p[0].min(p[1]).min(p[2])) as f32;
        let before: f32 = source.kept.iter().map(|p| chroma(*p)).sum();
        let after: f32 = source.kept.iter().map(|p| chroma(luts.apply_curves(*p))).sum();
        if before <= 1e-3 { 1.0 } else { after / before }
    };
    if curves_gain <= 1e-3 {
        return 0;
    }
    let ratio = target.mean_chroma() / source_chroma / curves_gain;
    ((ratio - 1.0) * 100.0 * s).round().clamp(-SATURATION_RANGE as f32, SATURATION_RANGE as f32) as i32
}

/// LR writes positive slider values with an explicit `+`.
fn signed(v: i32) -> String {
    if v > 0 { format!("+{v}") } else { v.to_string() }
}

fn scale_slider(v: i32, s: f32) -> i32 {
    (v as f32 * s).round() as i32
}

// Colours are handled in the chroma plane orthogonal to the gray axis: `E1`
// points at red (hue 0°), `E2` at hue 90°, so `atan2(d·E2, d·E1)` of the
// chroma vector `d = rgb − mean` is the hue with red 0°, yellow 60°, green
// 120°, cyan 180°, blue 240°, magenta 300° — the angles LR's bands and wheels
// use.
const E1: [f32; 3] = [0.816_496_6, -0.408_248_3, -0.408_248_3];
const E2: [f32; 3] = [0.0, std::f32::consts::FRAC_1_SQRT_2, -std::f32::consts::FRAC_1_SQRT_2];

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// `(mean, chroma-plane coordinates)` of a colour.
fn to_plane(rgb: [f32; 3]) -> (f32, f32, f32) {
    let m = (rgb[0] + rgb[1] + rgb[2]) / 3.0;
    (m, dot(rgb, E1), dot(rgb, E2))
}

fn from_plane(m: f32, a: f32, b: f32) -> [f32; 3] {
    [0, 1, 2].map(|c| m + a * E1[c] + b * E2[c])
}

fn hue_deg(a: f32, b: f32) -> f32 {
    b.atan2(a).to_degrees().rem_euclid(360.0)
}

/// Signed difference `to − from` in degrees, in `(−180, 180]`.
fn hue_delta(from: f32, to: f32) -> f32 {
    let d = (to - from).rem_euclid(360.0);
    if d > 180.0 { d - 360.0 } else { d }
}

/// The two LR bands a hue falls between and their linear blend weights.
fn band_weights(h: f32) -> [(usize, f32); 2] {
    let h = h.rem_euclid(360.0);
    for i in 0..8 {
        let lo = HSL_CENTERS[i];
        let hi = if i == 7 { 360.0 } else { HSL_CENTERS[i + 1] };
        if h >= lo && h < hi {
            let t = (h - lo) / (hi - lo);
            return [(i, 1.0 - t), ((i + 1) % 8, t)];
        }
    }
    [(0, 1.0), (1, 0.0)]
}

/// How strongly HSL luminance acts on a colour: full on saturated colours,
/// fading to nothing on grays (LR's luminance sliders don't touch neutrals).
fn hsl_lum_weight(rgb: [f32; 3]) -> f32 {
    let (mx, mn) = (rgb[0].max(rgb[1]).max(rgb[2]), rgb[0].min(rgb[1]).min(rgb[2]));
    if mx <= 1e-4 { 0.0 } else { ((mx - mn) / mx / 0.5).min(1.0) }
}

/// Model of LR's HSL panel: per-band hue rotation, chroma scale and
/// brightness scale, blended between the two nearest bands.
fn apply_hsl(hsl: &[HslAdjust; 8], rgb: [f32; 3]) -> [f32; 3] {
    let (m, a, b) = to_plane(rgb);
    let chroma = (a * a + b * b).sqrt();
    if chroma < 1e-5 {
        return rgb;
    }
    let (mut dh, mut ds, mut dl) = (0.0f32, 0.0f32, 0.0f32);
    for (band, w) in band_weights(hue_deg(a, b)) {
        dh += w * hsl[band].hue as f32;
        ds += w * hsl[band].saturation as f32;
        dl += w * hsl[band].luminance as f32;
    }
    let theta = (dh / 100.0 * HSL_HUE_SPAN).to_radians();
    let k = (1.0 + ds / 100.0).max(0.0);
    let (sin, cos) = theta.sin_cos();
    let (a2, b2) = ((a * cos - b * sin) * k, (a * sin + b * cos) * k);
    let f = (1.0 + HSL_LUM_SPAN * hsl_lum_weight(rgb) * dl / 100.0).max(0.0);
    from_plane(m, a2, b2).map(|c| c * f)
}

/// Shadow / midtone / highlight weights of a luma; they sum to 1.
fn zone_weights(y: f32) -> [f32; 3] {
    [(1.0 - y) * (1.0 - y), 2.0 * y * (1.0 - y), y * y]
}

/// RGB tint a colour-grading wheel adds at full zone weight.
fn wheel_tint(w: GradeWheel) -> [f32; 3] {
    let mag = w.saturation as f32 / 100.0 * GRADE_MAX_TINT;
    let h = (w.hue as f32).to_radians();
    from_plane(0.0, mag * h.cos(), mag * h.sin())
}

/// Chroma scale of the near-neutral weighting for pixel-fitted curves.
const NEUTRAL_CHROMA: f32 = 0.06;
/// Minimum near-neutral pixel mass (in pixels) for pixel-fitted curves.
const MIN_NEUTRAL_MASS: f32 = 300.0;
const REGRESSION_BINS: usize = 64;
/// Input segments of a pixel-fitted curve (≤ one point each).
const CURVE_SEGMENTS: usize = 12;
/// Near-neutral pixel mass a segment needs to get a curve point.
const MIN_SEGMENT_MASS: f32 = 30.0;

/// Weighted conditional mean of `y` given `x` (both 0–1), as `(mean x, mean
/// y, weight)` per populated bin.
fn binned_regression(samples: impl Iterator<Item = (f32, f32, f32)>) -> Vec<(f32, f32, f32)> {
    let mut acc = vec![(0.0f64, 0.0f64, 0.0f64); REGRESSION_BINS];
    for (x, y, w) in samples {
        let b = ((x.clamp(0.0, 1.0) * REGRESSION_BINS as f32) as usize).min(REGRESSION_BINS - 1);
        acc[b].0 += (w * x) as f64;
        acc[b].1 += (w * y) as f64;
        acc[b].2 += w as f64;
    }
    acc.into_iter()
        .filter(|(_, _, w)| *w > 1.0)
        .map(|(sx, sy, w)| ((sx / w) as f32, (sy / w) as f32, w as f32))
        .collect()
}

/// `(from, to)` pairs for [`curve_from_pairs`] from a binned regression:
/// the bins merged into [`CURVE_SEGMENTS`] even input segments, keeping each
/// with enough weight. Even spacing (rather than percentiles) matters: a few
/// reference photos' neutral pixels are often mostly very dark or very
/// bright, and percentiles would leave the midtones without a point.
fn regression_pairs(bins: &[(f32, f32, f32)]) -> Vec<(f32, f32)> {
    let mut seg = [(0.0f32, 0.0f32, 0.0f32); CURVE_SEGMENTS];
    for (x, y, w) in bins {
        let k = ((x * CURVE_SEGMENTS as f32) as usize).min(CURVE_SEGMENTS - 1);
        seg[k].0 += x * w;
        seg[k].1 += y * w;
        seg[k].2 += w;
    }
    seg.iter().filter(|(_, _, w)| *w >= MIN_SEGMENT_MASS).map(|(x, y, w)| (x / w, (y / w).clamp(0.0, 1.0))).collect()
}

/// Paired curves and saturation fitted on aligned pixels: master and RGB
/// curves by regression on near-neutral pixels (which carry the tone and the
/// per-tone casts, but no hue for HSL to act on), saturation by the chroma
/// ratio of the rest. `None` without enough near-neutral pixels.
fn pixel_curves(name: &str, pairs: &[([f32; 3], [f32; 3])], s: f32, casts: CastModel) -> Option<StylePreset> {
    if pairs.is_empty() {
        return None;
    }
    let neutral = |o: &[f32; 3]| {
        let (_, a, b) = to_plane(*o);
        (-(a * a + b * b) / (NEUTRAL_CHROMA * NEUTRAL_CHROMA)).exp()
    };
    let weights: Vec<f32> = pairs.iter().map(|(o, _)| neutral(o)).collect();
    if weights.iter().sum::<f32>() < MIN_NEUTRAL_MASS {
        return None;
    }
    let mean = |p: &[f32; 3]| (p[0] + p[1] + p[2]) / 3.0;
    // Master curve on the mean (tone without the cast; luma would fold a
    // green cast into brightness).
    let master_bins = binned_regression(pairs.iter().zip(&weights).map(|((o, e), w)| (mean(o), mean(e), *w)));
    let master_pairs = regression_pairs(&master_bins);
    if master_pairs.len() < 3 {
        return None;
    }
    let tone_curve = curve_from_pairs(&master_pairs, s);
    let master = curve_lut_f32(&tone_curve);
    let channel = |c: usize| {
        if casts == CastModel::Wheels {
            return vec![(0, 0), (255, 255)];
        }
        let bins = binned_regression(
            pairs.iter().zip(&weights).map(|((o, e), w)| (sample_lut(&master, o[c] * 255.0) / 255.0, e[c], *w)),
        );
        let pairs = regression_pairs(&bins);
        if pairs.len() < 3 { vec![(0, 0), (255, 255)] } else { curve_from_pairs(&pairs, s) }
    };
    let mut preset = StylePreset {
        name: name.to_string(),
        tone_curve,
        red_curve: channel(0),
        green_curve: channel(1),
        blue_curve: channel(2),
        saturation: 0,
        hsl: Default::default(),
        color_grade: ColorGrade::default(),
    };

    // Global saturation: chroma-weighted mean chroma ratio once the curves
    // are applied. Per-colour departures from it are left to HSL.
    let (master, chans) = preset.float_luts();
    let (mut num, mut den) = (0.0f64, 0.0f64);
    for (o, e) in pairs {
        let sim = preset.apply_rgb_with(&master, &chans, *o);
        let (_, a_s, b_s) = to_plane(sim);
        let cs = (a_s * a_s + b_s * b_s).sqrt();
        if cs < HSL_MIN_CHROMA {
            continue;
        }
        let (_, a_e, b_e) = to_plane(*e);
        let ce = (a_e * a_e + b_e * b_e).sqrt();
        num += (cs * (ce / cs).min(3.0)) as f64;
        den += cs as f64;
    }
    if den > 0.0 {
        let ratio = (num / den) as f32;
        preset.saturation =
            ((ratio - 1.0) * 100.0 * s).round().clamp(-SATURATION_RANGE as f32, SATURATION_RANGE as f32) as i32;
    }
    Some(preset)
}

/// How per-tone colour casts are expressed in a pixel-fitted preset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CastModel {
    /// Red / green / blue curves (act per channel).
    Curves,
    /// Colour-grading wheels (act by brightness); RGB curves stay flat.
    Wheels,
}

/// Mean absolute per-channel error (0–1) of a preset over the pixel pairs.
fn pixel_error(p: &StylePreset, pairs: &[([f32; 3], [f32; 3])]) -> f32 {
    let (master, chans) = p.float_luts();
    let sum: f32 = pairs
        .iter()
        .map(|(o, e)| {
            let r = p.apply_rgb_with(&master, &chans, *o);
            (0..3).map(|c| (r[c] - e[c]).abs()).sum::<f32>()
        })
        .sum();
    sum / (3 * pairs.len().max(1)) as f32
}

/// `base` (curves and saturation) plus fitted colour grading and HSL.
fn with_selective(base: StylePreset, pairs: &[([f32; 3], [f32; 3])]) -> StylePreset {
    let (hsl, color_grade) = fit_selective(&base, pairs);
    StylePreset { hsl, color_grade, ..base }
}

/// Least-squares colour-grading wheels and HSL sliders on top of `base`
/// (curves and saturation only). Grading first, from near-neutral pixels —
/// HSL doesn't touch those, so they show the tint alone — then HSL on the
/// coloured pixels with that tint taken out.
fn fit_selective(base: &StylePreset, pairs: &[([f32; 3], [f32; 3])]) -> ([HslAdjust; 8], ColorGrade) {
    let (master, chans) = base.float_luts();
    let sim: Vec<[f32; 3]> = pairs.iter().map(|(o, _)| base.apply_rgb_with(&master, &chans, *o)).collect();

    let mut grade_sys = NormalEq::new(3);
    let mut rhs = [[0.0f64; 3]; 2];
    for (s, (o, e)) in sim.iter().zip(pairs) {
        let (_, ao, bo) = to_plane(*o);
        let wt = (-(ao * ao + bo * bo) / (NEUTRAL_CHROMA * NEUTRAL_CHROMA)).exp();
        if wt < 1e-3 {
            continue;
        }
        let (_, a_s, b_s) = to_plane(*s);
        let (_, ae, be) = to_plane(*e);
        let zw = zone_weights(luma(s[0], s[1], s[2]).clamp(0.0, 1.0));
        grade_sys.add_matrix(&zw, wt);
        for z in 0..3 {
            rhs[0][z] += (wt * zw[z] * (ae - a_s)) as f64;
            rhs[1][z] += (wt * zw[z] * (be - b_s)) as f64;
        }
    }
    let ta = grade_sys.solve_rhs(&rhs[0], &[false; 3]);
    let tb = grade_sys.solve_rhs(&rhs[1], &[false; 3]);
    let wheel = |z: usize| {
        let sat = ((ta[z].hypot(tb[z]) / GRADE_MAX_TINT * 100.0).round() as i32).clamp(0, 100);
        if sat < SLIDER_DEADBAND {
            GradeWheel::default()
        } else {
            GradeWheel { hue: (hue_deg(ta[z], tb[z]).round() as i32) % 360, saturation: sat }
        }
    };
    let grade = ColorGrade { shadows: wheel(0), midtones: wheel(1), highlights: wheel(2) };

    // HSL: one regression per slider kind, regressors = band weights of the
    // simulated colour's hue, samples weighted by its chroma. Targets are the
    // edits minus the grading tint (grading is applied after HSL).
    let tints = grade.wheels().map(wheel_tint);
    let mut hue_sys = NormalEq::new(8);
    let mut sat_sys = NormalEq::new(8);
    let mut lum_sys = NormalEq::new(8);
    let mut band_mass = [0.0f32; 8];
    for (s, (_, e)) in sim.iter().zip(pairs) {
        let (ms, a_s, b_s) = to_plane(*s);
        let cs = (a_s * a_s + b_s * b_s).sqrt();
        if cs < HSL_MIN_CHROMA || ms < 0.02 {
            continue;
        }
        let zw = zone_weights(luma(s[0], s[1], s[2]).clamp(0.0, 1.0));
        let e = [0, 1, 2].map(|c| e[c] - (0..3).map(|z| zw[z] * tints[z][c]).sum::<f32>());
        let (me, a_e, b_e) = to_plane(e);
        let ce = (a_e * a_e + b_e * b_e).sqrt();
        let hs = hue_deg(a_s, b_s);
        let mut x = [0.0f32; 8];
        for (band, w) in band_weights(hs) {
            x[band] = w;
            band_mass[band] += w * cs;
        }
        // Slider units: hue degrees / span, chroma ratio, brightness ratio.
        if ce >= HSL_MIN_CHROMA * 0.5 {
            hue_sys.add(&x, hue_delta(hs, hue_deg(a_e, b_e)) / HSL_HUE_SPAN * 100.0, cs);
        }
        sat_sys.add(&x, ((ce / cs - 1.0) * 100.0).clamp(-100.0, 200.0), cs);
        let lw = HSL_LUM_SPAN * hsl_lum_weight(*s) / 100.0;
        if lw > 1e-4 {
            let xl = x.map(|v| v * lw);
            lum_sys.add(&xl, (me / ms - 1.0).clamp(-0.9, 2.0), cs);
        }
    }
    let total: f32 = band_mass.iter().sum();
    let weak: Vec<bool> = band_mass.iter().map(|m| total <= 0.0 || m / total < HSL_MIN_BAND_SHARE).collect();
    let solve = |sys: &NormalEq| -> [i32; 8] {
        let x = sys.solve(&weak);
        let mut out = [0i32; 8];
        for (o, v) in out.iter_mut().zip(x) {
            let v = (v.round() as i32).clamp(-100, 100);
            *o = if v.abs() < SLIDER_DEADBAND { 0 } else { v };
        }
        out
    };
    let (hue, sat, lum) = (solve(&hue_sys), solve(&sat_sys), solve(&lum_sys));
    let mut hsl = [HslAdjust::default(); 8];
    for i in 0..8 {
        hsl[i] = HslAdjust { hue: hue[i], saturation: sat[i], luminance: lum[i] };
    }
    (hsl, grade)
}

/// Weighted least-squares normal equations `AᵀWA x = AᵀWy`, solved with a
/// small ridge so sparse bands stay near zero instead of blowing up.
struct NormalEq {
    n: usize,
    a: Vec<f64>,
    b: Vec<f64>,
}

#[allow(clippy::needless_range_loop)] // index-heavy linear algebra reads clearer this way
impl NormalEq {
    fn new(n: usize) -> Self {
        Self { n, a: vec![0.0; n * n], b: vec![0.0; n] }
    }

    fn add_matrix(&mut self, x: &[f32], w: f32) {
        for i in 0..self.n {
            if x[i] == 0.0 {
                continue;
            }
            for j in 0..self.n {
                self.a[i * self.n + j] += (w * x[i] * x[j]) as f64;
            }
        }
    }

    fn add(&mut self, x: &[f32], y: f32, w: f32) {
        self.add_matrix(x, w);
        for i in 0..self.n {
            self.b[i] += (w * x[i] * y) as f64;
        }
    }

    fn solve(&self, fixed_zero: &[bool]) -> Vec<f32> {
        self.solve_rhs(&self.b, fixed_zero)
    }

    /// Gaussian elimination with partial pivoting; `fixed_zero` unknowns are
    /// pinned to 0.
    fn solve_rhs(&self, rhs: &[f64], fixed_zero: &[bool]) -> Vec<f32> {
        let n = self.n;
        let trace: f64 = (0..n).map(|i| self.a[i * n + i]).sum();
        let ridge = 1e-3 * trace / n as f64 + 1e-9;
        // A pinned unknown gets the row `x_i = 0` and drops out of the others.
        let mut m: Vec<Vec<f64>> = (0..n)
            .map(|i| {
                let mut row: Vec<f64> = (0..n)
                    .map(|j| if fixed_zero[i] || fixed_zero[j] { 0.0 } else { self.a[i * n + j] })
                    .collect();
                row.push(if fixed_zero[i] { 0.0 } else { rhs[i] });
                row[i] = if fixed_zero[i] { 1.0 } else { row[i] + ridge };
                row
            })
            .collect();
        for col in 0..n {
            let pivot = (col..n).max_by(|&a, &b| m[a][col].abs().total_cmp(&m[b][col].abs())).unwrap();
            m.swap(col, pivot);
            let p = m[col][col];
            if p.abs() < 1e-12 {
                continue;
            }
            for r in 0..n {
                if r != col {
                    let f = m[r][col] / p;
                    if f != 0.0 {
                        for c in col..=n {
                            m[r][c] -= f * m[col][c];
                        }
                    }
                }
            }
        }
        (0..n).map(|i| if m[i][i].abs() < 1e-12 { 0.0 } else { (m[i][n] / m[i][i]) as f32 }).collect()
    }
}

fn quantize_curve(pts: &[(f32, f32)]) -> Vec<(u8, u8)> {
    let mut out: Vec<(u8, u8)> = Vec::with_capacity(pts.len());
    for (x, y) in pts {
        let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        let (qx, qy) = (q(*x), q(*y));
        match out.last() {
            Some((lx, _)) if *lx >= qx => continue,
            _ => {}
        }
        let qy = out.last().map_or(qy, |(_, ly)| qy.max(*ly));
        out.push((qx, qy));
    }
    out
}

struct CurveLuts {
    master: [u8; 256],
    rgb: [[u8; 256]; 3],
}

impl CurveLuts {
    fn apply_curves(&self, px: [u8; 3]) -> [u8; 3] {
        [0, 1, 2].map(|c| self.rgb[c][self.master[px[c] as usize] as usize])
    }
}

/// [`curve_lut_f32`] rounded to 8 bits, for fast per-pixel previews.
fn curve_lut(pts: &[(u8, u8)]) -> [u8; 256] {
    curve_lut_f32(pts).map(|v| v.round().clamp(0.0, 255.0) as u8)
}

/// Linear interpolation into a 256-entry LUT at fractional position `x`.
fn sample_lut(lut: &[f32; 256], x: f32) -> f32 {
    let x = x.clamp(0.0, 255.0);
    let i = (x.floor() as usize).min(254);
    let t = x - i as f32;
    lut[i] * (1.0 - t) + lut[i + 1] * t
}

/// Monotone cubic (Fritsch–Carlson) interpolation of a point curve into a LUT.
fn curve_lut_f32(pts: &[(u8, u8)]) -> [f32; 256] {
    let mut lut = [0f32; 256];
    if pts.len() < 2 {
        for (i, v) in lut.iter_mut().enumerate() {
            *v = i as f32;
        }
        return lut;
    }
    let xs: Vec<f32> = pts.iter().map(|p| p.0 as f32).collect();
    let ys: Vec<f32> = pts.iter().map(|p| p.1 as f32).collect();
    let n = xs.len();
    let d: Vec<f32> = (0..n - 1).map(|i| (ys[i + 1] - ys[i]) / (xs[i + 1] - xs[i])).collect();
    let mut m = vec![0.0f32; n];
    m[0] = d[0];
    m[n - 1] = d[n - 2];
    for i in 1..n - 1 {
        m[i] = if d[i - 1] * d[i] <= 0.0 { 0.0 } else { (d[i - 1] + d[i]) / 2.0 };
    }
    for i in 0..n - 1 {
        if d[i] == 0.0 {
            m[i] = 0.0;
            m[i + 1] = 0.0;
        } else {
            let (a, b) = (m[i] / d[i], m[i + 1] / d[i]);
            let h = a * a + b * b;
            if h > 9.0 {
                let t = 3.0 / h.sqrt();
                m[i] = t * a * d[i];
                m[i + 1] = t * b * d[i];
            }
        }
    }
    for (xi, v) in lut.iter_mut().enumerate() {
        let x = xi as f32;
        let y = if x <= xs[0] {
            ys[0]
        } else if x >= xs[n - 1] {
            ys[n - 1]
        } else {
            let k = (0..n - 1).find(|&k| x <= xs[k + 1]).unwrap_or(n - 2);
            let h = xs[k + 1] - xs[k];
            let t = (x - xs[k]) / h;
            let (t2, t3) = (t * t, t * t * t);
            (2.0 * t3 - 3.0 * t2 + 1.0) * ys[k]
                + (t3 - 2.0 * t2 + t) * h * m[k]
                + (-2.0 * t3 + 3.0 * t2) * ys[k + 1]
                + (t3 - t2) * h * m[k + 1]
        };
        *v = y.clamp(0.0, 255.0);
    }
    lut
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Procedural "photo": neutral gradient sky, gray buildings, a green and a
    /// red patch, mid-gray ground. Plenty of near-neutral pixels in every zone.
    fn scene(seed: u32) -> DynamicImage {
        let (w, h) = (480u32, 320u32);
        DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |x, y| {
            let (fx, fy) = (x as f32 / w as f32, y as f32 / h as f32);
            let shift = (seed % 7) as f32 * 0.02;
            let mut v = if fy < 0.4 { 0.85 - fy * 0.5 + shift } else { 0.45 - (fy - 0.4) * 0.5 };
            if fy > 0.25 && fy < 0.75 && ((x / 60 + seed) % 3 == 0) {
                v = 0.15 + fx * 0.2;
            }
            let mut px = [v, v, v];
            if (fx - 0.3).abs() < 0.08 && (fy - 0.6).abs() < 0.1 {
                px = [0.25, 0.55, 0.2];
            }
            if (fx - 0.7 - shift).abs() < 0.06 && (fy - 0.5).abs() < 0.08 {
                px = [0.7, 0.2, 0.15];
            }
            image::Rgb(px.map(|c| (c.clamp(0.0, 1.0) * 255.0) as u8))
        }))
    }

    /// A known grade: S-curve contrast, lifted blacks, warm highlights, teal
    /// shadows, 25 % desaturation.
    fn grade(img: &DynamicImage) -> DynamicImage {
        let mut out = img.to_rgb8();
        for p in out.pixels_mut() {
            let [r, g, b] = p.0.map(|c| c as f32 / 255.0);
            let tone = |v: f32| {
                let s = v + 0.25 * (v - 0.5) * (1.0 - (2.0 * v - 1.0).abs());
                0.06 + 0.92 * s
            };
            let [mut r, mut g, mut b] = [tone(r), tone(g), tone(b)];
            let y = luma(r, g, b);
            let hi = ((y - 0.5) * 2.0).clamp(0.0, 1.0);
            let sh = ((0.5 - y) * 2.0).clamp(0.0, 1.0);
            r += 0.05 * hi - 0.04 * sh;
            g += 0.01 * sh;
            b += -0.05 * hi + 0.04 * sh;
            let y = luma(r, g, b);
            let [r, g, b] = [r, g, b].map(|c| y + (c - y) * 0.75);
            p.0 = [r, g, b].map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8);
        }
        DynamicImage::ImageRgb8(out)
    }

    fn stats(imgs: &[DynamicImage]) -> StyleStats {
        let mut s = StyleStats::new();
        for i in imgs {
            s.add_image(i);
        }
        s
    }

    fn luma_err(a: &StyleStats, b: &StyleStats) -> f32 {
        [0.005f32, 0.05, 0.15, 0.30, 0.50, 0.70, 0.85, 0.95, 0.995].iter().map(|p| (a.quantile(*p) - b.quantile(*p)).abs()).sum()
    }

    fn cast_err(a: &StyleStats, b: &StyleStats) -> f32 {
        (0..3)
            .filter_map(|z| Some((a.zone_cast(z)?, b.zone_cast(z)?)))
            .map(|(x, y)| (0..3).map(|c| (x[c] - y[c]).abs()).sum::<f32>())
            .sum()
    }

    #[test]
    fn identical_sets_give_identity_settings() {
        let s = stats(&[scene(1), scene(2)]);
        let p = StylePreset::fit_paired("x", &s, &s, &PairedPixels::new(), 1.0).unwrap();
        for (x, y) in p.tone_curve.iter().chain(&p.red_curve).chain(&p.blue_curve) {
            assert!((*x as i32 - *y as i32).abs() <= 2, "{x} → {y}");
        }
        assert!(p.saturation.abs() <= 2, "saturation {}", p.saturation);
        let u = StylePreset::fit_unpaired("x", &s, &s, 1.0).unwrap();
        assert!(u.tone_curve.iter().all(|(x, y)| (*x as i32 - *y as i32).abs() <= 6), "{:?}", u.tone_curve);
    }

    #[test]
    fn zero_strength_is_identity() {
        let base = stats(&[scene(1)]);
        let reference = stats(&[grade(&scene(1))]);
        for p in [
            StylePreset::fit_paired("x", &reference, &base, &PairedPixels::new(), 0.0).unwrap(),
            StylePreset::fit_unpaired("x", &reference, &base, 0.0).unwrap(),
        ] {
            for c in [&p.tone_curve, &p.red_curve, &p.green_curve, &p.blue_curve] {
                assert!(c.iter().all(|(x, y)| (*x as i32 - *y as i32).abs() <= 1), "{c:?}");
            }
            assert_eq!(p.saturation, 0);
        }
    }

    /// Paired: the fitted preset, applied to the originals, must land much
    /// closer to the edited versions than the originals started.
    #[test]
    fn paired_fit_recovers_a_known_grade() {
        let bases: Vec<_> = (0..3).map(scene).collect();
        let refs: Vec<_> = bases.iter().map(grade).collect();
        let (b, r) = (stats(&bases), stats(&refs));
        let p = StylePreset::fit_paired("x", &r, &b, &PairedPixels::new(), 1.0).unwrap();
        let applied: Vec<_> =
            bases.iter().map(|i| DynamicImage::ImageRgb8(p.apply_preview(i))).collect();
        let a = stats(&applied);

        let (l0, l1) = (luma_err(&b, &r), luma_err(&a, &r));
        assert!(l1 < 0.4 * l0, "luma error {l0:.3} → {l1:.3}");
        let (c0, c1) = (cast_err(&b, &r), cast_err(&a, &r));
        assert!(c1 < 0.5 * c0, "cast error {c0:.3} → {c1:.3}");
        let chroma_ratio = a.mean_chroma() / r.mean_chroma();
        assert!((chroma_ratio - 1.0).abs() < 0.15, "chroma ratio {chroma_ratio:.3}");
        assert!(p.saturation < 0, "desaturating grade should lower saturation");
    }

    /// Unpaired: learned on one scene, applied to a different one, the split
    /// tone (warm highlights, cool shadows) must carry over.
    #[test]
    fn unpaired_split_tone_transfers_across_content() {
        let r = stats(&[grade(&scene(5))]);
        let b = stats(&[scene(1), scene(2)]);
        let p = StylePreset::fit_unpaired("x", &r, &b, 1.0).unwrap();
        let at = |curve: &[(u8, u8)], x: f32| {
            let lut = curve_lut(curve);
            lut[(x * 255.0) as usize] as i32 - (x * 255.0) as i32
        };
        assert!(at(&p.red_curve, 0.82) > 0 && at(&p.blue_curve, 0.82) < 0, "warm highlights");
        assert!(at(&p.red_curve, 0.18) < 0 && at(&p.blue_curve, 0.18) > 0, "cool shadows");
    }

    /// Unpaired: a reference that is merely a brighter, warmer *scene* (no
    /// grade at all) must not brighten or warm the preset — the failure that
    /// made naive cross-scene matching worse than doing nothing.
    #[test]
    fn unpaired_ignores_scene_brightness_and_overall_cast() {
        let warm_bright = |img: &DynamicImage| {
            let mut o = img.to_rgb8();
            for p in o.pixels_mut() {
                let [r, g, b] = p.0.map(|c| c as f32);
                p.0 = [r * 1.3, g * 1.2, b * 1.0]
                    .map(|v| v.clamp(0.0, 255.0) as u8);
            }
            DynamicImage::ImageRgb8(o)
        };
        let r = stats(&[warm_bright(&scene(2)), warm_bright(&scene(4))]);
        let b = stats(&[scene(1), scene(3)]);
        let p = StylePreset::fit_unpaired("x", &r, &b, 1.0).unwrap();
        let mid = curve_lut(&p.tone_curve)[128] as i32;
        assert!((mid - 128).abs() <= 8, "midtones moved to {mid}");
        let at = |curve: &[(u8, u8)]| curve_lut(curve)[128] as i32 - 128;
        assert!(at(&p.red_curve).abs() <= 3 && at(&p.blue_curve).abs() <= 3, "overall cast leaked");
        assert_eq!(p.saturation, 0);
    }

    /// Unpaired fade needs every reference to agree: one frame that simply
    /// lacks deep shadows must not lift the blacks of the whole preset.
    #[test]
    fn unpaired_fade_requires_agreement() {
        let lifted = |img: &DynamicImage| {
            let mut o = img.to_rgb8();
            for p in o.pixels_mut() {
                p.0 = p.0.map(|c| (30.0 + c as f32 * 0.85) as u8);
            }
            DynamicImage::ImageRgb8(o)
        };
        let b = stats(&[scene(1)]);
        let all_faded = stats(&[lifted(&scene(2)), lifted(&scene(3))]);
        let p = StylePreset::fit_unpaired("x", &all_faded, &b, 1.0).unwrap();
        assert!(p.tone_curve[0].1 >= 15, "faded set should lift blacks: {:?}", p.tone_curve);
        let mixed = stats(&[lifted(&scene(2)), scene(3)]);
        let p = StylePreset::fit_unpaired("x", &mixed, &b, 1.0).unwrap();
        assert!(p.tone_curve[0].1 <= 3, "one unfaded reference vetoes the lift: {:?}", p.tone_curve);
    }

    #[test]
    fn curves_are_valid_point_curves() {
        let (r, b) = (stats(&[grade(&scene(3))]), stats(&[scene(4)]));
        for p in [
            StylePreset::fit_paired("x", &r, &b, &PairedPixels::new(), 1.0).unwrap(),
            StylePreset::fit_unpaired("x", &r, &b, 1.0).unwrap(),
        ] {
            for c in [&p.tone_curve, &p.red_curve, &p.green_curve, &p.blue_curve] {
                assert_eq!(c.first().unwrap().0, 0);
                assert_eq!(c.last().unwrap().0, 255);
                assert!(c.windows(2).all(|w| w[1].0 > w[0].0 && w[1].1 >= w[0].1), "{c:?}");
            }
        }
    }

    #[test]
    fn empty_inputs_are_errors() {
        let s = stats(&[scene(1)]);
        let e = StyleStats::new();
        assert_eq!(StylePreset::fit_paired("x", &e, &s, &PairedPixels::new(), 1.0), Err(StyleError::NoReferences));
        assert_eq!(StylePreset::fit_unpaired("x", &s, &e, 1.0), Err(StyleError::NoBaseline));
    }

    #[test]
    fn merge_matches_sequential_accumulation() {
        let (a, b) = (scene(1), scene(2));
        let seq = stats(&[a.clone(), b.clone()]);
        let mut merged = stats(&[a]);
        merged.merge(stats(&[b]));
        assert_eq!(merged.image_count(), 2);
        assert!((seq.quantile(0.5) - merged.quantile(0.5)).abs() < 1e-6);
        assert!((seq.mean_chroma() - merged.mean_chroma()).abs() < 1e-6);
    }

    /// Trilinear lookup into a parsed `.cube` body (red fastest).
    fn cube_lookup(values: &[[f32; 3]], n: usize, rgb: [f32; 3]) -> [f32; 3] {
        let pos = rgb.map(|v| v.clamp(0.0, 1.0) * (n - 1) as f32);
        let i0 = pos.map(|p| (p.floor() as usize).min(n - 2));
        let t = [pos[0] - i0[0] as f32, pos[1] - i0[1] as f32, pos[2] - i0[2] as f32];
        let at = |r: usize, g: usize, b: usize| values[r + n * (g + n * b)];
        let mut out = [0.0f32; 3];
        for dr in 0..2 {
            for dg in 0..2 {
                for db in 0..2 {
                    let w = (if dr == 1 { t[0] } else { 1.0 - t[0] })
                        * (if dg == 1 { t[1] } else { 1.0 - t[1] })
                        * (if db == 1 { t[2] } else { 1.0 - t[2] });
                    let v = at(i0[0] + dr, i0[1] + dg, i0[2] + db);
                    for c in 0..3 {
                        out[c] += w * v[c];
                    }
                }
            }
        }
        out
    }

    fn parse_cube(text: &str) -> (usize, Vec<[f32; 3]>) {
        let mut n = 0;
        let mut vals = Vec::new();
        for line in text.lines() {
            if let Some(v) = line.strip_prefix("LUT_3D_SIZE ") {
                n = v.trim().parse().unwrap();
            } else if line.starts_with(|c: char| c.is_ascii_digit() || c == '-') {
                let f: Vec<f32> = line.split_whitespace().map(|x| x.parse().unwrap()).collect();
                vals.push([f[0], f[1], f[2]]);
            }
        }
        (n, vals)
    }

    /// The `.cube` must encode the same transform the preview (and the LR
    /// preset's curves) apply: looking colours up in the LUT has to agree with
    /// applying the preset directly.
    #[test]
    fn cube_lut_matches_the_preset_transform() {
        let p = StylePreset::fit_paired("Cube test", &stats(&[grade(&scene(2))]), &stats(&[scene(2)]), &PairedPixels::new(), 1.0)
            .unwrap();
        let f = p.render(StyleFormat::CubeLut);
        assert_eq!(f.file_name, "Cube test.cube");
        assert!(f.content.contains("TITLE \"Cube test\"") && f.content.contains("LUT_3D_SIZE 33"));
        let (n, vals) = parse_cube(&f.content);
        assert_eq!(n, 33);
        assert_eq!(vals.len(), 33 * 33 * 33);
        assert!(vals.iter().flatten().all(|v| (0.0..=1.0).contains(v)));
        let mut worst = 0.0f32;
        for i in 0..500u32 {
            let rgb = [(i * 37 % 256) as f32 / 255.0, (i * 91 % 256) as f32 / 255.0, (i * 53 % 256) as f32 / 255.0];
            let direct = p.apply_rgb(rgb);
            let via_lut = cube_lookup(&vals, n, rgb);
            for c in 0..3 {
                worst = worst.max((direct[c] - via_lut[c]).abs());
            }
        }
        assert!(worst < 2.0 / 255.0, "LUT deviates from the preset by {:.4}", worst);
    }

    #[test]
    fn identity_preset_bakes_an_identity_lut() {
        let s = stats(&[scene(1)]);
        let p = StylePreset::fit_paired("id", &s, &s, &PairedPixels::new(), 0.0).unwrap();
        let (n, vals) = parse_cube(&p.to_cube_lut(17));
        assert_eq!(n, 17);
        for (i, v) in vals.iter().enumerate() {
            let expect = [(i % 17) as f32 / 16.0, ((i / 17) % 17) as f32 / 16.0, (i / 289) as f32 / 16.0];
            for c in 0..3 {
                assert!((v[c] - expect[c]).abs() < 0.01, "entry {i}: {v:?} vs {expect:?}");
            }
        }
    }

    /// A scene with every LR hue band present at several brightnesses, on a
    /// neutral gradient.
    fn colourful(seed: u32) -> DynamicImage {
        let (w, h) = (480u32, 320u32);
        let hues = [0.0f32, 30.0, 60.0, 120.0, 180.0, 220.0, 270.0, 315.0];
        DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |x, y| {
            let (fx, fy) = (x as f32 / w as f32, y as f32 / h as f32);
            let v = 0.2 + 0.6 * fx;
            let mut px = [v, v, v];
            let band = ((fx * 8.0) as usize + seed as usize) % 8;
            if fy > 0.15 && fy < 0.85 && (x % 60) > 8 {
                let val = 0.35 + 0.55 * (1.0 - fy);
                px = hsv(hues[band], 0.75, val);
            }
            image::Rgb(px.map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8))
        }))
    }

    fn hsv(h: f32, s: f32, v: f32) -> [f32; 3] {
        let c = v * s;
        let hp = h.rem_euclid(360.0) / 60.0;
        let x = c * (1.0 - (hp % 2.0 - 1.0).abs());
        let (r, g, b) = match hp as u32 {
            0 => (c, x, 0.0),
            1 => (x, c, 0.0),
            2 => (0.0, c, x),
            3 => (0.0, x, c),
            4 => (x, 0.0, c),
            _ => (c, 0.0, x),
        };
        let m = v - c;
        [r + m, g + m, b + m]
    }

    fn to_hsv(p: [f32; 3]) -> (f32, f32, f32) {
        let (mx, mn) = (p[0].max(p[1]).max(p[2]), p[0].min(p[1]).min(p[2]));
        let c = mx - mn;
        let h = if c <= 0.0 {
            0.0
        } else if mx == p[0] {
            60.0 * ((p[1] - p[2]) / c).rem_euclid(6.0)
        } else if mx == p[1] {
            60.0 * ((p[2] - p[0]) / c + 2.0)
        } else {
            60.0 * ((p[0] - p[1]) / c + 4.0)
        };
        (h, if mx > 0.0 { c / mx } else { 0.0 }, mx)
    }

    /// A grade only selective tools can express: greens desaturated, blues
    /// darkened, oranges pushed toward yellow — written in HSV, independent
    /// of the HSL model being fitted.
    fn selective_grade(img: &DynamicImage) -> DynamicImage {
        let mut out = img.to_rgb8();
        for p in out.pixels_mut() {
            let (mut h, mut s, mut v) = to_hsv(p.0.map(|c| c as f32 / 255.0));
            if (90.0..150.0).contains(&h) {
                s *= 0.5;
            }
            if (200.0..250.0).contains(&h) {
                v *= 0.8;
            }
            if (15.0..45.0).contains(&h) {
                h += 10.0;
            }
            p.0 = hsv(h, s, v).map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8);
        }
        DynamicImage::ImageRgb8(out)
    }

    fn mean_abs_diff(a: &RgbImage, b: &RgbImage) -> f32 {
        let s: u64 = a.as_raw().iter().zip(b.as_raw()).map(|(x, y)| (*x as i32 - *y as i32).unsigned_abs() as u64).sum();
        s as f32 / a.as_raw().len() as f32
    }

    #[test]
    fn hsl_fit_recovers_selective_colour() {
        let bases: Vec<_> = (0..3).map(colourful).collect();
        let edits: Vec<_> = bases.iter().map(selective_grade).collect();
        let (b, e) = (stats(&bases), stats(&edits));
        let mut px = PairedPixels::new();
        for (o, ed) in bases.iter().zip(&edits) {
            assert!(px.add_pair(ed, o));
        }
        let curves_only = StylePreset::fit_paired("x", &e, &b, &PairedPixels::new(), 1.0).unwrap();
        let full = StylePreset::fit_paired("x", &e, &b, &px, 1.0).unwrap();
        assert!(full.has_hsl(), "{:?}", full.hsl);
        let err = |p: &StylePreset| -> f32 {
            bases.iter().zip(&edits).map(|(o, ed)| mean_abs_diff(&p.apply_preview(o), &ed.to_rgb8())).sum()
        };
        let (e0, e1) = (err(&curves_only), err(&full));
        assert!(e1 < 0.6 * e0, "pixel error {e0:.2} → {e1:.2}");
        let [_, orange, _, green, _, blue, _, _] = full.hsl;
        assert!(green.saturation <= -20, "greens desaturated: {:?}", full.hsl);
        assert!(blue.luminance <= -5, "blues darkened: {:?}", full.hsl);
        assert!(orange.hue >= 5, "oranges toward yellow: {:?}", full.hsl);
        let half = StylePreset::fit_paired("x", &e, &b, &px, 0.5).unwrap();
        assert!((half.hsl[3].saturation - green.saturation / 2).abs() <= 1, "strength scales HSL");
    }

    #[test]
    fn unedited_pairs_fit_no_hsl_or_grading() {
        let imgs: Vec<_> = (0..2).map(colourful).collect();
        let mut px = PairedPixels::new();
        imgs.iter().for_each(|i| assert!(px.add_pair(i, i)));
        let s = stats(&imgs);
        let p = StylePreset::fit_paired("x", &s, &s, &px, 1.0).unwrap();
        assert!(!p.has_hsl(), "{:?}", p.hsl);
        assert!(p.color_grade.is_neutral(), "{:?}", p.color_grade);
    }

    /// A warm-midtone tint on top of a known grade is picked up by the
    /// grading wheels, at roughly the right hue.
    #[test]
    fn grading_picks_up_a_tint_the_curves_leave() {
        let tint = |img: &DynamicImage| {
            let mut o = img.to_rgb8();
            for p in o.pixels_mut() {
                let [r, g, b] = p.0.map(|c| c as f32 / 255.0);
                let y = luma(r, g, b);
                let w = 4.0 * y * (1.0 - y);
                // Orange-ish (hue ≈ 30°) tint, strongest in the midtones.
                p.0 = [r + 0.06 * w, g + 0.0 * w, b - 0.06 * w].map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8);
            }
            DynamicImage::ImageRgb8(o)
        };
        let bases: Vec<_> = (0..3).map(colourful).collect();
        let edits: Vec<_> = bases.iter().map(tint).collect();
        let mut px = PairedPixels::new();
        bases.iter().zip(&edits).for_each(|(o, e)| assert!(px.add_pair(e, o)));
        let curves_only = StylePreset::fit_paired("x", &stats(&edits), &stats(&bases), &PairedPixels::new(), 1.0).unwrap();
        let p = StylePreset::fit_paired("x", &stats(&edits), &stats(&bases), &px, 1.0).unwrap();
        let err = |p: &StylePreset| -> f32 {
            bases.iter().zip(&edits).map(|(o, ed)| mean_abs_diff(&p.apply_preview(o), &ed.to_rgb8())).sum()
        };
        let (e0, e1) = (err(&curves_only), err(&p));
        println!("grading: {e0:.3} → {e1:.3} {:?}", p.color_grade);
        assert!(e1 < 0.8 * e0, "pixel error {e0:.3} → {e1:.3}");
        // Per-channel curves spread a midtone tint over every tone, so the
        // wheels add it in the midtones and take some back at the ends.
        let mid = p.color_grade.midtones;
        assert!(mid.saturation >= 10 && hue_delta(30.0, mid.hue as f32).abs() < 30.0, "{:?}", p.color_grade);
    }

    #[test]
    fn misaligned_pairs_are_rejected() {
        let o = colourful(1);
        let mut px = PairedPixels::new();
        assert!(px.add_pair(&grade(&o), &o), "an edit of the same frame lines up");
        assert!(px.add_pair(&o.resize(240, 160, image::imageops::FilterType::Triangle), &o), "resized export");
        assert!(!px.add_pair(&o.crop_imm(0, 0, 360, 320), &o), "different aspect");
        assert!(!px.add_pair(&o.fliph(), &o), "mirrored");
        let shifted = o.crop_imm(60, 40, 360, 240).resize_exact(480, 320, image::imageops::FilterType::Triangle);
        assert!(!px.add_pair(&shifted, &o), "same aspect but cropped in");
        assert_eq!(px.pair_count(), 2);
    }

    #[test]
    fn hsl_model_moves_hues_the_lr_way() {
        let mut hsl = [HslAdjust::default(); 8];
        hsl[0].hue = 100; // red toward orange
        let (_, a, b) = to_plane(apply_hsl(&hsl, [0.8, 0.2, 0.2]));
        let h = hue_deg(a, b);
        assert!(h > 20.0 && h < 40.0, "red +100 → {h}");
        hsl[0] = HslAdjust { hue: 0, saturation: -100, luminance: 0 };
        let g = apply_hsl(&hsl, [0.8, 0.2, 0.2]);
        assert!((g[0] - g[1]).abs() < 1e-4 && (g[1] - g[2]).abs() < 1e-4, "red −100 sat → gray {g:?}");
        assert_eq!(apply_hsl(&hsl, [0.5, 0.5, 0.5]), [0.5, 0.5, 0.5], "neutrals untouched");
        let t = wheel_tint(GradeWheel { hue: 60, saturation: 100 });
        assert!(t[0] > 0.0 && t[1] > 0.0 && t[2] < 0.0, "hue 60 tints yellow: {t:?}");
    }

    fn selective_preset() -> StylePreset {
        let mut p = StylePreset::fit_paired("Sel", &stats(&[scene(1)]), &stats(&[scene(1)]), &PairedPixels::new(), 1.0)
            .unwrap();
        p.hsl[3] = HslAdjust { hue: 20, saturation: -40, luminance: 10 };
        p.hsl[5] = HslAdjust { hue: -10, saturation: 15, luminance: -30 };
        p.color_grade.shadows = GradeWheel { hue: 200, saturation: 25 };
        p.color_grade.highlights = GradeWheel { hue: 40, saturation: 20 };
        p
    }

    #[test]
    fn selective_settings_reach_the_xmp_and_the_lut() {
        let p = selective_preset();
        let x = p.to_lr_preset_xmp();
        for needle in [
            r#"crs:SaturationAdjustmentGreen="-40""#,
            r#"crs:HueAdjustmentGreen="+20""#,
            r#"crs:LuminanceAdjustmentBlue="-30""#,
            r#"crs:HueAdjustmentRed="0""#,
            r#"crs:SplitToningShadowHue="200""#,
            r#"crs:SplitToningShadowSaturation="25""#,
            r#"crs:SplitToningHighlightHue="40""#,
            r#"crs:ColorGradeMidtoneSat="0""#,
            r#"crs:ColorGradeBlending="50""#,
            "HSL, color grading learned",
        ] {
            assert!(x.contains(needle), "missing {needle}");
        }
        let (n, vals) = parse_cube(&p.to_cube_lut(33));
        let mut worst = 0.0f32;
        for i in 0..800u32 {
            let rgb = [(i * 37 % 256) as f32 / 255.0, (i * 91 % 256) as f32 / 255.0, (i * 53 % 256) as f32 / 255.0];
            let (d, l) = (p.apply_rgb(rgb), cube_lookup(&vals, n, rgb));
            for c in 0..3 {
                worst = worst.max((d[c] - l[c]).abs());
            }
        }
        assert!(worst < 3.0 / 255.0, "LUT deviates by {worst:.4}");

        // Untouched sections stay out, so applying doesn't reset the user's own.
        let plain = StylePreset::fit_paired("x", &stats(&[scene(1)]), &stats(&[scene(1)]), &PairedPixels::new(), 1.0)
            .unwrap()
            .to_lr_preset_xmp();
        assert!(!plain.contains("HueAdjustment") && !plain.contains("SplitToning") && !plain.contains("ColorGrade"));
    }

    /// Presets serialised before HSL / grading existed still deserialise.
    #[test]
    fn old_presets_deserialise() {
        let json = r#"{"name":"x","tone_curve":[[0,0],[255,255]],"red_curve":[[0,0],[255,255]],
            "green_curve":[[0,0],[255,255]],"blue_curve":[[0,0],[255,255]],"saturation":5}"#;
        let p: StylePreset = serde_json::from_str(json).unwrap();
        assert!(!p.has_hsl() && p.color_grade.is_neutral());
    }

    #[test]
    fn preset_xmp_has_lr_preset_fields() {
        let mut p = StylePreset::fit_paired("x", &stats(&[grade(&scene(1))]), &stats(&[scene(1)]), &PairedPixels::new(), 1.0)
            .unwrap();
        p.name = "Warm <film> & teal".into();
        let x = p.to_lr_preset_xmp();
        for needle in [
            r#"xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/""#,
            r#"crs:PresetType="Normal""#,
            r#"crs:HasSettings="True""#,
            r#"crs:ToneCurveName2012="Custom""#,
            "<crs:ToneCurvePV2012>",
            "<crs:ToneCurvePV2012Red>",
            "<crs:ToneCurvePV2012Green>",
            "<crs:ToneCurvePV2012Blue>",
            "Warm &lt;film&gt; &amp; teal",
            "<rdf:li>0, ",
            "<rdf:li>255, ",
        ] {
            assert!(x.contains(needle), "missing {needle}");
        }
        assert!(!x.contains("ProcessVersion"), "must not force a process version");
        assert_eq!(x.matches("<rdf:Seq>").count(), 4);
        assert_eq!(p.render(StyleFormat::LrPreset).file_name, "Warm _film_ _ teal.xmp");
    }
}
