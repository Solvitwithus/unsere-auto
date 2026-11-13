mod auth;
mod error;
mod etims;
mod import;
mod routes;
mod state;

use clap::{Parser, Subcommand};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use state::{AppState, EtimsConfig};
use std::str::FromStr;
use std::time::Duration;
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::{ServeDir, ServeFile};

#[derive(Parser)]
#[command(about = "Weighbridge central server")]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run the API + dashboard (default)
    Serve,
    /// Create or reset an admin user
    CreateAdmin {
        #[arg(long)]
        username: String,
        #[arg(long, env = "ADMIN_PASSWORD")]
        password: String,
        #[arg(long, default_value = "Administrator")]
        name: String,
    },
    /// Create a site and station from a client preset (presets/*.json). Prints the station key.
    ApplyPreset { file: String },
    /// Import CSV exports from the old system (see docs/IMPORT.md)
    Import {
        /// parties | products | vehicles | tickets
        kind: String,
        file: String,
        /// Site code the imported tickets belong to
        #[arg(long)]
        site: Option<String>,
        /// Check the file and report, but write nothing
        #[arg(long)]
        dry_run: bool,
    },
}

/// Neon: the pooled URL for traffic, the direct one (no "-pooler") for migrations.
fn options(url: &str) -> anyhow::Result<PgConnectOptions> {
    // sqlx doesn't know libpq's channel_binding; TLS is still enforced by sslmode=require
    let cleaned = url.replace("&channel_binding=require", "").replace("channel_binding=require&", "").replace("?channel_binding=require", "");
    let o = PgConnectOptions::from_str(&cleaned)?;
    Ok(o.statement_cache_capacity(std::env::var("DB_STATEMENT_CACHE").ok().and_then(|v| v.parse().ok()).unwrap_or(256)))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _ = dotenvy::dotenv();
    tracing_subscriber::fmt()
        .with_env_filter(std::env::var("RUST_LOG").unwrap_or_else(|_| "info,sqlx=warn,tower_http=info".into()))
        .init();
    let cli = Cli::parse();

    let url = std::env::var("DATABASE_URL").map_err(|_| anyhow::anyhow!("DATABASE_URL is not set (see .env.example)"))?;
    let direct = std::env::var("DIRECT_DATABASE_URL").unwrap_or_else(|_| url.replace("-pooler.", "."));
    {
        let mig = PgPoolOptions::new().max_connections(1).acquire_timeout(Duration::from_secs(30)).connect_with(options(&direct)?).await?;
        sqlx::migrate!("./migrations").run(&mig).await?;
        mig.close().await;
    }
    let db = PgPoolOptions::new()
        .max_connections(std::env::var("DB_MAX_CONNECTIONS").ok().and_then(|v| v.parse().ok()).unwrap_or(10))
        .min_connections(2)
        .acquire_timeout(Duration::from_secs(30))
        .connect_with(options(&url)?)
        .await?;

    let etims = match (std::env::var("ETIMS_URL"), std::env::var("ETIMS_API_KEY")) {
        (Ok(u), Ok(k)) if !u.is_empty() && !k.is_empty() => Some(EtimsConfig {
            base_url: u.trim_end_matches('/').to_string(),
            api_key: k,
            control_unit_id: std::env::var("ETIMS_CONTROL_UNIT_ID").ok().filter(|s| !s.is_empty()),
        }),
        _ => None,
    };
    let jwt_secret = std::env::var("JWT_SECRET").unwrap_or_default();
    anyhow::ensure!(jwt_secret.len() >= 32, "JWT_SECRET must be at least 32 characters");
    let st = AppState { db, jwt_secret, etims, http: reqwest::Client::builder().user_agent("wb-server").build()? };

    match cli.cmd.unwrap_or(Cmd::Serve) {
        Cmd::CreateAdmin { username, password, name } => {
            anyhow::ensure!(password.len() >= 8, "password must be at least 8 characters");
            let username = username.trim().to_lowercase();
            sqlx::query(
                "INSERT INTO users (id, username, full_name, password_hash, role) VALUES ($1,$2,$3,$4,'admin')
                 ON CONFLICT (username) DO UPDATE SET password_hash = EXCLUDED.password_hash, role = 'admin', active = TRUE, updated_at = now()",
            )
            .bind(uuid::Uuid::new_v4()).bind(&username).bind(&name).bind(wb_core::password::hash(&password))
            .execute(&st.db).await?;
            println!("admin '{username}' ready");
        }
        Cmd::ApplyPreset { file } => import::apply_preset(&st, &file).await?,
        Cmd::Import { kind, file, site, dry_run } => import::run(&st, &kind, &file, site.as_deref(), dry_run).await?,
        Cmd::Serve => {
            etims::spawn(st.clone());
            let web_dir = std::env::var("WEB_DIR").unwrap_or_else(|_| "web/out".into());
            let static_files = ServeDir::new(&web_dir).not_found_service(ServeFile::new(format!("{web_dir}/404.html")));
            let cors = match std::env::var("CORS_ORIGINS") {
                Ok(list) if !list.is_empty() => CorsLayer::new()
                    .allow_origin(list.split(',').filter_map(|o| o.trim().parse().ok()).collect::<Vec<axum::http::HeaderValue>>())
                    .allow_methods(Any)
                    .allow_headers(Any),
                _ => CorsLayer::new(),
            };
            let app = routes::api()
                .with_state(st)
                .fallback_service(static_files)
                .layer(cors)
                .layer(tower_http::trace::TraceLayer::new_for_http());
            let bind = std::env::var("BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".into());
            let listener = tokio::net::TcpListener::bind(&bind).await?;
            tracing::info!("wb-server on http://{bind} (dashboard from {web_dir})");
            axum::serve(listener, app).with_graceful_shutdown(async { let _ = tokio::signal::ctrl_c().await; }).await?;
        }
    }
    Ok(())
}
