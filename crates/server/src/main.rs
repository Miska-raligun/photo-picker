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
    // Kept for the post-serve shutdown hooks; `router` consumes the original.
    let shutdown_state = state.clone();
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
    // Graceful shutdown on Ctrl+C / SIGTERM: stop accepting connections, let
    // in-flight requests finish, flip every scan's cancellation flag so
    // running pipelines stop at their next checkpoint, and persist the run
    // index one final time. The runs.json write is already atomic
    // (tmp+rename), so this is about promptness, not corruption.
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    shutdown_state.cancel_all_runs().await;
    shutdown_state.persist_runs().await;
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c().await.expect("install Ctrl+C handler");
    };
    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("install SIGTERM handler")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    println!("photo-pick: shutting down…");
}
