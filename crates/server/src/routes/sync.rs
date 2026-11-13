//! Station <-> server sync.
//!
//! * `POST /api/sync/push`: the station uploads tickets created/changed
//!   offline. Last write wins on `updated_at`, so an office void (newer) is not
//!   undone by a stale copy from the station.
//! * `GET /api/sync/pull?since=`: masters, users (for offline login), the
//!   station's own tickets changed on the server (voids), and its scale config.

use crate::auth::StationAuth;
use crate::error::ApiResult;
use crate::routes::masters::PRODUCT_COLS;
use crate::state::AppState;
use axum::extract::{Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use uuid::Uuid;
use wb_core::{normalize_plate, PullResponse, PushRequest, PushResponse, Ticket};

pub fn routes() -> Router<AppState> {
    Router::new().route("/api/sync/push", post(push)).route("/api/sync/pull", get(pull))
}

async fn push(State(st): State<AppState>, s: StationAuth, Json(b): Json<PushRequest>) -> ApiResult<PushResponse> {
    sqlx::query("UPDATE stations SET last_seen_at = now(), last_status = COALESCE($2, last_status) WHERE id = $1")
        .bind(s.station_id).bind(&b.status).execute(&st.db).await?;
    let mut accepted = Vec::new();
    for t in b.tickets {
        match upsert_ticket(&st, &s, &t).await {
            Ok(()) => accepted.push(t.id),
            Err(e) => tracing::warn!("station {} ticket {}: {e:#}", s.code, t.ticket_no),
        }
    }
    Ok(Json(PushResponse { accepted }))
}

async fn upsert_ticket(st: &AppState, s: &StationAuth, t: &Ticket) -> anyhow::Result<()> {
    anyhow::ensure!(wb_core::TRANS_TYPES.contains(&t.trans_type.as_str()), "bad trans_type");
    let plate = normalize_plate(&t.vehicle_plate);
    let mut tx = st.db.begin().await?;
    // a plate typed at the station becomes a vehicle record
    sqlx::query("INSERT INTO vehicles (id, plate, driver_name) VALUES ($1, $2, $3) ON CONFLICT (plate) DO NOTHING")
        .bind(Uuid::new_v4()).bind(&plate).bind(&t.driver_name).execute(&mut *tx).await?;
    // ignore references to masters the server doesn't know
    let party_id: Option<Uuid> = match t.party_id {
        Some(p) => sqlx::query_scalar("SELECT id FROM parties WHERE id=$1").bind(p).fetch_optional(&mut *tx).await?,
        None => None,
    };
    let product_id: Option<Uuid> = match t.product_id {
        Some(p) => sqlx::query_scalar("SELECT id FROM products WHERE id=$1").bind(p).fetch_optional(&mut *tx).await?,
        None => None,
    };
    // Insert the ticket, or update our copy if the station's is newer.
    // Rules enforced in SQL:
    //   * site and station always come from the station key, never from the body,
    //     so one client's station can't write into another client's data;
    //   * an update only applies when the station's `updated_at` is newer AND
    //     the ticket belongs to the same station;
    //   * the first weight is never changed after creation.
    let res = sqlx::query(
        "INSERT INTO tickets (id, ticket_no, site_id, station_id, trans_type, status, vehicle_plate, driver_name, party_id, party_name, product_id, product_name,
            source, destination, reference, memo, first_weight_kg, first_weight_at, first_raw, first_manual, second_weight_kg, second_weight_at, second_raw,
            second_manual, charges, operator_id, operator_name, void_reason, print_count, created_at, updated_at, synced_at)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,$21,$22,$23,$24,$25,$26,$27,$28,$29,$30,$31, now())
         ON CONFLICT (id) DO UPDATE SET
            trans_type=EXCLUDED.trans_type, status=EXCLUDED.status, vehicle_plate=EXCLUDED.vehicle_plate, driver_name=EXCLUDED.driver_name,
            party_id=EXCLUDED.party_id, party_name=EXCLUDED.party_name, product_id=EXCLUDED.product_id, product_name=EXCLUDED.product_name,
            source=EXCLUDED.source, destination=EXCLUDED.destination, reference=EXCLUDED.reference, memo=EXCLUDED.memo,
            second_weight_kg=EXCLUDED.second_weight_kg, second_weight_at=EXCLUDED.second_weight_at, second_raw=EXCLUDED.second_raw,
            second_manual=EXCLUDED.second_manual, charges=EXCLUDED.charges, void_reason=EXCLUDED.void_reason, print_count=EXCLUDED.print_count,
            updated_at=EXCLUDED.updated_at, synced_at=now()
         WHERE tickets.updated_at < EXCLUDED.updated_at AND tickets.station_id = EXCLUDED.station_id",
    )
    .bind(t.id).bind(&t.ticket_no).bind(s.site_id).bind(s.station_id).bind(&t.trans_type).bind(&t.status).bind(&plate).bind(&t.driver_name)
    .bind(party_id).bind(&t.party_name).bind(product_id).bind(&t.product_name).bind(&t.source).bind(&t.destination).bind(&t.reference)
    .bind(&t.memo).bind(t.first_weight_kg).bind(t.first_weight_at).bind(&t.first_raw).bind(t.first_manual).bind(t.second_weight_kg)
    .bind(t.second_weight_at).bind(&t.second_raw).bind(t.second_manual).bind(t.charges).bind(t.operator_id).bind(&t.operator_name)
    .bind(&t.void_reason).bind(t.print_count).bind(t.created_at).bind(t.updated_at)
    .execute(&mut *tx).await?;
    // remember the tare of an empty weighing for next time
    if t.status == "complete" {
        if let (Some(second), true) = (t.second_weight_kg, res.rows_affected() > 0) {
            let tare = second.min(t.first_weight_kg);
            sqlx::query("UPDATE vehicles SET stored_tare_kg=$2, tare_updated_at=now(), updated_at=now() WHERE plate=$1").bind(&plate).bind(tare).execute(&mut *tx).await?;
        }
    }
    tx.commit().await?;

    // auto-invoice completed sales when the site wants it
    if res.rows_affected() > 0 && t.status == "complete" && t.trans_type == "sale" {
        let auto: Option<bool> = sqlx::query_scalar("SELECT auto_invoice FROM sites WHERE id=$1").bind(s.site_id).fetch_optional(&st.db).await?;
        if auto == Some(true) {
            match crate::routes::sales::create_from_ticket(st, t.id, Default::default(), (t.operator_name.clone(), format!("station:{}", s.code))).await {
                Ok(_) => crate::etims::kick(st),
                Err(e) => tracing::info!("auto-invoice {} skipped: {e}", t.ticket_no),
            }
        }
    }
    Ok(())
}

#[derive(Deserialize)]
struct PullQ {
    since: Option<DateTime<Utc>>,
}

async fn pull(State(st): State<AppState>, s: StationAuth, Query(q): Query<PullQ>) -> ApiResult<PullResponse> {
    let since = q.since.unwrap_or(DateTime::<Utc>::UNIX_EPOCH);
    let now: DateTime<Utc> = sqlx::query_scalar("SELECT now()").fetch_one(&st.db).await?;
    sqlx::query("UPDATE stations SET last_seen_at = now() WHERE id = $1").bind(s.station_id).execute(&st.db).await?;
    let site = sqlx::query_as("SELECT id, code, name, address, phone, ticket_footer FROM sites WHERE id=$1").bind(s.site_id).fetch_optional(&st.db).await?;
    // users for this site (and global ones), always sent in full so removals propagate
    let users = sqlx::query_as("SELECT id, username, full_name, password_hash, role, active FROM users WHERE (site_id IS NULL OR site_id = $1) AND role IN ('admin','supervisor','operator')")
        .bind(s.site_id).fetch_all(&st.db).await?;
    let parties = sqlx::query_as("SELECT id, code, name, kind, tin, phone, email, address, active, updated_at FROM parties WHERE updated_at > $1")
        .bind(since).fetch_all(&st.db).await?;
    let products = sqlx::query_as(&format!("SELECT {PRODUCT_COLS} FROM products WHERE updated_at > $1")).bind(since).fetch_all(&st.db).await?;
    let vehicles = sqlx::query_as("SELECT id, plate, stored_tare_kg, party_id, driver_name, active, updated_at FROM vehicles WHERE updated_at > $1")
        .bind(since).fetch_all(&st.db).await?;
    let tickets = sqlx::query_as::<_, Ticket>(&format!(
        "SELECT {} FROM tickets t WHERE t.station_id = $1 AND t.synced_at > $2",
        crate::routes::tickets::TICKET_COLS
    ))
    .bind(s.station_id).bind(since).fetch_all(&st.db).await?;
    let scale_config: Option<serde_json::Value> =
        sqlx::query_scalar("SELECT scale_config FROM stations WHERE id=$1").bind(s.station_id).fetch_one(&st.db).await?;
    Ok(Json(PullResponse {
        now: Some(now),
        station_id: Some(s.station_id),
        station_code: s.code,
        site,
        users,
        parties,
        products,
        vehicles,
        tickets,
        scale_config,
    }))
}
