//! Tickets (read, void, CSV), reports.

use crate::auth::AuthUser;
use crate::error::{bad, ApiResult, AppError};
use crate::routes::masters::rows_json;
use crate::state::{audit, AppState};
use axum::extract::{Path, Query, State};
use axum::http::header;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{Postgres, QueryBuilder};
use uuid::Uuid;
use wb_core::Ticket;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/tickets", get(list))
        .route("/api/tickets/export.csv", get(export_csv))
        .route("/api/tickets/{id}", get(get_one))
        .route("/api/tickets/{id}/void", post(void))
        .route("/api/tickets/{id}/text", get(ticket_text))
        .route("/api/reports/summary", get(summary))
}

pub const TICKET_COLS: &str = "t.id, t.ticket_no, t.site_id, t.station_id, t.trans_type, t.status, t.vehicle_plate, t.driver_name, t.party_id, t.party_name, \
    t.product_id, t.product_name, t.source, t.destination, t.reference, t.memo, t.first_weight_kg, t.first_weight_at, t.first_raw, t.first_manual, \
    t.second_weight_kg, t.second_weight_at, t.second_raw, t.second_manual, t.net_weight_kg, t.charges::float8 AS charges, t.operator_id, t.operator_name, \
    t.void_reason, t.print_count, t.created_at, t.updated_at";

#[derive(Serialize, sqlx::FromRow)]
pub struct TicketRow {
    #[sqlx(flatten)]
    #[serde(flatten)]
    pub ticket: Ticket,
    pub site_name: Option<String>,
    pub station_code: Option<String>,
    pub sale_id: Option<Uuid>,
    pub sale_no: Option<String>,
    pub etims_status: Option<String>,
}

#[derive(Deserialize, Default)]
pub struct Filter {
    pub from: Option<NaiveDate>,
    pub to: Option<NaiveDate>,
    pub site_id: Option<Uuid>,
    pub status: Option<String>,
    pub trans_type: Option<String>,
    pub q: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

fn base_query<'a>(select: &str, f: &'a Filter, scope_site: Option<Uuid>) -> QueryBuilder<'a, Postgres> {
    let mut qb = QueryBuilder::new(select);
    qb.push(" FROM tickets t LEFT JOIN sites s ON s.id = t.site_id LEFT JOIN stations st ON st.id = t.station_id \
              LEFT JOIN sales sa ON sa.ticket_id = t.id AND sa.status = 'APPROVED' AND sa.sale_type = 'NORMAL' WHERE TRUE");
    if let Some(d) = f.from {
        qb.push(" AND t.created_at >= ").push_bind(d.and_hms_opt(0, 0, 0).unwrap().and_utc());
    }
    if let Some(d) = f.to {
        qb.push(" AND t.created_at < ").push_bind((d + chrono::Duration::days(1)).and_hms_opt(0, 0, 0).unwrap().and_utc());
    }
    if let Some(s) = scope_site.or(f.site_id) {
        qb.push(" AND t.site_id = ").push_bind(s);
    }
    if let Some(s) = f.status.as_deref().filter(|s| !s.is_empty()) {
        qb.push(" AND t.status = ").push_bind(s.to_string());
    }
    if let Some(s) = f.trans_type.as_deref().filter(|s| !s.is_empty()) {
        qb.push(" AND t.trans_type = ").push_bind(s.to_string());
    }
    if let Some(q) = f.q.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        let like = format!("%{q}%");
        qb.push(" AND (t.ticket_no ILIKE ").push_bind(like.clone())
            .push(" OR t.vehicle_plate ILIKE ").push_bind(format!("%{}%", wb_core::normalize_plate(q)))
            .push(" OR t.party_name ILIKE ").push_bind(like.clone())
            .push(" OR t.product_name ILIKE ").push_bind(like.clone())
            .push(" OR t.driver_name ILIKE ").push_bind(like)
            .push(")");
    }
    qb
}

/// Users tied to a site only see that site.
fn scope(u: &AuthUser) -> Option<Uuid> {
    if u.0.role == "admin" {
        None
    } else {
        u.0.site_id
    }
}

async fn list(State(st): State<AppState>, u: AuthUser, Query(f): Query<Filter>) -> ApiResult<Value> {
    let sc = scope(&u);
    let mut qb = base_query(
        &format!("SELECT {TICKET_COLS}, s.name AS site_name, st.code AS station_code, sa.id AS sale_id, sa.sale_no, sa.etims_status"),
        &f,
        sc,
    );
    qb.push(" ORDER BY t.created_at DESC LIMIT ").push_bind(f.limit.unwrap_or(100).clamp(1, 1000)).push(" OFFSET ").push_bind(f.offset.unwrap_or(0).max(0));
    let rows: Vec<TicketRow> = qb.build_query_as().fetch_all(&st.db).await?;
    let mut cq = base_query("SELECT count(*)", &f, sc);
    let total: i64 = cq.build_query_scalar().fetch_one(&st.db).await?;
    Ok(Json(json!({ "rows": rows, "total": total })))
}

pub async fn load(db: &sqlx::PgPool, id: Uuid) -> Result<TicketRow, AppError> {
    let none = Filter::default();
    let mut qb = base_query(
        &format!("SELECT {TICKET_COLS}, s.name AS site_name, st.code AS station_code, sa.id AS sale_id, sa.sale_no, sa.etims_status"),
        &none,
        None,
    );
    qb.push(" AND t.id = ").push_bind(id);
    Ok(qb.build_query_as().fetch_optional(db).await?.ok_or(AppError::NotFound)?)
}

async fn get_one(State(st): State<AppState>, u: AuthUser, Path(id): Path<Uuid>) -> ApiResult<TicketRow> {
    let t = load(&st.db, id).await?;
    if let Some(s) = scope(&u) {
        if t.ticket.site_id != Some(s) {
            return Err(AppError::NotFound);
        }
    }
    Ok(Json(t))
}

async fn ticket_text(State(st): State<AppState>, u: AuthUser, Path(id): Path<Uuid>) -> Result<impl IntoResponse, AppError> {
    let t = get_one(State(st.clone()), u, Path(id)).await?.0;
    let site: Option<wb_core::Site> = match t.ticket.site_id {
        Some(s) => sqlx::query_as("SELECT id, code, name, address, phone, ticket_footer FROM sites WHERE id=$1").bind(s).fetch_optional(&st.db).await?,
        None => None,
    };
    Ok(([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], wb_core::render_ticket_text(&t.ticket, site.as_ref(), true)))
}

#[derive(Deserialize)]
struct VoidReq {
    reason: String,
}

async fn void(State(st): State<AppState>, u: AuthUser, Path(id): Path<Uuid>, Json(b): Json<VoidReq>) -> ApiResult<Value> {
    u.require(&["supervisor"])?;
    if b.reason.trim().len() < 3 {
        return Err(bad("Give a reason for voiding"));
    }
    let t = load(&st.db, id).await?;
    if t.sale_id.is_some() && t.etims_status.as_deref() == Some("SIGNED") {
        return Err(bad("This ticket has a KRA-signed invoice. Issue a credit note in eTIMS first, then cancel the sale."));
    }
    // updated_at/synced_at move forward so the station picks the void up on its next pull
    let n = sqlx::query("UPDATE tickets SET status='void', void_reason=$2, updated_at=now(), synced_at=now() WHERE id=$1 AND status <> 'void'")
        .bind(id).bind(b.reason.trim()).execute(&st.db).await?.rows_affected();
    if n == 0 {
        return Err(bad("Ticket is already void"));
    }
    if let Some(sale) = t.sale_id {
        sqlx::query("UPDATE sales SET status='CANCELED', etims_status = CASE WHEN etims_status IN ('PENDING','FAILED') THEN 'DISABLED' ELSE etims_status END, updated_at=now() WHERE id=$1")
            .bind(sale).execute(&st.db).await?;
    }
    audit(&st.db, &u.actor(), "void", "ticket", &id.to_string(), json!({ "reason": b.reason, "ticket_no": t.ticket.ticket_no })).await;
    Ok(Json(json!({ "ok": true })))
}

async fn export_csv(State(st): State<AppState>, u: AuthUser, Query(f): Query<Filter>) -> Result<impl IntoResponse, AppError> {
    let mut qb = base_query(
        &format!("SELECT {TICKET_COLS}, s.name AS site_name, st.code AS station_code, sa.id AS sale_id, sa.sale_no, sa.etims_status"),
        &f,
        scope(&u),
    );
    qb.push(" ORDER BY t.created_at DESC LIMIT 100000");
    let rows: Vec<TicketRow> = qb.build_query_as().fetch_all(&st.db).await?;
    let mut w = csv::Writer::from_writer(vec![]);
    w.write_record([
        "ticket_no", "date", "site", "station", "type", "status", "vehicle", "driver", "customer_supplier", "product", "source", "destination",
        "reference", "first_kg", "first_at", "second_kg", "second_at", "net_kg", "charges", "operator", "invoice", "etims", "memo",
    ]).map_err(anyhow::Error::from)?;
    let dt = |d: Option<DateTime<Utc>>| d.map(|d| d.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string()).unwrap_or_default();
    for r in rows {
        let t = &r.ticket;
        w.write_record([
            t.ticket_no.clone(), dt(Some(t.created_at)), r.site_name.clone().unwrap_or_default(), r.station_code.clone().unwrap_or_default(),
            t.trans_type.clone(), t.status.clone(), t.vehicle_plate.clone(), t.driver_name.clone(), t.party_name.clone(), t.product_name.clone(),
            t.source.clone(), t.destination.clone(), t.reference.clone(), t.first_weight_kg.to_string(), dt(Some(t.first_weight_at)),
            t.second_weight_kg.map(|v| v.to_string()).unwrap_or_default(), dt(t.second_weight_at),
            t.net_weight_kg.map(|v| v.to_string()).unwrap_or_default(), format!("{:.2}", t.charges), t.operator_name.clone(),
            r.sale_no.clone().unwrap_or_default(), r.etims_status.clone().unwrap_or_default(), t.memo.clone(),
        ]).map_err(anyhow::Error::from)?;
    }
    let body = w.into_inner().map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok((
        [(header::CONTENT_TYPE, "text/csv; charset=utf-8"), (header::CONTENT_DISPOSITION, "attachment; filename=\"tickets.csv\"")],
        body,
    ))
}

async fn summary(State(st): State<AppState>, u: AuthUser, Query(f): Query<Filter>) -> ApiResult<Value> {
    let sc = scope(&u).or(f.site_id);
    let from = f.from.unwrap_or_else(|| (Utc::now() - chrono::Duration::days(29)).date_naive());
    let to = f.to.unwrap_or_else(|| Utc::now().date_naive());
    let (a, b) = (from.and_hms_opt(0, 0, 0).unwrap().and_utc(), (to + chrono::Duration::days(1)).and_hms_opt(0, 0, 0).unwrap().and_utc());
    let w = "t.created_at >= $1 AND t.created_at < $2 AND ($3::uuid IS NULL OR t.site_id = $3)";

    let totals = sqlx::query(&format!(
        "SELECT count(*) AS tickets,
                count(*) FILTER (WHERE status='complete') AS complete,
                count(*) FILTER (WHERE status='open') AS open,
                count(*) FILTER (WHERE status='void') AS void,
                COALESCE(sum(net_weight_kg) FILTER (WHERE status='complete'), 0)::float8 AS net_kg,
                COALESCE(sum(charges) FILTER (WHERE status<>'void'), 0)::float8 AS charges,
                count(*) FILTER (WHERE first_manual OR second_manual) AS manual_weights
         FROM tickets t WHERE {w}"
    )).bind(a).bind(b).bind(sc).fetch_all(&st.db).await?;

    let by_day = sqlx::query(&format!(
        "SELECT (t.created_at AT TIME ZONE 'Africa/Nairobi')::date AS day, count(*) AS tickets,
                COALESCE(sum(net_weight_kg) FILTER (WHERE status='complete'),0)::float8 AS net_kg
         FROM tickets t WHERE {w} AND status <> 'void' GROUP BY 1 ORDER BY 1"
    )).bind(a).bind(b).bind(sc).fetch_all(&st.db).await?;

    let by_product = sqlx::query(&format!(
        "SELECT COALESCE(NULLIF(product_name,''),'(none)') AS name, trans_type, count(*) AS tickets, COALESCE(sum(net_weight_kg),0)::float8 AS net_kg
         FROM tickets t WHERE {w} AND status='complete' GROUP BY 1,2 ORDER BY net_kg DESC LIMIT 20"
    )).bind(a).bind(b).bind(sc).fetch_all(&st.db).await?;

    let by_party = sqlx::query(&format!(
        "SELECT COALESCE(NULLIF(party_name,''),'(walk-in)') AS name, count(*) AS tickets, COALESCE(sum(net_weight_kg),0)::float8 AS net_kg
         FROM tickets t WHERE {w} AND status='complete' GROUP BY 1 ORDER BY net_kg DESC LIMIT 20"
    )).bind(a).bind(b).bind(sc).fetch_all(&st.db).await?;

    let sales = sqlx::query(
        "SELECT count(*) AS invoices, COALESCE(sum(total_amount),0)::float8 AS total, COALESCE(sum(tax_amount),0)::float8 AS tax,
                count(*) FILTER (WHERE etims_status='SIGNED') AS signed,
                count(*) FILTER (WHERE etims_status IN ('PENDING','SUBMITTED')) AS pending,
                count(*) FILTER (WHERE etims_status='FAILED') AS failed
         FROM sales WHERE status='APPROVED' AND sale_type='NORMAL' AND sale_date >= $1 AND sale_date < $2 AND ($3::uuid IS NULL OR site_id = $3)",
    ).bind(a).bind(b).bind(sc).fetch_all(&st.db).await?;

    let stations = sqlx::query(
        "SELECT t.code, t.name, s.name AS site_name, t.last_seen_at, t.last_status FROM stations t JOIN sites s ON s.id=t.site_id
         WHERE t.active AND ($1::uuid IS NULL OR t.site_id = $1) ORDER BY t.code",
    ).bind(sc).fetch_all(&st.db).await?;

    Ok(Json(json!({
        "from": from, "to": to,
        "totals": rows_json(&totals).get(0).cloned().unwrap_or(Value::Null),
        "sales": rows_json(&sales).get(0).cloned().unwrap_or(Value::Null),
        "by_day": rows_json(&by_day),
        "by_product": rows_json(&by_product),
        "by_party": rows_json(&by_party),
        "stations": rows_json(&stations),
    })))
}
