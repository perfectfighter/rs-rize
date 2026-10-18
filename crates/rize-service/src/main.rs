use rize_service::{router, validate_binding, AppState};
#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let env: std::collections::HashMap<String, String> = std::env::vars().collect();
    let host = env
        .get("RIZE_SERVER_HOST")
        .cloned()
        .unwrap_or_else(|| "127.0.0.1".into());
    validate_binding(&host, &env)?;
    let port = env
        .get("PORT")
        .or_else(|| env.get("RIZE_SERVER_PORT"))
        .map(String::as_str)
        .unwrap_or("4791")
        .parse::<u16>()?;
    let state = AppState::new(
        env.get("RIZE_DB_PATH")
            .map(String::as_str)
            .unwrap_or("data/rize-service.sqlite"),
        env.clone(),
    )?;
    #[cfg(unix)]
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let listener = tokio::net::TcpListener::bind((host.as_str(), port)).await?;
    eprintln!("Rize Rust service listening on {}", listener.local_addr()?);
    let task = if env.get("RIZE_RUN_ROUTINES").map(String::as_str) == Some("1") {
        let state = state.clone();
        Some(tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
            interval.tick().await;
            loop {
                interval.tick().await;
                let _ = state.tick_services().await;
            }
        }))
    } else {
        None
    };
    let served = axum::serve(
        listener,
        router(state.clone()).into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(async move {
        #[cfg(unix)]
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {},
            _ = terminate.recv() => {},
        }
        #[cfg(not(unix))]
        let _ = tokio::signal::ctrl_c().await;
    })
    .await;
    if let Some(task) = task {
        task.abort();
        let _ = task.await;
    }
    // All requests and the routine task have stopped before checkpointing WAL.
    state
        .db()?
        .execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")?;
    served?;
    Ok(())
}
