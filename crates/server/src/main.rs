use anyhow::Result;
use photo_pick_server::{router, AppState};
use std::net::SocketAddr;

#[tokio::main]
async fn main() -> Result<()> {
    // Silence rawler's "Decoder has no preview image support" WARNs — we treat
    // them as expected (we always have an EXIF/byte-scan fallback). Override
    // by setting RUST_LOG.
    let filter = std::env::var("RUST_LOG")
        .unwrap_or_else(|_| "info,photo_pick=info,rawler=error".into());
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .init();

    let addr: SocketAddr = std::env::var("PHOTO_PICK_BIND")
        .unwrap_or_else(|_| "127.0.0.1:7777".into())
        .parse()?;

    let state = AppState::new();
    // Restore the list of past runs from disk so users see their history
    // after a restart. Detail (composition_picks/photos) is lazy-loaded on
    // first access via the report.json on disk.
    state.load_from_disk().await;
    let app = router(state);

    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
            eprintln!("photo-pick: port {} on {} is already in use.", addr.port(), addr.ip());
            eprintln!("  → set PHOTO_PICK_BIND=127.0.0.1:<port> (e.g. 7778) and retry,");
            eprintln!("    or stop the process holding the port (`lsof -i :{}` on macOS/Linux).", addr.port());
            std::process::exit(2);
        }
        Err(e) => {
            return Err(anyhow::anyhow!("bind {addr}: {e}"));
        }
    };
    println!("photo-pick server listening on http://{addr}");
    axum::serve(listener, app).await?;
    Ok(())
}
