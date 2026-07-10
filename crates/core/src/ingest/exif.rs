use super::DriveMode;
use chrono::{DateTime, NaiveDateTime, Utc};
use std::fs::File;
use std::io::BufReader;
use std::path::Path;

#[derive(Debug, Default, Clone)]
pub struct ExifInfo {
    pub captured_at: Option<DateTime<Utc>>,
    pub burst_id: Option<String>,
    pub drive_mode: Option<DriveMode>,
    pub iso: Option<u32>,
    pub exposure_bias_ev: Option<f32>,
}

pub fn extract_exif_info(path: &Path) -> Result<ExifInfo, exif::Error> {
    let file = File::open(path).map_err(exif::Error::Io)?;
    let mut reader = BufReader::new(file);
    let exif_reader = exif::Reader::new();
    let exif_data = exif_reader.read_from_container(&mut reader)?;

    let captured_at = read_datetime(&exif_data);
    let burst_id = read_burst_id(&exif_data);
    let drive_mode = read_drive_mode(&exif_data);
    let iso = read_iso(&exif_data);
    let exposure_bias_ev = read_exposure_bias(&exif_data);

    Ok(ExifInfo { captured_at, burst_id, drive_mode, iso, exposure_bias_ev })
}

/// Fallback for RAW containers kamadak-exif can't walk — CR3 is ISO BMFF,
/// RAF has a proprietary header, so `read_from_container` errors on both and
/// their photos would land with no timestamp (breaking Stage A time
/// clustering: every CR3 becomes a singleton group). rawler's per-vendor
/// metadata decoders read the same fields from anything rawler can open.
pub fn extract_exif_info_via_rawler(path: &Path) -> Option<ExifInfo> {
    let meta = rawler_metadata(path)?;
    let exif = &meta.exif;
    let captured_at = exif
        .date_time_original
        .as_deref()
        .or(exif.create_date.as_deref())
        .and_then(parse_exif_datetime_string);
    let iso = exif
        .iso_speed
        .or_else(|| exif.iso_speed_ratings.map(u32::from));
    let exposure_bias_ev = exif
        .exposure_bias
        .filter(|r| r.d != 0)
        .map(|r| r.n as f32 / r.d as f32);
    Some(ExifInfo {
        captured_at,
        burst_id: None,
        drive_mode: None,
        iso,
        exposure_bias_ev,
    })
}

fn rawler_metadata(path: &Path) -> Option<rawler::decoders::RawMetadata> {
    let source = rawler::rawsource::RawSource::new(path).ok()?;
    let decoder = rawler::get_decoder(&source).ok()?;
    decoder
        .raw_metadata(&source, &rawler::decoders::RawDecodeParams::default())
        .ok()
}

/// Parse the EXIF-standard "YYYY:MM:DD HH:MM:SS" datetime string rawler
/// hands back verbatim from the file.
fn parse_exif_datetime_string(s: &str) -> Option<DateTime<Utc>> {
    NaiveDateTime::parse_from_str(s.trim(), "%Y:%m:%d %H:%M:%S")
        .ok()
        .map(|n| n.and_utc())
}

/// EXIF orientation (1–8) from the primary IFD; defaults to 1 (no transform)
/// when absent or unreadable. Read at decode time so portrait-orientation shots
/// are uprighted before scoring/face detection/display (the `image` crate does
/// not auto-apply orientation on decode).
///
/// RAW containers kamadak can't walk (CR3/RAF) fall back to rawler's
/// metadata decoder — otherwise portrait CR3s would render sideways.
pub fn read_orientation(path: &Path) -> u16 {
    use exif::{In, Tag, Value};
    let via_kamadak = (|| {
        let file = File::open(path).ok()?;
        let mut reader = BufReader::new(file);
        let data = exif::Reader::new().read_from_container(&mut reader).ok()?;
        match data.get_field(Tag::Orientation, In::PRIMARY).map(|f| &f.value) {
            Some(Value::Short(v)) => v.first().copied().filter(|o| (1..=8).contains(o)),
            _ => None,
        }
    })();
    if let Some(o) = via_kamadak {
        return o;
    }
    // Only pay a rawler open for files that are actually RAW.
    if matches!(super::classify_extension(path), Some(super::ImageFormat::Raw(_))) {
        if let Some(o) = rawler_metadata(path)
            .and_then(|m| m.exif.orientation)
            .filter(|o| (1..=8).contains(o))
        {
            return o;
        }
    }
    1
}

fn read_iso(data: &exif::Exif) -> Option<u32> {
    use exif::{In, Tag, Value};
    // ISOSpeedRatings (deprecated) and PhotographicSensitivity both stash ISO.
    let f = data
        .get_field(Tag::PhotographicSensitivity, In::PRIMARY)
        .or_else(|| data.get_field(Tag::ISOSpeed, In::PRIMARY))?;
    match &f.value {
        Value::Short(v) => v.first().map(|x| *x as u32),
        Value::Long(v) => v.first().copied(),
        _ => None,
    }
}

fn read_exposure_bias(data: &exif::Exif) -> Option<f32> {
    use exif::{In, Tag, Value};
    let f = data.get_field(Tag::ExposureBiasValue, In::PRIMARY)?;
    match &f.value {
        Value::SRational(v) => v.first().map(|r| r.num as f32 / r.denom as f32),
        _ => None,
    }
}

fn read_datetime(data: &exif::Exif) -> Option<DateTime<Utc>> {
    use exif::{In, Tag};
    let field = data
        .get_field(Tag::DateTimeOriginal, In::PRIMARY)
        .or_else(|| data.get_field(Tag::DateTimeDigitized, In::PRIMARY))
        .or_else(|| data.get_field(Tag::DateTime, In::PRIMARY))?;

    let s = field.display_value().with_unit(data).to_string();
    let naive = NaiveDateTime::parse_from_str(&s, "%Y-%m-%d %H:%M:%S").ok()?;

    let subsec_ms: u32 = data
        .get_field(Tag::SubSecTimeOriginal, In::PRIMARY)
        .or_else(|| data.get_field(Tag::SubSecTime, In::PRIMARY))
        .and_then(|f| {
            let v = f.display_value().with_unit(data).to_string();
            v.trim().trim_matches('"').parse::<u32>().ok()
        })
        .map(|n| {
            // SubSecTime is fractional seconds with variable precision: "12" = 120ms.
            let s = n.to_string();
            let padded = format!("{:0<3}", s); // pad right to 3 digits
            padded[..3].parse::<u32>().unwrap_or(0)
        })
        .unwrap_or(0);

    let with_ms = naive.and_utc() + chrono::Duration::milliseconds(subsec_ms as i64);
    Some(with_ms)
}

fn read_burst_id(data: &exif::Exif) -> Option<String> {
    // Apple/Sony/Canon stash burst identifiers in vendor-specific MakerNote tags.
    // kamadak-exif doesn't decode MakerNote payloads, so this returns None for now.
    // M2/M3 can add per-vendor handling.
    let _ = data;
    None
}

fn read_drive_mode(data: &exif::Exif) -> Option<DriveMode> {
    use exif::{In, Tag};
    // Tag::CustomRendered is too generic; vendor drive-mode tags live in MakerNote.
    // For M1 we only flag DriveMode when EXIF makes it trivially explicit.
    let _ = (data.get_field(Tag::CustomRendered, In::PRIMARY),);
    None
}
