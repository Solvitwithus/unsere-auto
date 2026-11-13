//! wb-station: runs on the weighbridge PC.
//!
//!   wb-station --data-dir C:\Weighbridge --web-dir web
//!
//! Then open http://127.0.0.1:7878 (the installer adds a shortcut / kiosk window).

mod api;
mod app;
mod config;
mod print;
mod sync;

use app::{App, Inner};
use clap::Parser;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};
use tower_http::services::{ServeDir, ServeFile};

#[derive(Parser)]
#[command(about = "Weighbridge station")]
struct Args {
    /// Where the local database and settings live
    #[arg(long, env = "WB_DATA_DIR", default_value = "station-data")]
    data_dir: PathBuf,
    /// Local address for the operator screen. Keep it on 127.0.0.1 unless
    /// other PCs on the LAN must reach this station.
    #[arg(long, env = "WB_LISTEN", default_value = "127.0.0.1:7878")]
    listen: String,
    /// The built dashboard/operator UI (web/out)
    #[arg(long, env = "WB_WEB_DIR", default_value = "web/out")]
    web_dir: PathBuf,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(std::env::var("RUST_LOG").unwrap_or_else(|_| "info,sqlx=warn".into()))
        .init();
    let a = Args::parse();
    std::fs::create_dir_all(&a.data_dir)?;

    let opts = SqliteConnectOptions::new()
        .filename(a.data_dir.join("station.db"))
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        // FULL: a power cut must never lose a weighed ticket
        .synchronous(SqliteSynchronous::Full)
        .foreign_keys(true);
    let db = SqlitePoolOptions::new().max_connections(4).connect_with(opts).await?;
    sqlx::migrate!("./migrations").run(&db).await?;

    let app = App(Arc::new(Inner {
        config: RwLock::new(config::StationConfig::load(&a.data_dir)),
        data_dir: a.data_dir.clone(),
        db,
        scale: Mutex::new(None),
        sessions: Mutex::new(Default::default()),
        sync: Mutex::new(Default::default()),
        sync_now: tokio::sync::Notify::new(),
        http: reqwest::Client::builder().user_agent(concat!("wb-station/", env!("CARGO_PKG_VERSION"))).build()?,
        web_dir: a.web_dir.clone(),
    }));
    {
        let a2 = app.clone();
        tokio::task::spawn_blocking(move || a2.restart_scale()).await?;
    }
    sync::spawn(app.clone());

    let web = app.web_dir.clone();
    let static_files = ServeDir::new(&web).not_found_service(ServeFile::new(web.join("404.html")));
    let router = api::routes().with_state(app).fallback_service(static_files);
    let listener = tokio::net::TcpListener::bind(&a.listen).await?;
    tracing::info!("station ready: http://{} (data in {})", a.listen, a.data_dir.display());
    axum::serve(listener, router).with_graceful_shutdown(async { let _ = tokio::signal::ctrl_c().await; }).await?;
    Ok(())
}
