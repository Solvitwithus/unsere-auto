//! Local HTTP API for the operator screen (served on localhost).

use crate::app::{meta_get, meta_set, App, Session};
use crate::config::{PrinterConfig, StationScaleConfig};
use crate::sync::TICKET_COLS;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{FromRequestParts, Path, Query, State};
use axum::http::request::Parts;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use chrono::Utc;
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::{Duration, Instant};
use uuid::Uuid;
use wb_core::{normalize_plate, password, render_ticket_text, Party, Product, Site, Ticket, Vehicle};

pub struct ApiError(StatusCode, String);
type R<T> = Result<Json<T>, ApiError>;

fn bad(m: impl Into<String>) -> ApiError {
    ApiError(StatusCode::BAD_REQUEST, m.into())
}
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({ "success": false, "error": self.1 }))).into_response()
    }
}
impl From<sqlx::Error> for ApiError {
    fn from(e: sqlx::Error) -> Self {
        tracing::error!("db: {e}");
        ApiError(StatusCode::INTERNAL_SERVER_ERROR, format!("Local database error: {e}"))
    }
}
impl From<anyhow::Error> for ApiError {
    fn from(e: anyhow::Error) -> Self {
        ApiError(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}"))
    }
}

pub struct User(pub Session);

impl FromRequestParts<App> for User {
    type Rejection = ApiError;
    async fn from_request_parts(parts: &mut Parts, app: &App) -> Result<Self, ApiError> {
        let token = parts
            .headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .map(String::from)
            .or_else(|| parts.uri.query().and_then(|q| q.split('&').find_map(|kv| kv.strip_prefix("token=").map(String::from))));
        token.and_then(|t| app.session(&t)).map(User).ok_or(ApiError(StatusCode::UNAUTHORIZED, "Please sign in".into()))
    }
}

impl axum::extract::OptionalFromRequestParts<App> for User {
    type Rejection = ApiError;
    async fn from_request_parts(parts: &mut Parts, app: &App) -> Result<Option<Self>, ApiError> {
        Ok(<User as FromRequestParts<App>>::from_request_parts(parts, app).await.ok())
    }
}

impl User {
    fn supervisor(&self) -> Result<(), ApiError> {
        if matches!(self.0.role.as_str(), "admin" | "supervisor") {
            Ok(())
        } else {
            Err(ApiError(StatusCode::FORBIDDEN, "A supervisor must do this".into()))
        }
    }
}

pub fn routes() -> Router<App> {
    Router::new()
        .route("/api/station/status", get(status))
        .route("/api/station/setup", post(setup))
        .route("/api/station/printer", put(set_printer))
        .route("/api/auth/login", post(login))
        .route("/api/auth/logout", post(logout))
        .route("/api/auth/me", get(me))
        .route("/ws/weight", get(ws_weight))
        .route("/api/scale/ports", get(ports))
        .route("/api/scale/profiles", get(profiles))
        .route("/api/scale/config", get(get_scale_config).put(put_scale_config))
        .route("/api/scale/detect", post(detect))
        .route("/api/scale/learn", post(learn))
        .route("/api/scale/raw", get(raw))
        .route("/api/masters", get(masters))
        .route("/api/tickets", get(list_tickets).post(first_weight))
        .route("/api/tickets/{id}", get(get_ticket))
        .route("/api/tickets/{id}/second", post(second_weight))
        .route("/api/tickets/{id}/print", post(print_ticket))
        .route("/api/tickets/{id}/text", get(ticket_text))
        .route("/api/tickets/{id}/void", post(void_ticket))
        .route("/api/sync/now", post(sync_now))
}

// ---------------- status / setup ----------------

async fn site(app: &App) -> Option<Site> {
    meta_get(&app.db, "site").await.and_then(|s| serde_json::from_str(&s).ok())
}

async fn status(State(app): State<App>) -> R<Value> {
    let cfg = app.config();
    Ok(Json(json!({
        "mode": "station",
        "version": env!("CARGO_PKG_VERSION"),
        "enrolled": cfg.is_enrolled(),
        "server_url": cfg.server_url,
        "station_code": meta_get(&app.db, "station_code").await,
        "site": site(&app).await,
        "scale_configured": cfg.scale.is_some(),
        "scale": app.scale_state(),
        "allow_manual": cfg.scale.as_ref().map(|s| s.allow_manual).unwrap_or(false),
        "printer": cfg.printer,
        "auto_print": cfg.auto_print,
        "sync": app.sync.lock().unwrap().clone(),
    })))
}

#[derive(Deserialize)]
struct SetupReq {
    server_url: String,
    station_key: String,
}

/// Enrol with the server. Open on first run; afterwards only supervisors may change it.
async fn setup(State(app): State<App>, user: Option<User>, Json(b): Json<SetupReq>) -> R<Value> {
    if app.config().is_enrolled() {
        user.ok_or(ApiError(StatusCode::UNAUTHORIZED, "Please sign in".into()))?.supervisor()?;
    }
    let url = b.server_url.trim().trim_end_matches('/').to_string();
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(bad("Server URL must start with http:// or https://"));
    }
    let key = b.station_key.trim().to_string();
    let old = app.config();
    app.update_config(|c| {
        c.server_url = url.clone();
        c.station_key = key.clone();
    })?;
    // the first pull proves the key works and brings users for login
    sqlx::query("DELETE FROM meta WHERE key = 'last_pull'").execute(&app.db).await?;
    if let Err(e) = crate::sync::pull(&app).await {
        app.update_config(|c| {
            c.server_url = old.server_url.clone();
            c.station_key = old.station_key.clone();
        })?;
        return Err(bad(format!("Could not connect with that server/key: {e:#}")));
    }
    app.sync_now.notify_one();
    Ok(Json(json!({ "ok": true, "station_code": meta_get(&app.db, "station_code").await, "site": site(&app).await })))
}

async fn set_printer(State(app): State<App>, u: User, Json(b): Json<Value>) -> R<Value> {
    u.supervisor()?;
    let printer: PrinterConfig = serde_json::from_value(b.get("printer").cloned().unwrap_or(Value::Null)).map_err(|e| bad(format!("printer: {e}")))?;
    let auto = b.get("auto_print").and_then(|v| v.as_bool()).unwrap_or(false);
    app.update_config(|c| {
        c.printer = printer;
        c.auto_print = auto;
    })?;
    Ok(Json(json!({ "ok": true })))
}

// ---------------- auth ----------------

#[derive(Deserialize)]
struct LoginReq {
    username: String,
    password: String,
}

async fn login(State(app): State<App>, Json(b): Json<LoginReq>) -> R<Value> {
    let row: Option<(Uuid, String, String, String, bool)> =
        sqlx::query_as("SELECT id, full_name, password_hash, role, active FROM users WHERE lower(username) = lower(?)")
            .bind(b.username.trim())
            .fetch_optional(&app.db)
            .await?;
    let ok = row.as_ref().is_some_and(|(_, _, h, _, active)| *active && password::verify(&b.password, h));
    if !ok {
        tokio::time::sleep(Duration::from_millis(400)).await;
        let hint = if row.is_none() && !app.config().is_enrolled() { " (set up the station first)" } else { "" };
        return Err(bad(format!("Wrong username or password{hint}")));
    }
    let (id, name, _, role, _) = row.unwrap();
    use rand::RngCore;
    let mut t = [0u8; 24];
    rand::thread_rng().fill_bytes(&mut t);
    let token = hex::encode(t);
    let s = Session { user_id: id, username: b.username.trim().to_lowercase(), name, role, expires: Instant::now() + Duration::from_secs(14 * 3600) };
    app.sessions.lock().unwrap().insert(token.clone(), s.clone());
    Ok(Json(json!({ "token": token, "user": s })))
}

async fn logout(State(app): State<App>, parts: axum::http::HeaderMap) -> R<Value> {
    if let Some(t) = parts.get("authorization").and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("Bearer ")) {
        app.sessions.lock().unwrap().remove(t);
    }
    Ok(Json(json!({ "ok": true })))
}

async fn me(u: User) -> R<Session> {
    Ok(Json(u.0))
}

// ---------------- live weight ----------------

async fn ws_weight(State(app): State<App>, ws: WebSocketUpgrade) -> impl IntoResponse {
    ws.on_upgrade(move |socket| weight_stream(app, socket))
}

async fn weight_stream(app: App, mut socket: WebSocket) {
    let mut last = String::new();
    let mut since = Instant::now();
    loop {
        let st = app.scale_state();
        let msg = serde_json::to_string(&st).unwrap_or_default();
        // send on change, and at least every 2 s as a heartbeat
        if msg != last || since.elapsed() > Duration::from_secs(2) {
            if socket.send(Message::Text(msg.clone().into())).await.is_err() {
                return;
            }
            last = msg;
            since = Instant::now();
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
}

// ---------------- scale setup ----------------

async fn ports(_u: User) -> R<Vec<String>> {
    Ok(Json(wb_scale::transport::list_serial_ports()))
}

async fn profiles(_u: User) -> R<Vec<wb_scale::Profile>> {
    Ok(Json(wb_scale::builtin_profiles()))
}

async fn get_scale_config(State(app): State<App>, _u: User) -> R<Option<StationScaleConfig>> {
    Ok(Json(app.scale_settings()))
}

async fn put_scale_config(State(app): State<App>, u: User, Json(b): Json<StationScaleConfig>) -> R<Value> {
    u.supervisor()?;
    b.engine_config().map_err(|e| bad(format!("{e:#}")))?;
    app.update_config(|c| c.scale = Some(b))?;
    let a = app.clone();
    tokio::task::spawn_blocking(move || a.restart_scale()).await.ok();
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct DetectReq {
    #[serde(default)]
    ports: Vec<String>,
    #[serde(default)]
    tcp: Option<String>,
}

async fn detect(State(app): State<App>, u: User, Json(b): Json<DetectReq>) -> R<Value> {
    u.supervisor()?;
    let a = app.clone();
    let res = tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<wb_scale::detect::Candidate>> {
        a.stop_scale();
        let r = if let Some(t) = b.tcp.filter(|t| !t.is_empty()) {
            let (h, p) = t.rsplit_once(':').ok_or_else(|| anyhow::anyhow!("TCP address must be host:port"))?;
            wb_scale::detect::detect_tcp(h, p.parse()?, &[], Duration::from_secs(2))
        } else {
            let ports = if b.ports.is_empty() { wb_scale::transport::list_serial_ports() } else { b.ports };
            Ok(wb_scale::detect::detect_serial(&ports, &[], Duration::from_millis(1200), |s| tracing::info!("detect: {s}")))
        };
        a.restart_scale();
        r
    })
    .await
    .map_err(|e| anyhow::anyhow!("{e}"))??;
    Ok(Json(json!({ "candidates": res })))
}

#[derive(Deserialize)]
struct LearnReq {
    weight_kg: f64,
}

async fn learn(State(app): State<App>, u: User, Json(b): Json<LearnReq>) -> R<Value> {
    u.supervisor()?;
    // listen for 2 more seconds to have plenty of frames
    tokio::time::sleep(Duration::from_secs(2)).await;
    let bytes = app.raw_bytes();
    if bytes.is_empty() {
        return Err(bad("No data is arriving from the indicator. Check the port/cable and baud rate first."));
    }
    match wb_scale::detect::learn(&bytes, b.weight_kg) {
        Some(p) => Ok(Json(json!({ "profile": p }))),
        None => Err(bad(format!(
            "Couldn't find {} in the data. Make sure the display is steady and shows exactly that number. Raw: {}",
            b.weight_kg,
            wb_scale::profile::escape(&bytes[bytes.len().saturating_sub(120)..])
        ))),
    }
}

async fn raw(State(app): State<App>, _u: User) -> R<Value> {
    let b = app.raw_bytes();
    Ok(Json(json!({ "raw": wb_scale::profile::escape(&b[b.len().saturating_sub(600)..]), "bytes": b.len() })))
}

// ---------------- masters ----------------

async fn masters(State(app): State<App>, _u: User) -> R<Value> {
    let parties: Vec<Party> = sqlx::query_as("SELECT id, code, name, kind, tin, phone, email, address, active, updated_at FROM parties WHERE active ORDER BY name").fetch_all(&app.db).await?;
    let products: Vec<Product> = sqlx::query_as("SELECT id, code, name, unit_price, quantity_unit, item_code, classification_code, tax_type_code, packaging_unit, active, updated_at FROM products WHERE active ORDER BY name").fetch_all(&app.db).await?;
    let vehicles: Vec<Vehicle> = sqlx::query_as("SELECT id, plate, stored_tare_kg, party_id, driver_name, active, updated_at FROM vehicles WHERE active ORDER BY plate").fetch_all(&app.db).await?;
    Ok(Json(json!({ "parties": parties, "products": products, "vehicles": vehicles })))
}

// ---------------- tickets ----------------

#[derive(Deserialize)]
struct ListQ {
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    q: Option<String>,
}

async fn list_tickets(State(app): State<App>, _u: User, Query(q): Query<ListQ>) -> R<Vec<Ticket>> {
    let like = q.q.as_deref().map(|s| format!("%{}%", s.trim())).unwrap_or_else(|| "%".into());
    let rows: Vec<Ticket> = match q.status.as_deref() {
        Some("open") => sqlx::query_as(&format!("SELECT {TICKET_COLS} FROM tickets WHERE status = 'open' AND (vehicle_plate LIKE ?1 OR ticket_no LIKE ?1) ORDER BY created_at DESC LIMIT 200")).bind(&like).fetch_all(&app.db).await?,
        _ => sqlx::query_as(&format!("SELECT {TICKET_COLS} FROM tickets WHERE (vehicle_plate LIKE ?1 OR ticket_no LIKE ?1 OR party_name LIKE ?1) ORDER BY created_at DESC LIMIT 200")).bind(&like).fetch_all(&app.db).await?,
    };
    Ok(Json(rows))
}

async fn load(app: &App, id: Uuid) -> Result<Ticket, ApiError> {
    sqlx::query_as(&format!("SELECT {TICKET_COLS} FROM tickets WHERE id = ?"))
        .bind(id)
        .fetch_optional(&app.db)
        .await?
        .ok_or(ApiError(StatusCode::NOT_FOUND, "Ticket not found".into()))
}

async fn get_ticket(State(app): State<App>, _u: User, Path(id): Path<Uuid>) -> R<Ticket> {
    Ok(Json(load(&app, id).await?))
}

/// Decide the weight to record: the live stable reading, or a supervisor's manual entry.
fn take_weight(app: &App, u: &User, manual: Option<f64>) -> Result<(f64, String, bool), ApiError> {
    let sc = app.scale_settings();
    let max = sc.as_ref().map(|s| s.max_capacity_kg).unwrap_or(100_000.0);
    let min = sc.as_ref().map(|s| s.min_weight_kg).unwrap_or(50.0);
    if let Some(w) = manual {
        if !sc.as_ref().is_some_and(|s| s.allow_manual) {
            return Err(bad("Manual weights are switched off for this station"));
        }
        u.supervisor()?;
        if !(w > 0.0 && w <= max) {
            return Err(bad("Manual weight is out of range"));
        }
        return Ok((w, format!("manual entry by {}", u.0.username), true));
    }
    // Normal case: the weight comes from the live reading held by the scale
    // engine, never from the browser. It must be fresh, stable, above the
    // empty-deck threshold and below capacity.
    let st = app.scale_state();
    let r = st.reading.filter(|_| st.receiving).ok_or_else(|| bad("No reading from the scale. Check the indicator and cable."))?;
    if !r.stable {
        return Err(bad("The weight is not stable yet. Wait for the truck to stop moving."));
    }
    if r.weight_kg < min {
        return Err(bad(format!("The scale reads {} kg - is the truck on the deck?", r.weight_kg)));
    }
    if r.weight_kg > max {
        return Err(bad("The weight is over the scale capacity"));
    }
    Ok((r.weight_kg, r.raw, false))
}

#[derive(Deserialize)]
struct FirstReq {
    trans_type: String,
    vehicle_plate: String,
    #[serde(default)]
    driver_name: String,
    #[serde(default)]
    party_id: Option<Uuid>,
    #[serde(default)]
    party_name: String,
    #[serde(default)]
    product_id: Option<Uuid>,
    #[serde(default)]
    source: String,
    #[serde(default)]
    destination: String,
    #[serde(default)]
    reference: String,
    #[serde(default)]
    memo: String,
    #[serde(default)]
    charges: Option<f64>,
    #[serde(default)]
    manual_weight_kg: Option<f64>,
    /// Single weighing: use the vehicle's stored tare as the other weight
    #[serde(default)]
    use_stored_tare: bool,
}

async fn first_weight(State(app): State<App>, u: User, Json(b): Json<FirstReq>) -> R<Ticket> {
    if !wb_core::TRANS_TYPES.contains(&b.trans_type.as_str()) {
        return Err(bad("Choose purchase, sale, transfer or other"));
    }
    let plate = normalize_plate(&b.vehicle_plate);
    if plate.len() < 3 {
        return Err(bad("Enter the vehicle number plate"));
    }
    // One open ticket per truck: stops a truck being weighed in twice
    // (the classic way to lose track of a load).
    let open: Option<String> = sqlx::query_scalar("SELECT ticket_no FROM tickets WHERE vehicle_plate = ? AND status = 'open'").bind(&plate).fetch_optional(&app.db).await?;
    if let Some(no) = open {
        return Err(bad(format!("{plate} already has an open ticket ({no}). Take its second weight instead.")));
    }
    let (weight, raw, manual) = take_weight(&app, &u, b.manual_weight_kg)?;
    let site = site(&app).await;
    let site_name = site.as_ref().map(|s| s.name.clone()).unwrap_or_default();
    let station_code = meta_get(&app.db, "station_code").await.filter(|s| !s.is_empty()).unwrap_or_else(|| "WB".into());
    let station_id = meta_get(&app.db, "station_id").await.and_then(|s| Uuid::parse_str(&s).ok());

    let party_name = match b.party_id {
        Some(p) => sqlx::query_scalar("SELECT name FROM parties WHERE id = ?").bind(p).fetch_optional(&app.db).await?.unwrap_or(b.party_name.clone()),
        None => b.party_name.trim().to_string(),
    };
    let product_name: String = match b.product_id {
        Some(p) => sqlx::query_scalar("SELECT name FROM products WHERE id = ?").bind(p).fetch_optional(&app.db).await?.unwrap_or_default(),
        None => String::new(),
    };
    // the old app defaulted the own-company side of the movement
    let mut source = b.source.trim().to_string();
    let mut destination = b.destination.trim().to_string();
    match b.trans_type.as_str() {
        "sale" if source.is_empty() => source = site_name.clone(),
        "purchase" if destination.is_empty() => destination = site_name.clone(),
        _ => {}
    }
    if b.trans_type == "sale" && destination.is_empty() {
        destination = party_name.clone();
    }
    if b.trans_type == "purchase" && source.is_empty() {
        source = party_name.clone();
    }

    let stored_tare: Option<f64> = if b.use_stored_tare {
        let t: Option<Option<f64>> = sqlx::query_scalar("SELECT stored_tare_kg FROM vehicles WHERE plate = ?").bind(&plate).fetch_optional(&app.db).await?;
        Some(t.flatten().ok_or_else(|| bad(format!("{plate} has no stored tare yet. Weigh it empty once.")))?)
    } else {
        None
    };

    // Ticket numbers are per station (DEM1-000001, ...) so stations can
    // number tickets offline without ever clashing with each other.
    let now = Utc::now();
    let mut tx = app.db.begin().await?;
    let n: i64 = meta_get(&app.db, "ticket_seq").await.and_then(|s| s.parse().ok()).unwrap_or(0) + 1;
    meta_set(&mut *tx, "ticket_seq", &n.to_string()).await?;
    let fee = b.charges.unwrap_or(0.0);
    let mut t = Ticket {
        id: Uuid::new_v4(),
        ticket_no: format!("{station_code}-{n:06}"),
        site_id: site.as_ref().map(|s| s.id),
        station_id,
        trans_type: b.trans_type.clone(),
        status: "open".into(),
        vehicle_plate: plate.clone(),
        driver_name: b.driver_name.trim().to_string(),
        party_id: b.party_id,
        party_name,
        product_id: b.product_id,
        product_name,
        source,
        destination,
        reference: b.reference.trim().to_string(),
        memo: b.memo.trim().to_string(),
        first_weight_kg: weight,
        first_weight_at: now,
        first_raw: raw,
        first_manual: manual,
        second_weight_kg: None,
        second_weight_at: None,
        second_raw: String::new(),
        second_manual: false,
        net_weight_kg: None,
        charges: fee,
        operator_id: Some(u.0.user_id),
        operator_name: u.0.name.clone(),
        void_reason: String::new(),
        print_count: 0,
        created_at: now,
        updated_at: now,
    };
    if let Some(tare) = stored_tare {
        // single pass: the stored tare is the first weighing, the live reading the second
        t.second_weight_kg = Some(t.first_weight_kg);
        t.second_weight_at = Some(now);
        t.second_raw = std::mem::take(&mut t.first_raw);
        t.second_manual = t.first_manual;
        t.first_weight_kg = tare;
        t.first_raw = "stored tare".into();
        t.first_manual = false;
        t.status = "complete".into();
        t.net_weight_kg = t.net();
    }
    insert(&mut tx, &t).await?;
    sqlx::query("INSERT OR IGNORE INTO vehicles (id, plate, stored_tare_kg, party_id, driver_name, active, updated_at) VALUES (?,?,NULL,NULL,?,1,?)")
        .bind(Uuid::new_v4()).bind(&plate).bind(&t.driver_name).bind(now).execute(&mut *tx).await?;
    tx.commit().await?;
    app.sync_now.notify_one();
    if t.status == "complete" && app.config().auto_print {
        let _ = do_print(&app, &t, false).await;
    }
    Ok(Json(t))
}

async fn insert(tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>, t: &Ticket) -> sqlx::Result<()> {
    sqlx::query(&format!("INSERT INTO tickets ({TICKET_COLS}, dirty) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,1)"))
        .bind(t.id).bind(&t.ticket_no).bind(t.site_id).bind(t.station_id).bind(&t.trans_type).bind(&t.status).bind(&t.vehicle_plate)
        .bind(&t.driver_name).bind(t.party_id).bind(&t.party_name).bind(t.product_id).bind(&t.product_name).bind(&t.source)
        .bind(&t.destination).bind(&t.reference).bind(&t.memo).bind(t.first_weight_kg).bind(t.first_weight_at).bind(&t.first_raw)
        .bind(t.first_manual).bind(t.second_weight_kg).bind(t.second_weight_at).bind(&t.second_raw).bind(t.second_manual)
        .bind(t.net_weight_kg).bind(t.charges).bind(t.operator_id).bind(&t.operator_name).bind(&t.void_reason).bind(t.print_count)
        .bind(t.created_at).bind(t.updated_at)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

#[derive(Deserialize, Default)]
struct SecondReq {
    #[serde(default)]
    manual_weight_kg: Option<f64>,
    #[serde(default)]
    charges: Option<f64>,
    #[serde(default)]
    memo: Option<String>,
}

async fn second_weight(State(app): State<App>, u: User, Path(id): Path<Uuid>, body: Option<Json<SecondReq>>) -> R<Ticket> {
    let b = body.map(|b| b.0).unwrap_or_default();
    let mut t = load(&app, id).await?;
    if t.status != "open" {
        return Err(bad(format!("Ticket {} is {}, not open", t.ticket_no, t.status)));
    }
    let (w, raw, manual) = take_weight(&app, &u, b.manual_weight_kg)?;
    let now = Utc::now();
    t.second_weight_kg = Some(w);
    t.second_weight_at = Some(now);
    t.second_raw = raw;
    t.second_manual = manual;
    t.status = "complete".into();
    t.net_weight_kg = t.net();
    if let Some(c) = b.charges {
        t.charges = c;
    }
    if let Some(m) = b.memo.filter(|m| !m.trim().is_empty()) {
        t.memo = m.trim().to_string();
    }
    t.updated_at = now;
    sqlx::query("UPDATE tickets SET second_weight_kg=?, second_weight_at=?, second_raw=?, second_manual=?, status='complete', net_weight_kg=?, charges=?, memo=?, updated_at=?, dirty=1 WHERE id=? AND status='open'")
        .bind(w).bind(now).bind(&t.second_raw).bind(manual).bind(t.net_weight_kg).bind(t.charges).bind(&t.memo).bind(now).bind(id)
        .execute(&app.db).await?;
    // remember the empty weight for single-pass weighing next time
    let tare = w.min(t.first_weight_kg);
    sqlx::query("UPDATE vehicles SET stored_tare_kg = ? WHERE plate = ?").bind(tare).bind(&t.vehicle_plate).execute(&app.db).await?;
    app.sync_now.notify_one();
    if app.config().auto_print {
        if let Err(e) = do_print(&app, &t, false).await {
            tracing::warn!("auto print failed: {e:#}");
        }
    }
    Ok(Json(t))
}

async fn do_print(app: &App, t: &Ticket, reprint: bool) -> anyhow::Result<bool> {
    let text = render_ticket_text(t, site(app).await.as_ref(), reprint);
    let cfg = app.config().printer;
    let printed = tokio::task::spawn_blocking(move || crate::print::print(&cfg, &text)).await??;
    sqlx::query("UPDATE tickets SET print_count = print_count + 1, updated_at = ?, dirty = 1 WHERE id = ?").bind(Utc::now()).bind(t.id).execute(&app.db).await?;
    Ok(printed)
}

async fn print_ticket(State(app): State<App>, _u: User, Path(id): Path<Uuid>) -> R<Value> {
    let t = load(&app, id).await?;
    let printed = do_print(&app, &t, t.print_count > 0).await.map_err(|e| bad(format!("Printer: {e:#}")))?;
    Ok(Json(json!({ "printed": printed, "browser_print": !printed })))
}

async fn ticket_text(State(app): State<App>, _u: User, Path(id): Path<Uuid>) -> Result<impl IntoResponse, ApiError> {
    let t = load(&app, id).await?;
    let text = render_ticket_text(&t, site(&app).await.as_ref(), t.print_count > 1);
    Ok(([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], text))
}

#[derive(Deserialize)]
struct VoidReq {
    reason: String,
}

async fn void_ticket(State(app): State<App>, u: User, Path(id): Path<Uuid>, Json(b): Json<VoidReq>) -> R<Value> {
    u.supervisor()?;
    if b.reason.trim().len() < 3 {
        return Err(bad("Give a reason"));
    }
    let n = sqlx::query("UPDATE tickets SET status='void', void_reason=?, updated_at=?, dirty=1 WHERE id=? AND status <> 'void'")
        .bind(format!("{} ({})", b.reason.trim(), u.0.username)).bind(Utc::now()).bind(id)
        .execute(&app.db).await?.rows_affected();
    if n == 0 {
        return Err(bad("Ticket not found or already void"));
    }
    app.sync_now.notify_one();
    Ok(Json(json!({ "ok": true })))
}

async fn sync_now(State(app): State<App>, _u: User) -> R<Value> {
    crate::sync::run_once(&app).await.map_err(|e| bad(format!("Sync failed: {e:#}")))?;
    Ok(Json(json!({ "ok": true, "sync": app.sync.lock().unwrap().clone() })))
}
