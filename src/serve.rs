use anyhow::Result;
use axum::{
    body::Body,
    extract::State,
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::net::TcpListener;
use tower_http::cors::{Any, CorsLayer};
use tracing::info;

// Embed the viewer HTML directly into the binary
const VIEWER_HTML: &str = include_str!("../viewer.html");

/// Server configuration
pub struct ServeConfig {
    pub port: u16,
    pub db_path: Option<PathBuf>,
    pub open_browser: bool,
}

/// Shared state for the server
struct AppState {
    db_path: Option<PathBuf>,
}

/// Start the viewer server
pub async fn serve(config: ServeConfig) -> Result<()> {
    let state = Arc::new(AppState {
        db_path: config.db_path.clone(),
    });

    // Build CORS layer
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    // Build router
    let app = Router::new()
        .route("/", get(serve_viewer))
        .route("/db", get(serve_database))
        .route("/data.duckdb", get(serve_database))
        .layer(cors)
        .with_state(state);

    let addr = format!("0.0.0.0:{}", config.port);
    let listener = TcpListener::bind(&addr).await?;

    let local_url = format!("http://localhost:{}", config.port);

    info!("Starting viewer server at {}", local_url);

    if let Some(ref db_path) = config.db_path {
        info!("Serving database: {}", db_path.display());
        println!("\n  Viewer:   {}", local_url);
        println!("  Database: {}/db", local_url);
    } else {
        println!("\n  Viewer: {}", local_url);
        println!("  (drop a .duckdb file to load data)");
    }

    println!("\n  Press Ctrl+C to stop\n");

    // Open browser if requested
    if config.open_browser {
        if let Err(e) = open::that(&local_url) {
            tracing::warn!("Failed to open browser: {}", e);
        }
    }

    axum::serve(listener, app).await?;

    Ok(())
}

/// Serve the viewer HTML
async fn serve_viewer() -> impl IntoResponse {
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .header(header::CACHE_CONTROL, "no-cache")
        .body(Body::from(VIEWER_HTML))
        .unwrap()
}

/// Serve the database file
async fn serve_database(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let Some(ref db_path) = state.db_path else {
        return Response::builder()
            .status(StatusCode::NOT_FOUND)
            .header(header::CONTENT_TYPE, "text/plain")
            .body(Body::from("No database file configured"))
            .unwrap();
    };

    match tokio::fs::read(db_path).await {
        Ok(data) => Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "application/octet-stream")
            .header(
                header::CONTENT_DISPOSITION,
                "attachment; filename=\"data.duckdb\"",
            )
            .header(header::CACHE_CONTROL, "no-cache")
            .body(Body::from(data))
            .unwrap(),
        Err(e) => Response::builder()
            .status(StatusCode::INTERNAL_SERVER_ERROR)
            .header(header::CONTENT_TYPE, "text/plain")
            .body(Body::from(format!("Failed to read database: {}", e)))
            .unwrap(),
    }
}
