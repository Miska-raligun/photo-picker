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
//!
//! The preset contains only these settings, so applying it leaves exposure,
//! white balance and everything else untouched.
//!
//! Two fitting modes, because what can be learned depends on what we can see:
//!
//! - **Paired** ([`StylePreset::fit_paired`]): each reference is an edited
//!   version of a photo we also have unedited (the user graded a few shots of
//!   a shoot). Same content on both sides, so the full transform is
//!   identifiable: luma and per-channel quantile matching plus the residual
//!   saturation change. On test grades this cuts the distance to the true
//!   result by roughly two thirds.
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
}

impl StylePreset {
    /// Paired fit: `edited[i]` is an edited version of `originals[i]` (pooled
    /// into the two stats). `strength` in `[0, 1]` scales every adjustment.
    pub fn fit_paired(
        name: &str,
        edited: &StyleStats,
        originals: &StyleStats,
        strength: f32,
    ) -> Result<Self, StyleError> {
        check_inputs(edited, originals)?;
        let s = strength.clamp(0.0, 1.0);

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
        };
        preset.saturation = residual_saturation(&preset, edited, originals, s);
        Ok(preset)
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
            // transferring it hurt in every unpaired test.
            saturation: 0,
        })
    }

    /// Approximate what LR will do: master curve, then the channel curves,
    /// then saturation around luma. For on-screen before/after previews only.
    pub fn apply_preview(&self, img: &DynamicImage) -> RgbImage {
        let luts = self.luts();
        let k = 1.0 + self.saturation as f32 / 100.0;
        let mut out = img.to_rgb8();
        for p in out.pixels_mut() {
            let [r, g, b] = luts.apply_curves(p.0).map(|v| v as f32);
            let y = luma(r, g, b);
            p.0 = [r, g, b].map(|c| (y + (c - y) * k).round().clamp(0.0, 255.0) as u8);
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
        let saturation =
            if self.saturation > 0 { format!("+{}", self.saturation) } else { self.saturation.to_string() };
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
   crs:ToneCurveName2012="Custom"
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
            desc = alt("Tone curve, RGB curves and saturation learned from reference photos"),
            master = seq(&self.tone_curve),
            red = seq(&self.red_curve),
            green = seq(&self.green_curve),
            blue = seq(&self.blue_curve),
        )
    }

    /// A file name for the preset derived from its name.
    pub fn file_name(&self) -> String {
        let stem: String = self
            .name
            .chars()
            .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' || c == ' ' { c } else { '_' })
            .collect::<String>()
            .trim()
            .to_string();
        format!("{}.xmp", if stem.is_empty() { "photo-pick style" } else { &stem })
    }
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
    let luts = preset.luts();
    let after = if source.kept.is_empty() {
        source.mean_chroma()
    } else {
        source
            .kept
            .iter()
            .map(|px| {
                let o = luts.apply_curves(*px);
                (o[0].max(o[1]).max(o[2]) - o[0].min(o[1]).min(o[2])) as f32 / 255.0
            })
            .sum::<f32>()
            / source.kept.len() as f32
    };
    if after <= 1e-3 {
        return 0;
    }
    let ratio = target.mean_chroma() / after;
    ((ratio - 1.0) * 100.0 * s).round().clamp(-SATURATION_RANGE as f32, SATURATION_RANGE as f32) as i32
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

/// Monotone cubic (Fritsch–Carlson) interpolation of a point curve into a LUT.
fn curve_lut(pts: &[(u8, u8)]) -> [u8; 256] {
    let mut lut = [0u8; 256];
    if pts.len() < 2 {
        for (i, v) in lut.iter_mut().enumerate() {
            *v = i as u8;
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
        *v = y.round().clamp(0.0, 255.0) as u8;
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
        let p = StylePreset::fit_paired("x", &s, &s, 1.0).unwrap();
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
            StylePreset::fit_paired("x", &reference, &base, 0.0).unwrap(),
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
        let p = StylePreset::fit_paired("x", &r, &b, 1.0).unwrap();
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
            StylePreset::fit_paired("x", &r, &b, 1.0).unwrap(),
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
        assert_eq!(StylePreset::fit_paired("x", &e, &s, 1.0), Err(StyleError::NoReferences));
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

    #[test]
    fn preset_xmp_has_lr_preset_fields() {
        let mut p = StylePreset::fit_paired("x", &stats(&[grade(&scene(1))]), &stats(&[scene(1)]), 1.0)
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
        assert_eq!(p.file_name(), "Warm _film_ _ teal.xmp");
    }
}
