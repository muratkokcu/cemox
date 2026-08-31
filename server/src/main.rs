//! Cemox randevu API'si — `server.js` yerine geçen Rust sunucusu.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::Duration;

use cemox_server::app::{AppState, build_router};
use cemox_server::config::{app_root, load_config};
use cemox_server::db::Db;
use cemox_server::email::EmailService;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "cemox_server=info,tower_http=warn".into()),
        )
        .init();

    let env = collect_env();
    let config =
        load_config(&env).map_err(|message| -> Box<dyn std::error::Error> { message.into() })?;

    let db = Db::new(&config.database_path)?;
    let email = EmailService::new(&config);
    if !email.enabled() {
        tracing::warn!("SMTP yapılandırılmadı; e-postalar yalnızca loglanacak.");
    }

    let port = config.port;
    let state = AppState::new(config, db, email);
    let router = build_router(state.clone());

    let address = SocketAddr::from(([0, 0, 0, 0], port));
    let listener = tokio::net::TcpListener::bind(address).await?;
    tracing::info!("Cemox API listening on http://localhost:{port}");

    // `runMaintenance` her 15 dakikada bir süresi dolan talepleri kapatır.
    let maintenance_state = state.clone();
    let maintenance = tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(15 * 60));
        ticker.tick().await; // ilk tick anında düşer, atlanır
        loop {
            ticker.tick().await;
            match maintenance_state
                .run_maintenance(cemox_server::time::now_ms())
                .await
            {
                Ok(count) if count > 0 => tracing::info!(expired = count, "bakım tamamlandı"),
                Ok(_) => {}
                Err(error) => tracing::error!(%error, "bakım başarısız"),
            }
        }
    });

    axum::serve(
        listener,
        router.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await?;

    maintenance.abort();
    Ok(())
}

async fn shutdown_signal() {
    let interrupt = async {
        tokio::signal::ctrl_c()
            .await
            .expect("SIGINT dinleyicisi kurulamadı");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("SIGTERM dinleyicisi kurulamadı")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = interrupt => {},
        _ = terminate => {},
    }
    tracing::info!("kapatılıyor…");
}

/// Süreç ortamı ile `.env` dosyasını birleştirir.
/// Node `process.loadEnvFile` gibi, gerçek ortam değişkenleri dosyayı ezer.
fn collect_env() -> HashMap<String, String> {
    let mut env: HashMap<String, String> = HashMap::new();
    if let Ok(contents) = std::fs::read_to_string(app_root().join(".env")) {
        for (key, value) in parse_env_file(&contents) {
            env.insert(key, value);
        }
    }
    for (key, value) in std::env::vars() {
        env.insert(key, value);
    }
    env
}

fn parse_env_file(contents: &str) -> Vec<(String, String)> {
    contents
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(|line| {
            let line = line.strip_prefix("export ").unwrap_or(line);
            let (key, value) = line.split_once('=')?;
            let value = value.trim();
            // Tırnak içindeki değerlerden tırnakları ayıkla (ör. SMTP_FROM).
            let value = value
                .strip_prefix('"')
                .and_then(|rest| rest.strip_suffix('"'))
                .or_else(|| {
                    value
                        .strip_prefix('\'')
                        .and_then(|rest| rest.strip_suffix('\''))
                })
                .unwrap_or(value);
            Some((key.trim().to_string(), value.to_string()))
        })
        .collect()
}
