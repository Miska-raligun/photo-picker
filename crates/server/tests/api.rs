//! Handler integration tests. These exercise the HTTP surface against a real
//! router + `AppState` with `tower::ServiceExt::oneshot` — no TCP, no ONNX.
//! Everything here must compile and pass with `--no-default-features` so the
//! suite runs in environments without the onnxruntime static lib.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use photo_pick_core::ingest::{ImageFormat, PhotoId, PhotoRef};
use photo_pick_server::state::{RunRecord, RunStatus};
use photo_pick_server::{router, AppState};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tower::ServiceExt;

/// AppState whose runs.json path points into a tempdir so tests never write
/// into the developer's real XDG data dir.
fn test_state(tmp: &Path) -> AppState {
    let mut state = AppState::new();
    state.runs_index_path = Some(std::sync::Arc::new(tmp.join("runs.json")));
    state
}

fn photo_ref(path: PathBuf) -> (PhotoId, PhotoRef) {
    let id = PhotoId::new();
    (
        id,
        PhotoRef {
            id,
            path,
            format: ImageFormat::Jpeg,
            captured_at: None,
            file_size: 3,
            sha256_short: [0u8; 16],
            burst_id: None,
            drive_mode: None,
            iso: None,
            exposure_bias_ev: None,
        },
    )
}

/// Insert a completed in-place run over `root` with the given photos.
async fn insert_run(state: &AppState, root: PathBuf, output: PathBuf, photos: Vec<(PhotoId, PhotoRef)>) -> String {
    let run_id = uuid::Uuid::new_v4().to_string();
    let rec = RunRecord {
        id: run_id.clone(),
        root,
        output,
        in_place: true,
        status: RunStatus::Completed,
        report: None,
        html_report: None,
        composition_picks: vec![],
        photos: photos.into_iter().collect::<HashMap<_, _>>(),
        explanations: HashMap::new(),
    };
    state.runs.lock().await.insert(run_id.clone(), rec);
    run_id
}

async fn post_json(app: &axum::Router, uri: &str, body: serde_json::Value) -> (StatusCode, serde_json::Value) {
    let resp = app
        .clone()
        .oneshot(
            Request::post(uri)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, json)
}

/// PHOTO_PICK_BROWSE_ROOTS is process-global and Rust runs tests
/// concurrently, so every test that sets it holds this lock for its duration.
static ROOTS_ENV: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[tokio::test]
async fn browse_and_reveal_respect_roots_allowlist() {
    let _env = ROOTS_ENV.lock().await;
    let allowed = tempfile::tempdir().unwrap();
    let denied = tempfile::tempdir().unwrap();
    std::env::set_var("PHOTO_PICK_BROWSE_ROOTS", allowed.path());

    let tmp = tempfile::tempdir().unwrap();
    let app = router(test_state(tmp.path()));

    // Inside the allowlist → 200.
    let uri = format!("/api/browse?path={}", allowed.path().display());
    let resp = app.clone().oneshot(Request::get(&uri).body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Outside → 403.
    let uri = format!("/api/browse?path={}", denied.path().display());
    let resp = app.clone().oneshot(Request::get(&uri).body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    // /api/reveal is gated by the same allowlist. An outside path must be
    // rejected BEFORE any file-manager process is spawned.
    let secret = denied.path().join("secret.txt");
    std::fs::write(&secret, b"x").unwrap();
    let uri = format!("/api/reveal?path={}", secret.display());
    let resp = app.clone().oneshot(Request::get(&uri).body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    std::env::remove_var("PHOTO_PICK_BROWSE_ROOTS");
}

#[tokio::test]
async fn apply_dry_run_previews_without_deleting() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("shoot");
    let output = tmp.path().join("out");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&output).unwrap();

    let a = root.join("a.jpg");
    let b = root.join("b.jpg");
    std::fs::write(&a, b"aaa").unwrap();
    std::fs::write(&b, b"bbb").unwrap();
    let (ida, pa) = photo_ref(a.clone());
    let (idb, pb) = photo_ref(b.clone());

    let state = test_state(tmp.path());
    let run_id = insert_run(&state, root, output.clone(), vec![(ida, pa), (idb, pb)]).await;
    let app = router(state);

    let (status, json) = post_json(
        &app,
        &format!("/api/runs/{run_id}/apply"),
        serde_json::json!({
            "delete_ids": [ida.to_string(), idb.to_string()],
            "use_trash": false,
            "dry_run": true,
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["dry_run"], true);
    assert_eq!(json["deleted"], 0);
    assert_eq!(json["would_delete"].as_array().unwrap().len(), 2);
    assert!(json["manifest_path"].is_null(), "dry runs must not write a manifest");
    assert!(a.exists() && b.exists(), "dry run must not touch files");
    let manifests: Vec<_> = std::fs::read_dir(&output)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().starts_with("deleted-"))
        .collect();
    assert!(manifests.is_empty());
}

#[cfg(unix)]
#[tokio::test]
async fn apply_refuses_symlink_escape_and_writes_manifest_for_the_rest() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("shoot");
    let output = tmp.path().join("out");
    let outside = tmp.path().join("elsewhere");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&output).unwrap();
    std::fs::create_dir_all(&outside).unwrap();

    // One honest file inside the run root and one symlink whose target
    // escapes it (the attacker-planted-link scenario).
    let honest = root.join("honest.jpg");
    std::fs::write(&honest, b"jpg").unwrap();
    let target = outside.join("victim.jpg");
    std::fs::write(&target, b"do-not-delete").unwrap();
    let link = root.join("trap.jpg");
    std::os::unix::fs::symlink(&target, &link).unwrap();

    let (id_honest, p_honest) = photo_ref(honest.clone());
    let (id_trap, p_trap) = photo_ref(link.clone());

    let state = test_state(tmp.path());
    let run_id = insert_run(&state, root, output.clone(), vec![(id_honest, p_honest), (id_trap, p_trap)]).await;
    let app = router(state);

    let (status, json) = post_json(
        &app,
        &format!("/api/runs/{run_id}/apply"),
        serde_json::json!({
            "delete_ids": [id_honest.to_string(), id_trap.to_string()],
            "use_trash": false,
            "dry_run": false,
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["deleted"], 1);
    assert!(!honest.exists(), "the honest file should be deleted");
    assert!(target.exists(), "the symlink target outside the root must survive");
    let failed = json["failed"].as_array().unwrap();
    assert_eq!(failed.len(), 1);
    assert_eq!(failed[0]["photo_id"], id_trap.to_string());

    // deleted_ids drives the UI's selective override cleanup.
    let deleted_ids = json["deleted_ids"].as_array().unwrap();
    assert_eq!(deleted_ids.len(), 1);
    assert_eq!(deleted_ids[0], id_honest.to_string());

    // The audit manifest exists and lists exactly the deleted photo.
    let manifest_path = PathBuf::from(json["manifest_path"].as_str().expect("manifest written"));
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
    assert_eq!(manifest["mode"], "delete");
    let items = manifest["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["photo_id"], id_honest.to_string());
}

#[tokio::test]
async fn cancel_reports_unknown_and_not_running() {
    let tmp = tempfile::tempdir().unwrap();
    let state = test_state(tmp.path());
    let run_id = insert_run(&state, tmp.path().to_path_buf(), tmp.path().to_path_buf(), vec![]).await;
    let app = router(state);

    // Unknown id → 404.
    let (status, _) = post_json(&app, "/api/runs/does-not-exist/cancel", serde_json::json!({})).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Known but terminal (no live cancel flag) → 409.
    let (status, _) = post_json(&app, &format!("/api/runs/{run_id}/cancel"), serde_json::json!({})).await;
    assert_eq!(status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn token_gate_locks_api_but_not_static_or_health() {
    let tmp = tempfile::tempdir().unwrap();
    let mut state = test_state(tmp.path());
    state.api_token = Some(std::sync::Arc::from("s3cret"));
    let app = router(state);

    // API without a token → 401.
    let resp = app.clone().oneshot(Request::get("/api/runs").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    // Wrong token → 401.
    let resp = app.clone().oneshot(
        Request::get("/api/runs").header("authorization", "Bearer nope").body(Body::empty()).unwrap(),
    ).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    // Bearer header → 200.
    let resp = app.clone().oneshot(
        Request::get("/api/runs").header("authorization", "Bearer s3cret").body(Body::empty()).unwrap(),
    ).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    // Cookie (what <img> thumbs / EventSource rely on) → 200.
    let resp = app.clone().oneshot(
        Request::get("/api/runs").header("cookie", "other=1; photo_pick_token=s3cret").body(Body::empty()).unwrap(),
    ).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    // Query param → 200.
    let resp = app.clone().oneshot(Request::get("/api/runs?token=s3cret").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    // Static shell loads without a token (user needs the UI to enter it)…
    let resp = app.clone().oneshot(Request::get("/").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    // …and so does the health probe.
    let resp = app.clone().oneshot(Request::get("/api/health").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn export_writes_xmp_sidecars_with_rating_and_note() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("shoot");
    let target = tmp.path().join("exported");
    std::fs::create_dir_all(&root).unwrap();

    let keeper = root.join("keeper.jpg");
    let fav = root.join("fav.jpg");
    std::fs::write(&keeper, b"k").unwrap();
    std::fs::write(&fav, b"f").unwrap();
    let (id_keep, p_keep) = photo_ref(keeper);
    let (id_fav, p_fav) = photo_ref(fav);

    let state = test_state(tmp.path());
    let run_id = insert_run(&state, root, tmp.path().join("out"), vec![(id_keep, p_keep), (id_fav, p_fav)]).await;
    let app = router(state);

    let (status, json) = post_json(
        &app,
        &format!("/api/runs/{run_id}/export"),
        serde_json::json!({
            "photo_ids": [id_keep.to_string(), id_fav.to_string()],
            "target_dir": target,
            "link_mode": "copy",
            "write_xmp": true,
            "flagged_ids": [id_fav.to_string()],
            "notes": { id_keep.to_string(): "crop tighter <later>" },
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["exported"], 2);
    assert_eq!(json["xmp_written"], 2);

    let keeper_xmp = std::fs::read_to_string(target.join("keeper.xmp")).unwrap();
    assert!(keeper_xmp.contains(r#"xmp:Rating="4""#), "plain keeper gets rating 4");
    assert!(keeper_xmp.contains("crop tighter &lt;later&gt;"), "note is XML-escaped");
    let fav_xmp = std::fs::read_to_string(target.join("fav.xmp")).unwrap();
    assert!(fav_xmp.contains(r#"xmp:Rating="5""#), "flagged photo gets rating 5");
    assert!(!fav_xmp.contains("dc:description"), "no note ⇒ no description block");
}

#[tokio::test]
async fn duplicates_groups_identical_content() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("shoot");
    std::fs::create_dir_all(&root).unwrap();
    // photo_ref gives every photo the same zeroed sha; give the third a
    // distinct hash so it must NOT join the duplicate set.
    let (ida, pa) = photo_ref(root.join("a.jpg"));
    let (idb, pb) = photo_ref(root.join("copy_of_a.jpg"));
    let (idc, mut pc) = photo_ref(root.join("unique.jpg"));
    pc.sha256_short = [9u8; 16];

    let state = test_state(tmp.path());
    let run_id = insert_run(&state, root, tmp.path().join("out"), vec![(ida, pa), (idb, pb), (idc, pc)]).await;
    let app = router(state);

    let resp = app.clone().oneshot(
        Request::get(&format!("/api/runs/{run_id}/duplicates")).body(Body::empty()).unwrap(),
    ).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["redundant_count"], 1);
    let groups = json["groups"].as_array().unwrap();
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0]["photos"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn similar_ranks_by_clip_cosine_from_cache() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("shoot");
    let output = tmp.path().join("out");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&output).unwrap();

    let (id_t, mut p_t) = photo_ref(root.join("target.jpg"));
    let (id_near, mut p_near) = photo_ref(root.join("near.jpg"));
    let (id_far, mut p_far) = photo_ref(root.join("far.jpg"));
    p_t.sha256_short = [1u8; 16];
    p_near.sha256_short = [2u8; 16];
    p_far.sha256_short = [3u8; 16];

    // Seed the run's feature cache with normalized embeddings.
    let cache = photo_pick_core::cache::CacheStore::open(&output.join(".cache.db")).unwrap();
    let mut feat = |id: PhotoId, embed: Vec<f32>| photo_pick_core::features::PhotoFeatures {
        clip_embed: Some(embed),
        ..photo_pick_core::features::PhotoFeatures::hashes_only(id, 0, 0)
    };
    cache.put(&[1u8; 16], &feat(id_t, vec![1.0, 0.0])).unwrap();
    let inv = 1.0 / (1.0f32 + 0.04).sqrt();
    cache.put(&[2u8; 16], &feat(id_near, vec![inv, 0.2 * inv])).unwrap();
    cache.put(&[3u8; 16], &feat(id_far, vec![0.0, 1.0])).unwrap();

    let state = test_state(tmp.path());
    let run_id = insert_run(&state, root, output, vec![(id_t, p_t), (id_near, p_near), (id_far, p_far)]).await;
    let app = router(state);

    let resp = app.clone().oneshot(
        Request::get(&format!("/api/runs/{run_id}/similar/{id_t}?k=5")).body(Body::empty()).unwrap(),
    ).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let similar = json["similar"].as_array().unwrap();
    assert_eq!(similar.len(), 2);
    assert_eq!(similar[0]["photo_id"], id_near.to_string(), "near vector ranks first");
    assert!(similar[0]["similarity"].as_f64().unwrap() > similar[1]["similarity"].as_f64().unwrap());
}

/// Travel-style run: every photo is its own composition group (all kept), so
/// the only thing separating them is the aesthetic score. With
/// `score_ratings`, sidecar stars rank against the whole run.
#[tokio::test]
async fn export_rates_by_aesthetic_score_against_the_whole_run() {
    use photo_pick_core::group::{CompositionGroup, GroupId};
    use photo_pick_core::scoring::{CompositionPick, FinalScore, Scene};

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("trip");
    let target = tmp.path().join("exported");
    std::fs::create_dir_all(&root).unwrap();

    // Five scored singletons (best → worst) plus one photo with no score.
    let mut photos = Vec::new();
    for name in ["p0.jpg", "p1.jpg", "p2.jpg", "p3.jpg", "p4.jpg", "unscored.jpg"] {
        let path = root.join(name);
        std::fs::write(&path, name.as_bytes()).unwrap();
        photos.push(photo_ref(path));
    }
    let ids: Vec<PhotoId> = photos.iter().map(|(id, _)| *id).collect();
    let picks: Vec<CompositionPick> = [0.9_f32, 0.7, 0.5, 0.3, 0.1]
        .iter()
        .zip(&ids)
        .map(|(aes, id)| CompositionPick {
            group: CompositionGroup { id: GroupId::new(), photo_ids: vec![*id] },
            kept: vec![(
                *id,
                FinalScore {
                    scene: Scene::Mixed,
                    tech: 0.5,
                    aesthetic: *aes,
                    composition: 0.5,
                    face_bonus: 0.0,
                    value: *aes,
                },
            )],
            rejected: vec![],
        })
        .collect();

    let state = test_state(tmp.path());
    let run_id = insert_run(&state, root, tmp.path().join("out"), photos).await;
    state.runs.lock().await.get_mut(&run_id).unwrap().composition_picks = picks;
    let app = router(state);

    // Export only some of them: ranks must still come from all five.
    let export = [ids[0], ids[2], ids[4], ids[5]];
    let (status, json) = post_json(
        &app,
        &format!("/api/runs/{run_id}/export"),
        serde_json::json!({
            "photo_ids": export.iter().map(|i| i.to_string()).collect::<Vec<_>>(),
            "target_dir": target,
            "write_xmp": true,
            "score_ratings": true,
            "flagged_ids": [ids[2].to_string()],
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["xmp_written"], 4);
    assert_eq!(json["xmp_scored"], 2, "best + worst; flagged and unscored don't count");

    let rating = |name: &str| {
        let x = std::fs::read_to_string(target.join(name)).unwrap();
        x.split("xmp:Rating=\"").nth(1).unwrap()[..1].to_string()
    };
    assert_eq!(rating("p0.xmp"), "4", "best of the run");
    assert_eq!(rating("p4.xmp"), "1", "worst of the run");
    assert_eq!(rating("p2.xmp"), "5", "user flag beats the score");
    assert_eq!(rating("unscored.xmp"), "4", "no score ⇒ old fixed rating");
}

/// Distinct procedural "photos" (different structure per `k`, so their
/// dHashes are far apart) with neutral areas in every tonal zone.
fn style_scene(k: u32) -> image::RgbImage {
    image::RgbImage::from_fn(480, 320, |x, y| {
        let band = match k % 3 {
            0 => x / 40,
            1 => y / 32,
            _ => (x / 48 + y / 48) % 4,
        };
        let v = (40 + (band * 37 + k * 11) % 180) as u8;
        if (x as i32 - 240).abs() < 30 && (y as i32 - 160).abs() < 30 {
            image::Rgb([200, 60, 40])
        } else {
            image::Rgb([v, v, v])
        }
    })
}

/// A clear grade: lifted blacks, warm highlights, cool shadows.
fn style_grade(img: &image::RgbImage) -> image::RgbImage {
    let mut o = img.clone();
    for p in o.pixels_mut() {
        let [r, g, b] = p.0.map(|c| c as f32 / 255.0);
        let y = 0.2126 * r + 0.7152 * g + 0.0722 * b;
        let (hi, sh) = (((y - 0.5) * 2.0).clamp(0.0, 1.0), ((0.5 - y) * 2.0).clamp(0.0, 1.0));
        let f = |v: f32, d: f32| ((0.08 + 0.9 * v + d).clamp(0.0, 1.0) * 255.0) as u8;
        p.0 = [f(r, 0.06 * hi - 0.05 * sh), f(g, 0.0), f(b, -0.06 * hi + 0.05 * sh)];
    }
    o
}

async fn style_run(tmp: &Path) -> (AppState, String, Vec<PhotoId>) {
    let root = tmp.join("shoot");
    std::fs::create_dir_all(&root).unwrap();
    let mut photos = Vec::new();
    for k in 0..3 {
        let path = root.join(format!("p{k}.jpg"));
        style_scene(k).save(&path).unwrap();
        photos.push(photo_ref(path));
    }
    let ids = photos.iter().map(|(id, _)| *id).collect();
    let state = test_state(tmp);
    let run_id = insert_run(&state, root, tmp.join("out"), photos).await;
    (state, run_id, ids)
}

#[tokio::test]
async fn style_preset_pairs_an_edit_with_its_original() {
    let _env = ROOTS_ENV.lock().await;
    let tmp = tempfile::tempdir().unwrap();
    std::env::set_var("PHOTO_PICK_BROWSE_ROOTS", tmp.path());
    let (state, run_id, ids) = style_run(tmp.path()).await;
    // The user's edited export of p1, saved outside the scanned folder.
    let edited = tmp.path().join("edits/p1-edit.jpg");
    std::fs::create_dir_all(edited.parent().unwrap()).unwrap();
    style_grade(&style_scene(1)).save(&edited).unwrap();
    let app = router(state);

    let (status, json) = post_json(
        &app,
        &format!("/api/runs/{run_id}/style-preset"),
        serde_json::json!({ "reference_paths": [edited], "name": "Trip look", "strength": 1.0 }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["mode"], "paired");
    assert_eq!(json["references_used"], 1);
    assert_eq!(json["file_name"], "Trip look.xmp");
    let xmp = json["xmp"].as_str().unwrap();
    assert!(xmp.contains("<crs:ToneCurvePV2012Red>") && xmp.contains("Trip look"));
    // The lifted-black grade shows up at the master curve's black point.
    assert!(json["preset"]["tone_curve"][0][1].as_u64().unwrap() >= 10, "{}", json["preset"]);
    // Previews skip the already-edited original.
    let previews: Vec<&str> = json["preview_ids"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
    assert!(!previews.is_empty() && !previews.contains(&ids[1].to_string().as_str()));

    // The preview endpoint renders the preset onto a run photo.
    let resp = app
        .clone()
        .oneshot(
            Request::post(format!("/api/runs/{run_id}/style-preview"))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({ "preset": json["preset"], "photo_id": previews[0] }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.headers()["content-type"], "image/jpeg");
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    assert!(image::load_from_memory(&bytes).is_ok());

    // References are held to the same allowlist as /browse and /scan.
    let outside = tempfile::tempdir().unwrap();
    let secret = outside.path().join("secret.jpg");
    style_scene(2).save(&secret).unwrap();
    let (status, _) = post_json(
        &app,
        &format!("/api/runs/{run_id}/style-preset"),
        serde_json::json!({ "reference_paths": [secret] }),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    std::env::remove_var("PHOTO_PICK_BROWSE_ROOTS");
}

#[tokio::test]
async fn style_preset_falls_back_to_unpaired_and_reports_bad_references() {
    let _env = ROOTS_ENV.lock().await;
    let tmp = tempfile::tempdir().unwrap();
    std::env::set_var("PHOTO_PICK_BROWSE_ROOTS", tmp.path());
    let (state, run_id, _) = style_run(tmp.path()).await;
    // A graded photo of something that isn't in the run, plus junk.
    let other = tmp.path().join("refs/other.jpg");
    std::fs::create_dir_all(other.parent().unwrap()).unwrap();
    let mut different = style_scene(7);
    for (x, y, p) in different.enumerate_pixels_mut() {
        if (x * 7 + y * 3) % 50 < 25 {
            *p = image::Rgb([20, 20, 20]);
        }
    }
    style_grade(&different).save(&other).unwrap();
    let junk = tmp.path().join("refs/broken.jpg");
    std::fs::write(&junk, b"not an image").unwrap();
    let app = router(state);

    let (status, json) = post_json(
        &app,
        &format!("/api/runs/{run_id}/style-preset"),
        serde_json::json!({ "reference_paths": [other, junk] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["mode"], "unpaired");
    assert_eq!(json["references_used"], 1);
    assert_eq!(json["references_failed"].as_array().unwrap().len(), 1);
    assert_eq!(json["preset"]["saturation"], 0, "unpaired never transfers saturation");
    assert_eq!(json["file_name"], "photo-pick style.xmp");

    // Nothing readable / nothing given → 400.
    let (status, _) = post_json(
        &app,
        &format!("/api/runs/{run_id}/style-preset"),
        serde_json::json!({ "reference_paths": [tmp.path().join("refs/broken.jpg")] }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = post_json(
        &app,
        &format!("/api/runs/{run_id}/style-preset"),
        serde_json::json!({ "reference_paths": [] }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    std::env::remove_var("PHOTO_PICK_BROWSE_ROOTS");
}

/// Regression: a completed run with no composition groups (CLIP off) used to
/// "rehydrate" from report.json on every call and replace its live photo map
/// with an empty one, so endpoints reported "photo not in run".
#[tokio::test]
async fn rehydration_keeps_live_photos_of_runs_without_composition_groups() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("shoot");
    let out = tmp.path().join("out");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&out).unwrap();
    std::fs::write(out.join("report.json"), br#"{"composition_groups": []}"#).unwrap();
    let path = root.join("a.jpg");
    style_scene(0).save(&path).unwrap();
    let (id, p) = photo_ref(path);
    let state = test_state(tmp.path());
    let run_id = insert_run(&state, root, out, vec![(id, p)]).await;
    let app = router(state);

    for _ in 0..2 {
        let resp = app
            .clone()
            .oneshot(Request::get(format!("/api/runs/{run_id}/thumb/{id}")).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "photo must stay in the run");
    }
}
