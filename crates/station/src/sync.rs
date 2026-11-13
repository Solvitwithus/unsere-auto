//! Background sync with the central server. Tickets are written locally first
//! (`dirty = 1`) and pushed when the server is reachable; masters, users and
//! office voids are pulled. Weighing never waits for the network.

use crate::app::{meta_get, meta_set, App};
use crate::config::StationScaleConfig;
use chrono::{DateTime, Utc};
use std::time::Duration;
use wb_core::{PullResponse, PushRequest, PushResponse, Ticket};

pub const TICKET_COLS: &str = "id, ticket_no, site_id, station_id, trans_type, status, vehicle_plate, driver_name, party_id, party_name, product_id, \
    product_name, source, destination, reference, memo, first_weight_kg, first_weight_at, first_raw, first_manual, second_weight_kg, second_weight_at, \
    second_raw, second_manual, net_weight_kg, charges, operator_id, operator_name, void_reason, print_count, created_at, updated_at";

pub fn spawn(app: App) {
    tokio::spawn(async move {
        loop {
            if app.config().is_enrolled() {
                if let Err(e) = run_once(&app).await {
                    let mut s = app.sync.lock().unwrap();
                    s.online = false;
                    s.error = Some(format!("{e:#}"));
                }
            }
            let pending: i64 = sqlx::query_scalar("SELECT count(*) FROM tickets WHERE dirty = 1").fetch_one(&app.db).await.unwrap_or(0);
            app.sync.lock().unwrap().pending = pending;
            let _ = tokio::time::timeout(Duration::from_secs(10), app.sync_now.notified()).await;
        }
    });
}

/// One sync round: send our changes first (so nothing typed here is ever
/// overwritten by an older server copy), then fetch what changed there.
pub async fn run_once(app: &App) -> anyhow::Result<()> {
    push(app).await?;
    pull(app).await?;
    let mut s = app.sync.lock().unwrap();
    s.online = true;
    s.error = None;
    Ok(())
}

async fn push(app: &App) -> anyhow::Result<()> {
    let cfg = app.config();
    let tickets: Vec<Ticket> = sqlx::query_as(&format!("SELECT {TICKET_COLS} FROM tickets WHERE dirty = 1 ORDER BY created_at LIMIT 200"))
        .fetch_all(&app.db)
        .await?;
    // Health report shown on the dashboard (Stations page): live scale state,
    // the scale settings in use, and how many tickets are waiting to sync.
    let status = serde_json::json!({
        "version": env!("CARGO_PKG_VERSION"),
        "scale": app.scale_state(),
        "scale_config": cfg.scale,
        "pending": tickets.len(),
        "at": Utc::now(),
    });
    let res = app
        .http
        .post(format!("{}/api/sync/push", cfg.server_url.trim_end_matches('/')))
        .header("x-station-key", &cfg.station_key)
        .json(&PushRequest { tickets: tickets.clone(), status: Some(status) })
        .timeout(Duration::from_secs(30))
        .send()
        .await?;
    anyhow::ensure!(res.status().is_success(), "server refused push: HTTP {}", res.status());
    let r: PushResponse = res.json().await?;
    for t in tickets.iter().filter(|t| r.accepted.contains(&t.id)) {
        // only clear if it wasn't changed again meanwhile
        sqlx::query("UPDATE tickets SET dirty = 0 WHERE id = ? AND updated_at = ?").bind(t.id).bind(t.updated_at).execute(&app.db).await?;
    }
    app.sync.lock().unwrap().last_push_at = Some(Utc::now());
    Ok(())
}

pub async fn pull(app: &App) -> anyhow::Result<()> {
    let cfg = app.config();
    // `last_pull` is the server's clock at our previous pull. Asking only for
    // changes since then keeps every pull small, even with years of data.
    let since = meta_get(&app.db, "last_pull").await;
    let mut req = app
        .http
        .get(format!("{}/api/sync/pull", cfg.server_url.trim_end_matches('/')))
        .header("x-station-key", &cfg.station_key)
        .timeout(Duration::from_secs(60));
    if let Some(s) = &since {
        req = req.query(&[("since", s)]);
    }
    let res = req.send().await?;
    anyhow::ensure!(res.status().is_success(), "server refused pull: HTTP {} (is the station key right?)", res.status());
    let p: PullResponse = res.json().await?;
    apply_pull(app, &p).await?;
    app.sync.lock().unwrap().last_pull_at = Some(Utc::now());
    Ok(())
}

async fn apply_pull(app: &App, p: &PullResponse) -> anyhow::Result<()> {
    let mut tx = app.db.begin().await?;
    if let Some(site) = &p.site {
        meta_set(&mut *tx, "site", &serde_json::to_string(site)?).await?;
    }
    if let Some(id) = p.station_id {
        meta_set(&mut *tx, "station_id", &id.to_string()).await?;
    }
    meta_set(&mut *tx, "station_code", &p.station_code).await?;
    // users: full replace so deactivations and removals take effect
    sqlx::query("DELETE FROM users").execute(&mut *tx).await?;
    for u in &p.users {
        sqlx::query("INSERT INTO users (id, username, full_name, password_hash, role, active) VALUES (?,?,?,?,?,?)")
            .bind(u.id).bind(&u.username).bind(&u.full_name).bind(&u.password_hash).bind(&u.role).bind(u.active)
            .execute(&mut *tx).await?;
    }
    for x in &p.parties {
        sqlx::query("INSERT OR REPLACE INTO parties (id, code, name, kind, tin, phone, email, address, active, updated_at) VALUES (?,?,?,?,?,?,?,?,?,?)")
            .bind(x.id).bind(&x.code).bind(&x.name).bind(&x.kind).bind(&x.tin).bind(&x.phone).bind(&x.email).bind(&x.address).bind(x.active).bind(x.updated_at)
            .execute(&mut *tx).await?;
    }
    for x in &p.products {
        sqlx::query("INSERT OR REPLACE INTO products (id, code, name, unit_price, quantity_unit, item_code, classification_code, tax_type_code, packaging_unit, active, updated_at) VALUES (?,?,?,?,?,?,?,?,?,?,?)")
            .bind(x.id).bind(&x.code).bind(&x.name).bind(x.unit_price).bind(&x.quantity_unit).bind(&x.item_code).bind(&x.classification_code)
            .bind(&x.tax_type_code).bind(&x.packaging_unit).bind(x.active).bind(x.updated_at)
            .execute(&mut *tx).await?;
    }
    for x in &p.vehicles {
        sqlx::query("DELETE FROM vehicles WHERE plate = ? AND id <> ?").bind(&x.plate).bind(x.id).execute(&mut *tx).await?;
        sqlx::query("INSERT OR REPLACE INTO vehicles (id, plate, stored_tare_kg, party_id, driver_name, active, updated_at) VALUES (?,?,?,?,?,?,?)")
            .bind(x.id).bind(&x.plate).bind(x.stored_tare_kg).bind(x.party_id).bind(&x.driver_name).bind(x.active).bind(x.updated_at)
            .execute(&mut *tx).await?;
    }
    // office changes to our tickets (voids): newer server copy wins, and isn't pushed back
    for t in &p.tickets {
        let local: Option<DateTime<Utc>> = sqlx::query_scalar("SELECT updated_at FROM tickets WHERE id = ?").bind(t.id).fetch_optional(&mut *tx).await?;
        if local.is_some_and(|l| l < t.updated_at) {
            sqlx::query("UPDATE tickets SET status = ?, void_reason = ?, updated_at = ?, dirty = 0 WHERE id = ?")
                .bind(&t.status).bind(&t.void_reason).bind(t.updated_at).bind(t.id)
                .execute(&mut *tx).await?;
        }
    }
    if let Some(now) = p.now {
        meta_set(&mut *tx, "last_pull", &now.to_rfc3339()).await?;
    }
    tx.commit().await?;

    // scale settings pushed from the dashboard
    if let Some(v) = &p.scale_config {
        let cfg = app.config();
        if cfg.server_scale_applied.as_ref() != Some(v) {
            match serde_json::from_value::<StationScaleConfig>(v.clone()) {
                Ok(sc) => {
                    tracing::info!("applying scale settings from server");
                    app.update_config(|c| {
                        c.scale = Some(sc);
                        c.server_scale_applied = Some(v.clone());
                    })?;
                    let a = app.clone();
                    tokio::task::spawn_blocking(move || a.restart_scale());
                }
                Err(e) => tracing::warn!("server scale config ignored: {e}"),
            }
        }
    }
    Ok(())
}
