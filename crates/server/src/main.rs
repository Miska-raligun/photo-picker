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
    let app = router(state);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    println!("photo-pick server listening on http://{addr}");
    axum::serve(listener, app).await?;
    Ok(())
}
