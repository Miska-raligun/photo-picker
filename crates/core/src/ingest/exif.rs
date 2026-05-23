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
