use sqlx::PgPool;

#[derive(Clone, Debug)]
pub struct EtimsConfig {
    /// e.g. https://etims.example.com/api/v1
    pub base_url: String,
    /// etims-rust API key (sent as `Authorization: Bearer ...`)
    pub api_key: String,
    pub control_unit_id: Option<String>,
}

#[derive(Clone)]
pub struct AppState {
    pub db: PgPool,
    pub jwt_secret: String,
    pub etims: Option<EtimsConfig>,
    pub http: reqwest::Client,
}

impl AppState {
    pub fn etims_initial_status(&self) -> &'static str {
        if self.etims.is_some() {
            "PENDING"
        } else {
            "DISABLED"
        }
    }
}

pub async fn audit(db: &PgPool, actor: &str, action: &str, entity: &str, entity_id: &str, details: serde_json::Value) {
    if let Err(e) = sqlx::query("INSERT INTO audit_log (actor, action, entity, entity_id, details) VALUES ($1,$2,$3,$4,$5)")
        .bind(actor)
        .bind(action)
        .bind(entity)
        .bind(entity_id)
        .bind(details)
        .execute(db)
        .await
    {
        tracing::warn!("audit write failed: {e}");
    }
}

/// Next value of a named counter (atomic).
pub async fn next_counter<'e, E: sqlx::PgExecutor<'e>>(db: E, name: &str) -> sqlx::Result<i64> {
    sqlx::query_scalar(
        "INSERT INTO counters (name, value) VALUES ($1, 1) ON CONFLICT (name) DO UPDATE SET value = counters.value + 1 RETURNING value",
    )
    .bind(name)
    .fetch_one(db)
    .await
}
