//! Shared station state: config, local DB, scale engine, sessions, sync status.

use crate::config::{StationConfig, StationScaleConfig};
use serde::Serialize;
use sqlx::SqlitePool;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};
use tokio::sync::Notify;
use uuid::Uuid;
use wb_scale::{ScaleHandle, ScaleState};

#[derive(Clone, Debug, Serialize)]
pub struct Session {
    pub user_id: Uuid,
    pub username: String,
    pub name: String,
    pub role: String,
    #[serde(skip)]
    pub expires: Instant,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct SyncStatus {
    pub last_push_at: Option<chrono::DateTime<chrono::Utc>>,
    pub last_pull_at: Option<chrono::DateTime<chrono::Utc>>,
    pub pending: i64,
    pub online: bool,
    pub error: Option<String>,
}

pub struct Inner {
    pub data_dir: PathBuf,
    pub db: SqlitePool,
    pub config: RwLock<StationConfig>,
    pub scale: Mutex<Option<ScaleHandle>>,
    pub sessions: Mutex<HashMap<String, Session>>,
    pub sync: Mutex<SyncStatus>,
    pub sync_now: Notify,
    pub http: reqwest::Client,
    pub web_dir: PathBuf,
}

#[derive(Clone)]
pub struct App(pub Arc<Inner>);

impl std::ops::Deref for App {
    type Target = Inner;
    fn deref(&self) -> &Inner {
        &self.0
    }
}

impl App {
    pub fn config(&self) -> StationConfig {
        self.config.read().unwrap().clone()
    }

    pub fn update_config(&self, f: impl FnOnce(&mut StationConfig)) -> anyhow::Result<StationConfig> {
        let mut c = self.config.write().unwrap();
        f(&mut c);
        c.save(&self.data_dir)?;
        Ok(c.clone())
    }

    pub fn scale_state(&self) -> ScaleState {
        self.scale.lock().unwrap().as_ref().map(|h| h.current()).unwrap_or_default()
    }

    pub fn raw_bytes(&self) -> Vec<u8> {
        self.scale.lock().unwrap().as_ref().map(|h| h.raw_bytes()).unwrap_or_default()
    }

    /// (Re)start the scale reader with the saved config.
    pub fn restart_scale(&self) {
        // drop the old handle first so the port is released
        self.scale.lock().unwrap().take();
        let cfg = self.config().scale;
        let Some(cfg) = cfg else {
            tracing::info!("no scale configured yet");
            return;
        };
        std::thread::sleep(Duration::from_millis(400));
        match cfg.engine_config().and_then(wb_scale::start) {
            Ok(h) => *self.scale.lock().unwrap() = Some(h),
            Err(e) => tracing::warn!("scale start failed: {e:#}"),
        }
    }

    /// Stop the reader (for detect/learn, which need the port).
    pub fn stop_scale(&self) {
        self.scale.lock().unwrap().take();
        std::thread::sleep(Duration::from_millis(500));
    }

    pub fn scale_settings(&self) -> Option<StationScaleConfig> {
        self.config().scale
    }

    pub fn session(&self, token: &str) -> Option<Session> {
        let mut s = self.sessions.lock().unwrap();
        s.retain(|_, v| v.expires > Instant::now());
        s.get(token).cloned()
    }
}

pub async fn meta_get(db: &SqlitePool, key: &str) -> Option<String> {
    sqlx::query_scalar("SELECT value FROM meta WHERE key = ?").bind(key).fetch_optional(db).await.ok().flatten()
}

pub async fn meta_set<'e, E: sqlx::SqliteExecutor<'e>>(db: E, key: &str, value: &str) -> sqlx::Result<()> {
    sqlx::query("INSERT INTO meta (key, value) VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value")
        .bind(key)
        .bind(value)
        .execute(db)
        .await?;
    Ok(())
}
