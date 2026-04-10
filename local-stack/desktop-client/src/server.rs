use axum::extract::State;
use axum::routing::get;
use axum::Router;
use serde::Serialize;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::net::TcpListener;
use tower_http::services::{ServeDir, ServeFile};
use tower_http::trace::TraceLayer;

#[derive(Clone)]
pub struct AppState {
    pub ui_dir: PathBuf,
    pub port: u16,
    pub hosts_entry_ok: bool,
}

#[derive(Serialize)]
pub struct StatusJson {
    pub bind_addr: String,
    pub port: u16,
    pub hostname: String,
    pub hosts_entry_ok: bool,
    pub preferred_url: String,
    pub fallback_url: String,
    pub ui_dir: String,
}

impl AppState {
    pub fn status_json(&self) -> StatusJson {
        let hostname = crate::hosts::HOSTNAME.to_string();
        let fallback_url = format!("http://127.0.0.1:{}", self.port);
        let preferred_url = if self.hosts_entry_ok {
            format!("http://{}:{}", hostname, self.port)
        } else {
            fallback_url.clone()
        };
        StatusJson {
            bind_addr: "127.0.0.1".to_string(),
            port: self.port,
            hostname,
            hosts_entry_ok: self.hosts_entry_ok,
            preferred_url,
            fallback_url,
            ui_dir: self.ui_dir.display().to_string(),
        }
    }
}

async fn api_status(State(state): State<Arc<AppState>>) -> axum::Json<StatusJson> {
    axum::Json(state.status_json())
}

pub async fn serve(
    listener: TcpListener,
    ui_dir: PathBuf,
    hosts_entry_ok: bool,
) -> Result<(), std::io::Error> {
    if !ui_dir.join("index.html").is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("index.html not found under {}", ui_dir.display()),
        ));
    }

    let port = listener.local_addr()?.port();
    let index = ui_dir.join("index.html");
    let state = Arc::new(AppState {
        ui_dir: ui_dir.clone(),
        port,
        hosts_entry_ok,
    });

    let static_files = ServeDir::new(&ui_dir)
        .append_index_html_on_directories(true)
        .not_found_service(ServeFile::new(index));

    let app = Router::new()
        .route("/api/v1/status", get(api_status))
        .fallback_service(static_files)
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    tracing::info!(port, "listening on 127.0.0.1");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    Ok(())
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
    tracing::info!("shutdown signal received");
}
