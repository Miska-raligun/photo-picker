//! SQLite-backed feature cache (M4.1).
//!
//! Caches `PhotoFeatures` by the file's SHA-256 (16-byte prefix). On a re-run
//! of the same directory, photos whose content hasn't changed are pulled from
//! the cache instead of re-running decode + hash + tech scoring + CLIP — turning
//! parameter tuning (--k1, --k2, thresholds) into a near-instant operation.
//!
//! The cache file is portable: it's pure SQLite with a versioned schema, no
//! foreign-host data, no embedded paths beyond hints. Delete it to rebuild.

use crate::error::{Error, Result};
use crate::features::PhotoFeatures;
use crate::ingest::PhotoId;
use crate::scoring::FaceInfo;
use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;

/// Bumped when the on-disk feature representation changes incompatibly. Older
/// rows are ignored (and silently re-extracted on next run).
/// - v1: M4.1 initial (technical scores + CLIP embed + face stub data)
/// - v2: M3.5-real YuNet face detection wired in — old rows have empty face data
/// - v3: real composition + aesthetic heuristics replace 0.5 stubs
pub const FEATURE_SCHEMA_VERSION: i64 = 3;

pub struct CacheStore {
    conn: Connection,
}

impl CacheStore {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p)
                .map_err(|e| Error::Io { path: p.to_path_buf(), source: e })?;
        }
        let conn = Connection::open(path)
            .map_err(|e| Error::Config(format!("cache open {}: {e}", path.display())))?;
        // Pragmas for fast-and-safe-enough single-writer access.
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA temp_store = MEMORY;",
        )
        .map_err(|e| Error::Config(format!("cache pragmas: {e}")))?;
        Self::init_schema(&conn)?;
        Ok(Self { conn })
    }

    fn init_schema(conn: &Connection) -> Result<()> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS features (
                sha256_short    TEXT    PRIMARY KEY,
                schema_version  INTEGER NOT NULL,
                phash           INTEGER NOT NULL,
                dhash           INTEGER NOT NULL,
                exposure        REAL,
                wb              REAL,
                sharpness_raw   REAL,
                noise           REAL,
                aesthetic       REAL,
                composition     REAL,
                clip_embed      BLOB,
                face_info_json  TEXT,
                created_at      INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS meta (
                key   TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );",
        )
        .map_err(|e| Error::Config(format!("cache schema: {e}")))?;
        Ok(())
    }

    /// Look up cached features for a content hash. The returned PhotoFeatures
    /// has `photo_id` set to the caller-provided id (cache is content-keyed,
    /// not id-keyed).
    pub fn get(&self, sha256_short: &[u8; 16], for_id: PhotoId) -> Result<Option<PhotoFeatures>> {
        let key = hex::encode(sha256_short);
        let mut stmt = self
            .conn
            .prepare_cached(
                "SELECT phash, dhash, exposure, wb, sharpness_raw, noise,
                        aesthetic, composition, clip_embed, face_info_json
                 FROM features
                 WHERE sha256_short = ?1 AND schema_version = ?2",
            )
            .map_err(|e| Error::Config(format!("cache get prepare: {e}")))?;

        let row = stmt
            .query_row(params![key, FEATURE_SCHEMA_VERSION], |r| {
                Ok((
                    r.get::<_, i64>(0)? as u64,         // phash
                    r.get::<_, i64>(1)? as u64,         // dhash
                    r.get::<_, Option<f64>>(2)?.map(|v| v as f32),
                    r.get::<_, Option<f64>>(3)?.map(|v| v as f32),
                    r.get::<_, Option<f64>>(4)?.map(|v| v as f32),
                    r.get::<_, Option<f64>>(5)?.map(|v| v as f32),
                    r.get::<_, Option<f64>>(6)?.map(|v| v as f32),
                    r.get::<_, Option<f64>>(7)?.map(|v| v as f32),
                    r.get::<_, Option<Vec<u8>>>(8)?,
                    r.get::<_, Option<String>>(9)?,
                ))
            })
            .optional()
            .map_err(|e| Error::Config(format!("cache get row: {e}")))?;

        let Some((phash, dhash, exposure, wb, sharpness_raw, noise, aesthetic, composition, clip_bytes, face_json)) = row
        else {
            return Ok(None);
        };

        let clip_embed = clip_bytes.map(|b| bytes_to_f32_vec(&b));
        let face = face_json
            .as_deref()
            .and_then(|s| serde_json::from_str::<FaceInfo>(s).ok());

        Ok(Some(PhotoFeatures {
            photo_id: for_id,
            phash,
            dhash,
            exposure,
            wb,
            sharpness_raw,
            noise,
            aesthetic,
            composition,
            face,
            clip_embed,
        }))
    }

    pub fn put(&self, sha256_short: &[u8; 16], features: &PhotoFeatures) -> Result<()> {
        let key = hex::encode(sha256_short);
        let clip_bytes: Option<Vec<u8>> = features.clip_embed.as_ref().map(|v| f32_vec_to_bytes(v));
        let face_json: Option<String> = features
            .face
            .as_ref()
            .map(|f| serde_json::to_string(f).unwrap_or_default());
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);

        self.conn
            .execute(
                "INSERT INTO features (
                    sha256_short, schema_version, phash, dhash,
                    exposure, wb, sharpness_raw, noise,
                    aesthetic, composition, clip_embed, face_info_json, created_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
                 ON CONFLICT(sha256_short) DO UPDATE SET
                    schema_version = excluded.schema_version,
                    phash = excluded.phash,
                    dhash = excluded.dhash,
                    exposure = excluded.exposure,
                    wb = excluded.wb,
                    sharpness_raw = excluded.sharpness_raw,
                    noise = excluded.noise,
                    aesthetic = excluded.aesthetic,
                    composition = excluded.composition,
                    clip_embed = excluded.clip_embed,
                    face_info_json = excluded.face_info_json,
                    created_at = excluded.created_at",
                params![
                    key,
                    FEATURE_SCHEMA_VERSION,
                    features.phash as i64,
                    features.dhash as i64,
                    features.exposure.map(|v| v as f64),
                    features.wb.map(|v| v as f64),
                    features.sharpness_raw.map(|v| v as f64),
                    features.noise.map(|v| v as f64),
                    features.aesthetic.map(|v| v as f64),
                    features.composition.map(|v| v as f64),
                    clip_bytes,
                    face_json,
                    now_ms,
                ],
            )
            .map_err(|e| Error::Config(format!("cache put: {e}")))?;
        Ok(())
    }

    pub fn row_count(&self) -> Result<i64> {
        self.conn
            .query_row("SELECT COUNT(*) FROM features", [], |r| r.get(0))
            .map_err(|e| Error::Config(format!("cache count: {e}")))
    }
}

fn f32_vec_to_bytes(v: &[f32]) -> Vec<u8> {
    bytemuck::cast_slice(v).to_vec()
}

fn bytes_to_f32_vec(b: &[u8]) -> Vec<f32> {
    let n = b.len() / 4;
    let mut out = vec![0.0_f32; n];
    let dst: &mut [u8] = bytemuck::cast_slice_mut(&mut out);
    dst.copy_from_slice(&b[..n * 4]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn roundtrip_features() {
        let dir = tempdir().unwrap();
        let cache = CacheStore::open(&dir.path().join("cache.db")).unwrap();
        let id = PhotoId::new();
        let key = [0xAB; 16];
        let feat = PhotoFeatures {
            photo_id: id,
            phash: 0xCAFEBABE_DEADBEEF,
            dhash: 0x12345678_9ABCDEF0,
            exposure: Some(0.7),
            wb: Some(0.9),
            sharpness_raw: Some(123.4),
            noise: Some(0.8),
            aesthetic: Some(0.5),
            composition: Some(0.6),
            face: None,
            clip_embed: Some(vec![0.1, 0.2, 0.3]),
        };
        cache.put(&key, &feat).unwrap();

        let other_id = PhotoId::new();
        let got = cache.get(&key, other_id).unwrap().unwrap();
        assert_eq!(got.photo_id, other_id, "cache returns features keyed to caller's photo id");
        assert_eq!(got.phash, feat.phash);
        assert_eq!(got.dhash, feat.dhash);
        assert_eq!(got.exposure, feat.exposure);
        assert_eq!(got.clip_embed.as_ref().unwrap().len(), 3);
        assert!((got.clip_embed.as_ref().unwrap()[1] - 0.2).abs() < 1e-6);
    }

    #[test]
    fn missing_key_returns_none() {
        let dir = tempdir().unwrap();
        let cache = CacheStore::open(&dir.path().join("c.db")).unwrap();
        let got = cache.get(&[0; 16], PhotoId::new()).unwrap();
        assert!(got.is_none());
    }
}
