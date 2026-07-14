use aeon_memory_gateway::{AppConfig, app, config::discover_and_load_config, runtime::build_core};
use clap::Parser;
use std::path::PathBuf;
use std::time::Duration;

#[cfg(unix)]
async fn shutdown_signal() {
    use tokio::signal::unix::{SignalKind, signal};

    let mut terminate = signal(SignalKind::terminate()).expect("install SIGTERM handler");
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = terminate.recv() => {}
    }
}

#[cfg(not(unix))]
async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}

#[derive(Parser)]
struct Args {
    /// Gateway YAML/JSON configuration. When omitted, uses TS-compatible discovery.
    #[arg(long)]
    config: Option<PathBuf>,
}

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("aeon-memory-server: {error}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), String> {
    let args = Args::parse();
    let (config_path, config) =
        discover_and_load_config(args.config.as_deref()).map_err(|error| error.to_string())?;
    eprintln!(
        "aeon-memory-server: config={}, data={}",
        config_path.display(),
        config.data.base_dir
    );
    let service = build_core(&config)
        .await
        .map_err(|error| error.to_string())?;
    let router = app(
        service.clone(),
        AppConfig {
            api_key: config.server.api_key.clone(),
            cors_origins: config.server.cors_origins.clone(),
        },
    );
    let listener = tokio::net::TcpListener::bind((config.server.host.as_str(), config.server.port))
        .await
        .map_err(|error| error.to_string())?;
    let server_result = axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .map_err(|error| error.to_string());

    // HTTP must stop accepting requests before core teardown. The official
    // host integration bounds gateway cleanup at three seconds; preserve that
    // recovery-friendly behavior while making failures visible on stderr.
    const CORE_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(3);
    let shutdown_result = match tokio::time::timeout(CORE_SHUTDOWN_TIMEOUT, service.shutdown())
        .await
    {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) => {
            eprintln!("aeon-memory-server: core shutdown failed: {error}");
            Err(error.to_string())
        }
        Err(_) => {
            eprintln!(
                "aeon-memory-server: warning: core shutdown exceeded {}ms; pending work remains checkpointed for recovery",
                CORE_SHUTDOWN_TIMEOUT.as_millis()
            );
            Ok(())
        }
    };
    server_result?;
    shutdown_result
}
