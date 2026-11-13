//! Auth, sites, users, parties (customers/suppliers), products, vehicles, stations.

use crate::auth::{issue, new_station_key, AuthUser, Claims};
use crate::error::{bad, ApiResult, AppError};
use crate::state::{audit, AppState};
use axum::extract::{Path, Query, State};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;
use wb_core::{normalize_plate, password, Party, Product, Vehicle};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/auth/login", post(login))
        .route("/api/auth/me", get(me))
        .route("/api/auth/password", post(change_password))
        .route("/api/sites", get(list_sites).post(create_site))
        .route("/api/sites/{id}", put(update_site))
        .route("/api/users", get(list_users).post(create_user))
        .route("/api/users/{id}", put(update_user))
        .route("/api/parties", get(list_parties).post(create_party))
        .route("/api/parties/{id}", put(update_party))
        .route("/api/products", get(list_products).post(create_product))
        .route("/api/products/{id}", put(update_product))
        .route("/api/vehicles", get(list_vehicles).post(create_vehicle))
        .route("/api/vehicles/{id}", put(update_vehicle))
        .route("/api/stations", get(list_stations).post(create_station))
        .route("/api/stations/{id}", put(update_station))
        .route("/api/stations/{id}/rotate-key", post(rotate_station_key))
        .route("/api/audit", get(list_audit))
}

fn req(s: &str, field: &str) -> Result<String, AppError> {
    let t = s.trim();
    if t.is_empty() {
        Err(bad(format!("{field} is required")))
    } else {
        Ok(t.to_string())
    }
}

fn opt(s: &Option<String>) -> Option<String> {
    s.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(String::from)
}

// ---------------- auth ----------------

#[derive(Deserialize)]
struct LoginReq {
    username: String,
    password: String,
}

async fn login(State(st): State<AppState>, Json(b): Json<LoginReq>) -> ApiResult<Value> {
    let row: Option<(Uuid, String, String, String, Option<Uuid>, bool)> =
        sqlx::query_as("SELECT id, full_name, password_hash, role, site_id, active FROM users WHERE lower(username) = lower($1)")
            .bind(b.username.trim())
            .fetch_optional(&st.db)
            .await?;
    let Some((id, name, hash, role, site_id, active)) = row else {
        return Err(bad("Wrong username or password"));
    };
    if !active || !password::verify(&b.password, &hash) {
        audit(&st.db, b.username.trim(), "login_failed", "user", &id.to_string(), json!({})).await;
        return Err(bad("Wrong username or password"));
    }
    let claims = Claims {
        sub: id,
        username: b.username.trim().to_lowercase(),
        name: name.clone(),
        role: role.clone(),
        site_id,
        exp: (chrono::Utc::now() + chrono::Duration::hours(12)).timestamp() as usize,
    };
    let token = issue(&st.jwt_secret, &claims)?;
    audit(&st.db, &claims.username, "login", "user", &id.to_string(), json!({})).await;
    Ok(Json(json!({ "token": token, "user": { "id": id, "username": claims.username, "name": name, "role": role, "site_id": site_id } })))
}

async fn me(u: AuthUser) -> ApiResult<Value> {
    Ok(Json(json!({ "id": u.0.sub, "username": u.0.username, "name": u.0.name, "role": u.0.role, "site_id": u.0.site_id })))
}

#[derive(Deserialize)]
struct PwReq {
    current: String,
    new: String,
}

async fn change_password(State(st): State<AppState>, u: AuthUser, Json(b): Json<PwReq>) -> ApiResult<Value> {
    if b.new.len() < 8 {
        return Err(bad("New password must be at least 8 characters"));
    }
    let hash: String = sqlx::query_scalar("SELECT password_hash FROM users WHERE id = $1").bind(u.0.sub).fetch_one(&st.db).await?;
    if !password::verify(&b.current, &hash) {
        return Err(bad("Current password is wrong"));
    }
    sqlx::query("UPDATE users SET password_hash = $2, updated_at = now() WHERE id = $1").bind(u.0.sub).bind(password::hash(&b.new)).execute(&st.db).await?;
    audit(&st.db, &u.actor(), "password_changed", "user", &u.0.sub.to_string(), json!({})).await;
    Ok(Json(json!({ "ok": true })))
}

// ---------------- sites ----------------

#[derive(Deserialize)]
struct SiteIn {
    code: String,
    name: String,
    #[serde(default)]
    address: String,
    #[serde(default)]
    phone: String,
    #[serde(default)]
    ticket_footer: Option<String>,
    #[serde(default)]
    kra_pin: String,
    #[serde(default)]
    auto_invoice: bool,
    #[serde(default = "yes")]
    prices_include_tax: bool,
    #[serde(default)]
    weighing_fee: f64,
}
fn yes() -> bool {
    true
}

const SITE_COLS: &str = "id, code, name, address, phone, ticket_footer, kra_pin, auto_invoice, prices_include_tax, weighing_fee::float8 AS weighing_fee, created_at";

async fn list_sites(State(st): State<AppState>, _u: AuthUser) -> ApiResult<Value> {
    let rows = sqlx::query(&format!("SELECT {SITE_COLS} FROM sites ORDER BY name")).fetch_all(&st.db).await?;
    Ok(Json(rows_json(&rows)))
}

async fn create_site(State(st): State<AppState>, u: AuthUser, Json(b): Json<SiteIn>) -> ApiResult<Value> {
    u.require(&[])?;
    let id = Uuid::new_v4();
    let code = req(&b.code, "Code")?.to_uppercase();
    if code.len() > 10 || !code.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err(bad("Site code: up to 10 letters/digits"));
    }
    sqlx::query("INSERT INTO sites (id, code, name, address, phone, ticket_footer, kra_pin, auto_invoice, prices_include_tax, weighing_fee) VALUES ($1,$2,$3,$4,$5,COALESCE($6,'Thank you'),$7,$8,$9,$10)")
        .bind(id).bind(&code).bind(req(&b.name, "Name")?).bind(&b.address).bind(&b.phone).bind(&b.ticket_footer)
        .bind(b.kra_pin.trim().to_uppercase()).bind(b.auto_invoice).bind(b.prices_include_tax).bind(b.weighing_fee)
        .execute(&st.db).await?;
    audit(&st.db, &u.actor(), "create", "site", &id.to_string(), json!({ "code": code })).await;
    Ok(Json(json!({ "id": id })))
}

async fn update_site(State(st): State<AppState>, u: AuthUser, Path(id): Path<Uuid>, Json(b): Json<SiteIn>) -> ApiResult<Value> {
    u.require(&[])?;
    let n = sqlx::query("UPDATE sites SET code=$2, name=$3, address=$4, phone=$5, ticket_footer=COALESCE($6, ticket_footer), kra_pin=$7, auto_invoice=$8, prices_include_tax=$9, weighing_fee=$10, updated_at=now() WHERE id=$1")
        .bind(id).bind(req(&b.code, "Code")?.to_uppercase()).bind(req(&b.name, "Name")?).bind(&b.address).bind(&b.phone).bind(&b.ticket_footer)
        .bind(b.kra_pin.trim().to_uppercase()).bind(b.auto_invoice).bind(b.prices_include_tax).bind(b.weighing_fee)
        .execute(&st.db).await?.rows_affected();
    if n == 0 {
        return Err(AppError::NotFound);
    }
    audit(&st.db, &u.actor(), "update", "site", &id.to_string(), json!({})).await;
    Ok(Json(json!({ "ok": true })))
}

// ---------------- users ----------------

#[derive(Deserialize)]
struct UserIn {
    username: String,
    #[serde(default)]
    full_name: String,
    #[serde(default)]
    password: Option<String>,
    role: String,
    #[serde(default)]
    site_id: Option<Uuid>,
    #[serde(default = "yes")]
    active: bool,
}

async fn list_users(State(st): State<AppState>, u: AuthUser) -> ApiResult<Value> {
    u.require(&["supervisor"])?;
    let rows = sqlx::query("SELECT u.id, u.username, u.full_name, u.role, u.site_id, s.name AS site_name, u.active, u.created_at FROM users u LEFT JOIN sites s ON s.id = u.site_id ORDER BY u.username")
        .fetch_all(&st.db).await?;
    Ok(Json(rows_json(&rows)))
}

fn check_role(r: &str) -> Result<(), AppError> {
    if wb_core::ROLES.contains(&r) {
        Ok(())
    } else {
        Err(bad(format!("Role must be one of {}", wb_core::ROLES.join(", "))))
    }
}

async fn create_user(State(st): State<AppState>, u: AuthUser, Json(b): Json<UserIn>) -> ApiResult<Value> {
    u.require(&[])?;
    check_role(&b.role)?;
    let pw = b.password.clone().unwrap_or_default();
    if pw.len() < 8 {
        return Err(bad("Password must be at least 8 characters"));
    }
    let id = Uuid::new_v4();
    let username = req(&b.username, "Username")?.to_lowercase();
    sqlx::query("INSERT INTO users (id, username, full_name, password_hash, role, site_id, active) VALUES ($1,$2,$3,$4,$5,$6,$7)")
        .bind(id).bind(&username).bind(b.full_name.trim()).bind(password::hash(&pw)).bind(&b.role).bind(b.site_id).bind(b.active)
        .execute(&st.db).await?;
    audit(&st.db, &u.actor(), "create", "user", &id.to_string(), json!({ "username": username, "role": b.role })).await;
    Ok(Json(json!({ "id": id })))
}

async fn update_user(State(st): State<AppState>, u: AuthUser, Path(id): Path<Uuid>, Json(b): Json<UserIn>) -> ApiResult<Value> {
    u.require(&[])?;
    check_role(&b.role)?;
    if id == u.0.sub && (!b.active || b.role != "admin") {
        return Err(bad("You can't remove your own admin access"));
    }
    let mut tx = st.db.begin().await?;
    let n = sqlx::query("UPDATE users SET username=$2, full_name=$3, role=$4, site_id=$5, active=$6, updated_at=now() WHERE id=$1")
        .bind(id).bind(req(&b.username, "Username")?.to_lowercase()).bind(b.full_name.trim()).bind(&b.role).bind(b.site_id).bind(b.active)
        .execute(&mut *tx).await?.rows_affected();
    if n == 0 {
        return Err(AppError::NotFound);
    }
    if let Some(pw) = opt(&b.password) {
        if pw.len() < 8 {
            return Err(bad("Password must be at least 8 characters"));
        }
        sqlx::query("UPDATE users SET password_hash=$2 WHERE id=$1").bind(id).bind(password::hash(&pw)).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    audit(&st.db, &u.actor(), "update", "user", &id.to_string(), json!({ "role": b.role, "active": b.active })).await;
    Ok(Json(json!({ "ok": true })))
}

// ---------------- parties ----------------

#[derive(Deserialize)]
struct ListQ {
    #[serde(default)]
    q: Option<String>,
    #[serde(default)]
    all: bool,
}

#[derive(Deserialize)]
struct PartyIn {
    #[serde(default)]
    code: Option<String>,
    name: String,
    #[serde(default = "customer")]
    kind: String,
    #[serde(default)]
    tin: Option<String>,
    #[serde(default)]
    phone: String,
    #[serde(default)]
    email: String,
    #[serde(default)]
    address: String,
    #[serde(default = "yes")]
    active: bool,
}
fn customer() -> String {
    "customer".into()
}

const PARTY_COLS: &str = "id, code, name, kind, tin, phone, email, address, active, updated_at";

async fn list_parties(State(st): State<AppState>, _u: AuthUser, Query(q): Query<ListQ>) -> ApiResult<Vec<Party>> {
    let rows = sqlx::query_as::<_, Party>(&format!(
        "SELECT {PARTY_COLS} FROM parties WHERE ($1::text IS NULL OR name ILIKE '%'||$1||'%' OR code ILIKE '%'||$1||'%') AND ($2 OR active) ORDER BY name LIMIT 1000"
    ))
    .bind(opt(&q.q)).bind(q.all).fetch_all(&st.db).await?;
    Ok(Json(rows))
}

fn check_tin(t: &Option<String>) -> Result<Option<String>, AppError> {
    match opt(t) {
        None => Ok(None),
        Some(t) => {
            let t = t.to_uppercase();
            // KRA PIN: letter, 9 digits, letter (e.g. P051234567X)
            let b = t.as_bytes();
            if b.len() == 11 && b[0].is_ascii_alphabetic() && b[10].is_ascii_alphabetic() && b[1..10].iter().all(u8::is_ascii_digit) {
                Ok(Some(t))
            } else {
                Err(bad("KRA PIN should look like P051234567X"))
            }
        }
    }
}

async fn create_party(State(st): State<AppState>, u: AuthUser, Json(b): Json<PartyIn>) -> ApiResult<Value> {
    u.require(&["supervisor"])?;
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO parties (id, code, name, kind, tin, phone, email, address, active) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)")
        .bind(id).bind(opt(&b.code)).bind(req(&b.name, "Name")?).bind(&b.kind).bind(check_tin(&b.tin)?).bind(b.phone.trim()).bind(b.email.trim()).bind(b.address.trim()).bind(b.active)
        .execute(&st.db).await?;
    audit(&st.db, &u.actor(), "create", "party", &id.to_string(), json!({ "name": b.name })).await;
    Ok(Json(json!({ "id": id })))
}

async fn update_party(State(st): State<AppState>, u: AuthUser, Path(id): Path<Uuid>, Json(b): Json<PartyIn>) -> ApiResult<Value> {
    u.require(&["supervisor"])?;
    let n = sqlx::query("UPDATE parties SET code=$2, name=$3, kind=$4, tin=$5, phone=$6, email=$7, address=$8, active=$9, updated_at=now() WHERE id=$1")
        .bind(id).bind(opt(&b.code)).bind(req(&b.name, "Name")?).bind(&b.kind).bind(check_tin(&b.tin)?).bind(b.phone.trim()).bind(b.email.trim()).bind(b.address.trim()).bind(b.active)
        .execute(&st.db).await?.rows_affected();
    if n == 0 {
        return Err(AppError::NotFound);
    }
    audit(&st.db, &u.actor(), "update", "party", &id.to_string(), json!({})).await;
    Ok(Json(json!({ "ok": true })))
}

// ---------------- products ----------------

#[derive(Deserialize)]
struct ProductIn {
    #[serde(default)]
    code: Option<String>,
    name: String,
    #[serde(default)]
    unit_price: f64,
    #[serde(default = "tne")]
    quantity_unit: String,
    #[serde(default)]
    item_code: Option<String>,
    #[serde(default)]
    classification_code: String,
    #[serde(default = "tax_b")]
    tax_type_code: String,
    #[serde(default = "nt")]
    packaging_unit: String,
    #[serde(default = "yes")]
    active: bool,
}
fn tne() -> String {
    "TNE".into()
}
fn tax_b() -> String {
    "B".into()
}
fn nt() -> String {
    "NT".into()
}

pub const PRODUCT_COLS: &str = "id, code, name, unit_price::float8 AS unit_price, quantity_unit, item_code, classification_code, tax_type_code, packaging_unit, active, updated_at";

async fn list_products(State(st): State<AppState>, _u: AuthUser, Query(q): Query<ListQ>) -> ApiResult<Vec<Product>> {
    let rows = sqlx::query_as::<_, Product>(&format!(
        "SELECT {PRODUCT_COLS} FROM products WHERE ($1::text IS NULL OR name ILIKE '%'||$1||'%' OR code ILIKE '%'||$1||'%') AND ($2 OR active) ORDER BY name"
    ))
    .bind(opt(&q.q)).bind(q.all).fetch_all(&st.db).await?;
    Ok(Json(rows))
}

fn check_product(b: &ProductIn) -> Result<(), AppError> {
    if !["A", "B", "C", "D", "E"].contains(&b.tax_type_code.as_str()) {
        return Err(bad("Tax type must be A, B, C, D or E"));
    }
    if b.quantity_unit.is_empty() || b.quantity_unit.len() > 5 || b.packaging_unit.len() > 5 {
        return Err(bad("Units are KRA codes of up to 5 characters (TNE, KG, U, NT...)"));
    }
    if b.unit_price < 0.0 {
        return Err(bad("Price can't be negative"));
    }
    Ok(())
}

async fn create_product(State(st): State<AppState>, u: AuthUser, Json(b): Json<ProductIn>) -> ApiResult<Value> {
    u.require(&["supervisor"])?;
    check_product(&b)?;
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO products (id, code, name, unit_price, quantity_unit, item_code, classification_code, tax_type_code, packaging_unit, active) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)")
        .bind(id).bind(opt(&b.code)).bind(req(&b.name, "Name")?).bind(b.unit_price).bind(b.quantity_unit.to_uppercase()).bind(opt(&b.item_code))
        .bind(b.classification_code.trim()).bind(&b.tax_type_code).bind(b.packaging_unit.to_uppercase()).bind(b.active)
        .execute(&st.db).await?;
    audit(&st.db, &u.actor(), "create", "product", &id.to_string(), json!({ "name": b.name })).await;
    Ok(Json(json!({ "id": id })))
}

async fn update_product(State(st): State<AppState>, u: AuthUser, Path(id): Path<Uuid>, Json(b): Json<ProductIn>) -> ApiResult<Value> {
    u.require(&["supervisor"])?;
    check_product(&b)?;
    let n = sqlx::query("UPDATE products SET code=$2, name=$3, unit_price=$4, quantity_unit=$5, item_code=$6, classification_code=$7, tax_type_code=$8, packaging_unit=$9, active=$10, updated_at=now() WHERE id=$1")
        .bind(id).bind(opt(&b.code)).bind(req(&b.name, "Name")?).bind(b.unit_price).bind(b.quantity_unit.to_uppercase()).bind(opt(&b.item_code))
        .bind(b.classification_code.trim()).bind(&b.tax_type_code).bind(b.packaging_unit.to_uppercase()).bind(b.active)
        .execute(&st.db).await?.rows_affected();
    if n == 0 {
        return Err(AppError::NotFound);
    }
    audit(&st.db, &u.actor(), "update", "product", &id.to_string(), json!({ "unit_price": b.unit_price })).await;
    Ok(Json(json!({ "ok": true })))
}

// ---------------- vehicles ----------------

#[derive(Deserialize)]
struct VehicleIn {
    plate: String,
    #[serde(default)]
    stored_tare_kg: Option<f64>,
    #[serde(default)]
    party_id: Option<Uuid>,
    #[serde(default)]
    driver_name: String,
    #[serde(default = "yes")]
    active: bool,
}

const VEHICLE_COLS: &str = "id, plate, stored_tare_kg, party_id, driver_name, active, updated_at";

async fn list_vehicles(State(st): State<AppState>, _u: AuthUser, Query(q): Query<ListQ>) -> ApiResult<Vec<Vehicle>> {
    let rows = sqlx::query_as::<_, Vehicle>(&format!(
        "SELECT {VEHICLE_COLS} FROM vehicles WHERE ($1::text IS NULL OR plate ILIKE '%'||$1||'%' OR driver_name ILIKE '%'||$1||'%') AND ($2 OR active) ORDER BY plate LIMIT 2000"
    ))
    .bind(opt(&q.q).map(|s| s.to_uppercase())).bind(q.all).fetch_all(&st.db).await?;
    Ok(Json(rows))
}

async fn create_vehicle(State(st): State<AppState>, u: AuthUser, Json(b): Json<VehicleIn>) -> ApiResult<Value> {
    u.require(&["supervisor", "operator"])?;
    let plate = normalize_plate(&b.plate);
    if plate.len() < 3 {
        return Err(bad("Enter a valid number plate"));
    }
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO vehicles (id, plate, stored_tare_kg, tare_updated_at, party_id, driver_name, active) VALUES ($1,$2,$3,CASE WHEN $3 IS NULL THEN NULL ELSE now() END,$4,$5,$6)")
        .bind(id).bind(&plate).bind(b.stored_tare_kg).bind(b.party_id).bind(b.driver_name.trim()).bind(b.active)
        .execute(&st.db).await?;
    audit(&st.db, &u.actor(), "create", "vehicle", &id.to_string(), json!({ "plate": plate })).await;
    Ok(Json(json!({ "id": id })))
}

async fn update_vehicle(State(st): State<AppState>, u: AuthUser, Path(id): Path<Uuid>, Json(b): Json<VehicleIn>) -> ApiResult<Value> {
    u.require(&["supervisor"])?;
    let n = sqlx::query("UPDATE vehicles SET plate=$2, tare_updated_at = CASE WHEN stored_tare_kg IS DISTINCT FROM $3 THEN now() ELSE tare_updated_at END, stored_tare_kg=$3, party_id=$4, driver_name=$5, active=$6, updated_at=now() WHERE id=$1")
        .bind(id).bind(normalize_plate(&b.plate)).bind(b.stored_tare_kg).bind(b.party_id).bind(b.driver_name.trim()).bind(b.active)
        .execute(&st.db).await?.rows_affected();
    if n == 0 {
        return Err(AppError::NotFound);
    }
    audit(&st.db, &u.actor(), "update", "vehicle", &id.to_string(), json!({ "stored_tare_kg": b.stored_tare_kg })).await;
    Ok(Json(json!({ "ok": true })))
}

// ---------------- stations ----------------

#[derive(Deserialize)]
struct StationIn {
    site_id: Uuid,
    code: String,
    name: String,
    #[serde(default)]
    scale_config: Option<Value>,
    #[serde(default = "yes")]
    active: bool,
}

async fn list_stations(State(st): State<AppState>, u: AuthUser) -> ApiResult<Value> {
    u.require(&["supervisor"])?;
    let rows = sqlx::query(
        "SELECT t.id, t.site_id, s.name AS site_name, t.code, t.name, t.scale_config, t.last_seen_at, t.last_status, t.active, t.created_at,
                (SELECT count(*) FROM tickets k WHERE k.station_id = t.id AND k.created_at > now() - interval '1 day') AS tickets_today
         FROM stations t JOIN sites s ON s.id = t.site_id ORDER BY s.name, t.code",
    )
    .fetch_all(&st.db).await?;
    Ok(Json(rows_json(&rows)))
}

async fn create_station(State(st): State<AppState>, u: AuthUser, Json(b): Json<StationIn>) -> ApiResult<Value> {
    u.require(&[])?;
    let id = Uuid::new_v4();
    let code = req(&b.code, "Code")?.to_uppercase();
    if code.len() > 10 || !code.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err(bad("Station code: up to 10 letters/digits (it prefixes ticket numbers)"));
    }
    let (key, hash) = new_station_key(id);
    sqlx::query("INSERT INTO stations (id, site_id, code, name, key_hash, scale_config, active) VALUES ($1,$2,$3,$4,$5,$6,$7)")
        .bind(id).bind(b.site_id).bind(&code).bind(req(&b.name, "Name")?).bind(hash).bind(&b.scale_config).bind(b.active)
        .execute(&st.db).await?;
    audit(&st.db, &u.actor(), "create", "station", &id.to_string(), json!({ "code": code })).await;
    Ok(Json(json!({ "id": id, "key": key, "note": "Copy this key into the station setup now. It is not shown again." })))
}

async fn update_station(State(st): State<AppState>, u: AuthUser, Path(id): Path<Uuid>, Json(b): Json<StationIn>) -> ApiResult<Value> {
    u.require(&[])?;
    let n = sqlx::query("UPDATE stations SET site_id=$2, name=$3, scale_config=$4, active=$5, updated_at=now() WHERE id=$1")
        .bind(id).bind(b.site_id).bind(req(&b.name, "Name")?).bind(&b.scale_config).bind(b.active)
        .execute(&st.db).await?.rows_affected();
    if n == 0 {
        return Err(AppError::NotFound);
    }
    audit(&st.db, &u.actor(), "update", "station", &id.to_string(), json!({})).await;
    Ok(Json(json!({ "ok": true })))
}

async fn rotate_station_key(State(st): State<AppState>, u: AuthUser, Path(id): Path<Uuid>) -> ApiResult<Value> {
    u.require(&[])?;
    let (key, hash) = new_station_key(id);
    let n = sqlx::query("UPDATE stations SET key_hash=$2, updated_at=now() WHERE id=$1").bind(id).bind(hash).execute(&st.db).await?.rows_affected();
    if n == 0 {
        return Err(AppError::NotFound);
    }
    audit(&st.db, &u.actor(), "rotate_key", "station", &id.to_string(), json!({})).await;
    Ok(Json(json!({ "key": key })))
}

// ---------------- audit ----------------

async fn list_audit(State(st): State<AppState>, u: AuthUser) -> ApiResult<Value> {
    u.require(&["supervisor"])?;
    let rows = sqlx::query("SELECT id, at, actor, action, entity, entity_id, details FROM audit_log ORDER BY at DESC LIMIT 300").fetch_all(&st.db).await?;
    Ok(Json(rows_json(&rows)))
}

/// Turn untyped rows into JSON objects (for list endpoints with joins).
pub fn rows_json(rows: &[sqlx::postgres::PgRow]) -> Value {
    use sqlx::{Column, Row, TypeInfo};
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let mut m = serde_json::Map::new();
        for (i, c) in r.columns().iter().enumerate() {
            let v: Value = match c.type_info().name() {
                "UUID" => r.try_get::<Option<Uuid>, _>(i).ok().flatten().map(|u| json!(u)).unwrap_or(Value::Null),
                "TEXT" | "VARCHAR" | "NAME" => r.try_get::<Option<String>, _>(i).ok().flatten().map(Value::from).unwrap_or(Value::Null),
                "BOOL" => r.try_get::<Option<bool>, _>(i).ok().flatten().map(Value::from).unwrap_or(Value::Null),
                "INT2" => r.try_get::<Option<i16>, _>(i).ok().flatten().map(Value::from).unwrap_or(Value::Null),
                "INT4" => r.try_get::<Option<i32>, _>(i).ok().flatten().map(Value::from).unwrap_or(Value::Null),
                "INT8" => r.try_get::<Option<i64>, _>(i).ok().flatten().map(Value::from).unwrap_or(Value::Null),
                "FLOAT4" => r.try_get::<Option<f32>, _>(i).ok().flatten().map(Value::from).unwrap_or(Value::Null),
                "FLOAT8" => r.try_get::<Option<f64>, _>(i).ok().flatten().map(Value::from).unwrap_or(Value::Null),
                "TIMESTAMPTZ" => r.try_get::<Option<chrono::DateTime<chrono::Utc>>, _>(i).ok().flatten().map(|d| json!(d)).unwrap_or(Value::Null),
                "DATE" => r.try_get::<Option<chrono::NaiveDate>, _>(i).ok().flatten().map(|d| json!(d)).unwrap_or(Value::Null),
                "JSONB" | "JSON" => r.try_get::<Option<Value>, _>(i).ok().flatten().unwrap_or(Value::Null),
                other => json!(format!("<{other}>")),
            };
            m.insert(c.name().to_string(), v);
        }
        out.push(Value::Object(m));
    }
    Value::Array(out)
}
