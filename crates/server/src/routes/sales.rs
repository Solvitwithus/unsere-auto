//! Sales (invoices) in the eTIMS format, created by hand or from a ticket.

use crate::auth::AuthUser;
use crate::error::{bad, ApiResult, AppError};
use crate::routes::masters::{rows_json, PRODUCT_COLS};
use crate::state::{audit, next_counter, AppState};
use axum::extract::{Path, Query, State};
use axum::http::header;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::NaiveDate;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;
use wb_core::sale::{calculate_invoice_totals, etims_request, Buyer, Sale, SaleItem, SaleLineInput, SALE_TYPES};
use wb_core::{Party, Product};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/sales", get(list).post(create))
        .route("/api/sales/{id}", get(get_one))
        .route("/api/sales/{id}/cancel", post(cancel))
        .route("/api/sales/{id}/retry", post(retry))
        .route("/api/sales/{id}/etims-request", get(etims_body))
        .route("/api/sales/{id}/receipt.txt", get(receipt_text))
        .route("/api/tickets/{id}/sale", post(create_for_ticket))
}

pub const SALE_COLS: &str = "id, sale_no, ticket_id, site_id, sale_type, status, payment_type, sale_date, buyer_name, buyer_tin, buyer_phone, buyer_email, \
    party_id, prices_include_tax, taxable_amount::float8 AS taxable_amount, tax_amount::float8 AS tax_amount, total_amount::float8 AS total_amount, \
    remark, actor_name, actor_id, etims_status, etims_invoice_id, etims_invoice_code, etims_scu, etims_error, etims_attempts, created_at, updated_at";

pub const ITEM_COLS: &str = "seq, product_id, item_code, classification, name, quantity::float8 AS quantity, quantity_unit, packaging_unit, \
    unit_price::float8 AS unit_price, discount_rate::float8 AS discount_rate, discount_amount::float8 AS discount_amount, vat_rate::float8 AS vat_rate, \
    tax_type_code, prc::float8 AS prc, sply_amt::float8 AS sply_amt, dc_amt::float8 AS dc_amt, taxbl_amt::float8 AS taxbl_amt, \
    tax_amt::float8 AS tax_amt, tot_amt::float8 AS tot_amt";

pub async fn load(db: &sqlx::PgPool, id: Uuid) -> Result<(Sale, Vec<SaleItem>), AppError> {
    let sale: Sale = sqlx::query_as(&format!("SELECT {SALE_COLS} FROM sales WHERE id=$1")).bind(id).fetch_optional(db).await?.ok_or(AppError::NotFound)?;
    let items: Vec<SaleItem> = sqlx::query_as(&format!("SELECT {ITEM_COLS} FROM sale_items WHERE sale_id=$1 ORDER BY seq")).bind(id).fetch_all(db).await?;
    Ok((sale, items))
}

#[derive(Deserialize, Default)]
pub struct SaleIn {
    #[serde(default)]
    pub ticket_id: Option<Uuid>,
    #[serde(default)]
    pub site_id: Option<Uuid>,
    #[serde(default)]
    pub party_id: Option<Uuid>,
    #[serde(default)]
    pub buyer: Option<Buyer>,
    #[serde(default)]
    pub payment_type: Option<String>,
    #[serde(default)]
    pub sale_type: Option<String>,
    #[serde(default)]
    pub prices_include_tax: Option<bool>,
    #[serde(default)]
    pub remark: Option<String>,
    #[serde(default)]
    pub items: Vec<SaleLineInput>,
}

/// Fill missing line fields from the product master.
async fn resolve_lines(db: &sqlx::PgPool, lines: &[SaleLineInput]) -> Result<Vec<SaleLineInput>, AppError> {
    let mut out = Vec::new();
    for l in lines {
        let mut l = l.clone();
        if let Some(pid) = l.product_id {
            let p: Product = sqlx::query_as(&format!("SELECT {PRODUCT_COLS} FROM products WHERE id=$1")).bind(pid).fetch_optional(db).await?
                .ok_or_else(|| bad("Unknown product on a line"))?;
            l.item_code = l.item_code.or(p.item_code.clone());
            l.name = l.name.filter(|s| !s.is_empty()).or(Some(p.name.clone()));
            l.classification = l.classification.filter(|s| !s.is_empty()).or(Some(p.classification_code.clone()));
            l.unit_price = l.unit_price.or(Some(p.unit_price));
            l.tax_type_code = l.tax_type_code.or(Some(p.tax_type_code.clone()));
            l.quantity_unit = l.quantity_unit.or(Some(p.quantity_unit.clone()));
            l.packaging_unit = l.packaging_unit.or(Some(p.packaging_unit.clone()));
        }
        out.push(l);
    }
    Ok(out)
}

/// Create a sale. `actor` = (name, id) as recorded on the eTIMS invoice.
pub async fn create_sale(st: &AppState, b: SaleIn, actor: (String, String)) -> Result<Uuid, AppError> {
    if b.items.is_empty() {
        return Err(bad("A sale needs at least one line"));
    }
    let sale_type = b.sale_type.clone().unwrap_or_else(|| "NORMAL".into());
    if !SALE_TYPES.contains(&sale_type.as_str()) || sale_type == "CREDIT_NOTE" {
        return Err(bad("Sale type must be NORMAL, PROFORMA or TRAINING (credit notes are issued in eTIMS)"));
    }
    let site: Option<(String, bool)> = match b.site_id {
        Some(s) => sqlx::query_as("SELECT code, prices_include_tax FROM sites WHERE id=$1").bind(s).fetch_optional(&st.db).await?,
        None => None,
    };
    let prices_include_tax = b.prices_include_tax.or(site.as_ref().map(|s| s.1)).unwrap_or(true);
    let lines = resolve_lines(&st.db, &b.items).await?;
    let mut items = Vec::new();
    for (i, l) in lines.iter().enumerate() {
        items.push(SaleItem::price(i as i32 + 1, l, prices_include_tax).map_err(bad)?);
    }
    let totals = calculate_invoice_totals(&items.iter().map(|i| i.amounts()).collect::<Vec<_>>());
    if totals.total_amount <= 0.0 {
        return Err(bad("The sale total must be more than zero"));
    }
    // buyer: explicit, else from the party, else walk-in
    let party: Option<Party> = match b.party_id {
        Some(p) => sqlx::query_as("SELECT id, code, name, kind, tin, phone, email, address, active, updated_at FROM parties WHERE id=$1")
            .bind(p).fetch_optional(&st.db).await?,
        None => None,
    };
    let mut buyer = b.buyer.clone().unwrap_or_default();
    if let Some(p) = &party {
        if buyer.name.trim().is_empty() {
            buyer.name = p.name.clone();
        }
        buyer.tin = buyer.tin.or(p.tin.clone());
        buyer.phone = buyer.phone.or(Some(p.phone.clone()).filter(|s| !s.is_empty()));
        buyer.email = buyer.email.or(Some(p.email.clone()).filter(|s| !s.is_empty()));
    }
    if buyer.name.trim().is_empty() {
        buyer.name = "Walk-in Customer".into();
    }

    // Number + insert in one transaction: the counter row is locked until
    // commit, so two invoices can never get the same number and a failed
    // insert doesn't burn a number (no gaps, which KRA cares about).
    let mut tx = st.db.begin().await?;
    let prefix = site.as_ref().map(|s| s.0.clone()).unwrap_or_else(|| "HQ".into());
    let n = next_counter(&mut *tx, &format!("sale:{prefix}")).await?;
    let sale_no = format!("INV-{prefix}-{n:06}");
    let id = Uuid::new_v4();
    // proforma/training never go to KRA for signing through this flow
    let etims_status = if sale_type == "NORMAL" { st.etims_initial_status() } else { "DISABLED" };
    sqlx::query(
        "INSERT INTO sales (id, sale_no, ticket_id, site_id, sale_type, status, payment_type, sale_date, buyer_name, buyer_tin, buyer_phone, buyer_email,
                            party_id, prices_include_tax, taxable_amount, tax_amount, total_amount, remark, actor_name, actor_id, etims_status, etims_next_try_at)
         VALUES ($1,$2,$3,$4,$5,'APPROVED',$6,now(),$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19, now())",
    )
    .bind(id).bind(&sale_no).bind(b.ticket_id).bind(b.site_id).bind(&sale_type)
    .bind(b.payment_type.clone().filter(|s| !s.is_empty()).unwrap_or_else(|| "Cash".into()))
    .bind(buyer.name.trim()).bind(buyer.tin.as_deref().map(|s| s.trim().to_uppercase()).filter(|s| !s.is_empty()))
    .bind(&buyer.phone).bind(&buyer.email).bind(b.party_id).bind(prices_include_tax)
    .bind(totals.taxable_amount).bind(totals.tax_amount).bind(totals.total_amount).bind(&b.remark)
    .bind(&actor.0).bind(&actor.1).bind(etims_status)
    .execute(&mut *tx).await?;
    for it in &items {
        sqlx::query(
            "INSERT INTO sale_items (sale_id, seq, product_id, item_code, classification, name, quantity, quantity_unit, packaging_unit, unit_price,
                                     discount_rate, discount_amount, vat_rate, tax_type_code, prc, sply_amt, dc_amt, taxbl_amt, tax_amt, tot_amt)
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20)",
        )
        .bind(id).bind(it.seq).bind(it.product_id).bind(&it.item_code).bind(&it.classification).bind(&it.name).bind(it.quantity)
        .bind(&it.quantity_unit).bind(&it.packaging_unit).bind(it.unit_price).bind(it.discount_rate).bind(it.discount_amount)
        .bind(it.vat_rate).bind(&it.tax_type_code).bind(it.prc).bind(it.sply_amt).bind(it.dc_amt).bind(it.taxbl_amt).bind(it.tax_amt).bind(it.tot_amt)
        .execute(&mut *tx).await?;
    }
    tx.commit().await?;
    audit(&st.db, &actor.1, "create", "sale", &id.to_string(), json!({ "sale_no": sale_no, "total": totals.total_amount, "ticket_id": b.ticket_id })).await;
    Ok(id)
}

#[derive(Deserialize, Default)]
pub struct FromTicket {
    #[serde(default)]
    pub payment_type: Option<String>,
    #[serde(default)]
    pub buyer: Option<Buyer>,
    #[serde(default)]
    pub remark: Option<String>,
    /// Override the product's price for this sale
    #[serde(default)]
    pub unit_price: Option<f64>,
}

/// Build the sale for a completed ticket: one line for the product (net weight
/// in the product's unit) plus a weighing-fee line if the ticket has charges.
pub async fn create_from_ticket(st: &AppState, ticket_id: Uuid, opts: FromTicket, actor: (String, String)) -> Result<Uuid, AppError> {
    let t = crate::routes::tickets::load(&st.db, ticket_id).await?;
    if t.sale_id.is_some() {
        return Err(AppError::Conflict(format!("Ticket already invoiced ({})", t.sale_no.unwrap_or_default())));
    }
    let tk = &t.ticket;
    if tk.status != "complete" {
        return Err(bad("Only completed tickets (both weights taken) can be invoiced"));
    }
    let net = tk.net().unwrap_or(0.0);
    let mut items = Vec::new();
    if let Some(pid) = tk.product_id {
        let p: Product = sqlx::query_as(&format!("SELECT {PRODUCT_COLS} FROM products WHERE id=$1")).bind(pid).fetch_one(&st.db).await?;
        // Quantity in the product's selling unit. Weights are in kg; a product
        // sold per tonne (TNE) gets net/1000, e.g. 24,420 kg -> 24.42 TNE.
        // eTIMS accepts 2 decimals, so SaleItem::price rounds it.
        let qty = match p.quantity_unit.as_str() {
            "TNE" => net / 1000.0,
            "KG" => net,
            _ => 1.0,
        };
        if qty > 0.0 {
            items.push(SaleLineInput {
                product_id: Some(pid),
                quantity: qty,
                unit_price: opts.unit_price.or(Some(p.unit_price)),
                ..Default::default()
            });
        }
    }
    if tk.charges > 0.0 {
        let fee: Option<Product> = sqlx::query_as(&format!("SELECT {PRODUCT_COLS} FROM products WHERE code='FEE' AND active LIMIT 1")).fetch_optional(&st.db).await?;
        items.push(match fee {
            Some(f) => SaleLineInput { product_id: Some(f.id), quantity: 1.0, unit_price: Some(tk.charges), quantity_unit: Some("U".into()), ..Default::default() },
            None => SaleLineInput {
                name: Some("Weighbridge fee".into()),
                quantity: 1.0,
                unit_price: Some(tk.charges),
                quantity_unit: Some("U".into()),
                ..Default::default()
            },
        });
    }
    if items.is_empty() {
        return Err(bad("Nothing to invoice: set a product with a price, or charges, on the ticket"));
    }
    let remark = opts.remark.clone().or_else(|| Some(format!("Weighbridge ticket {} - vehicle {} - net {} kg", tk.ticket_no, tk.vehicle_plate, wb_core::group(net))));
    create_sale(
        st,
        SaleIn {
            ticket_id: Some(ticket_id),
            site_id: tk.site_id,
            party_id: tk.party_id,
            buyer: opts.buyer.or_else(|| (tk.party_id.is_none() && !tk.party_name.is_empty()).then(|| Buyer { name: tk.party_name.clone(), ..Default::default() })),
            payment_type: opts.payment_type,
            sale_type: Some("NORMAL".into()),
            prices_include_tax: None,
            remark,
            items,
        },
        actor,
    )
    .await
}

async fn create(State(st): State<AppState>, u: AuthUser, Json(b): Json<SaleIn>) -> ApiResult<Value> {
    u.require(&["supervisor", "operator"])?;
    let site_id = b.site_id.or(u.0.site_id);
    let id = create_sale(&st, SaleIn { site_id, ..b }, (u.0.name.clone(), u.0.username.clone())).await?;
    let (sale, items) = load(&st.db, id).await?;
    Ok(Json(json!({ "sale": sale, "items": items })))
}

async fn create_for_ticket(State(st): State<AppState>, u: AuthUser, Path(id): Path<Uuid>, body: Option<Json<FromTicket>>) -> ApiResult<Value> {
    u.require(&["supervisor", "operator"])?;
    let sid = create_from_ticket(&st, id, body.map(|b| b.0).unwrap_or_default(), (u.0.name.clone(), u.0.username.clone())).await?;
    let (sale, items) = load(&st.db, sid).await?;
    Ok(Json(json!({ "sale": sale, "items": items })))
}

#[derive(Deserialize)]
struct ListQ {
    from: Option<NaiveDate>,
    to: Option<NaiveDate>,
    etims_status: Option<String>,
    q: Option<String>,
    limit: Option<i64>,
}

async fn list(State(st): State<AppState>, u: AuthUser, Query(q): Query<ListQ>) -> ApiResult<Value> {
    let site = if u.0.role == "admin" { None } else { u.0.site_id };
    let from = q.from.map(|d| d.and_hms_opt(0, 0, 0).unwrap().and_utc());
    let to = q.to.map(|d| (d + chrono::Duration::days(1)).and_hms_opt(0, 0, 0).unwrap().and_utc());
    let rows = sqlx::query(
        "SELECT s.id, s.sale_no, s.sale_date, s.sale_type, s.status, s.payment_type, s.buyer_name, s.buyer_tin, s.total_amount::float8 AS total_amount,
                s.tax_amount::float8 AS tax_amount, s.etims_status, s.etims_invoice_code, s.etims_error, s.ticket_id, t.ticket_no, t.vehicle_plate, si.name AS site_name
         FROM sales s LEFT JOIN tickets t ON t.id = s.ticket_id LEFT JOIN sites si ON si.id = s.site_id
         WHERE ($1::timestamptz IS NULL OR s.sale_date >= $1) AND ($2::timestamptz IS NULL OR s.sale_date < $2)
           AND ($3::text IS NULL OR s.etims_status = $3) AND ($4::uuid IS NULL OR s.site_id = $4)
           AND ($5::text IS NULL OR s.sale_no ILIKE '%'||$5||'%' OR s.buyer_name ILIKE '%'||$5||'%' OR t.vehicle_plate ILIKE '%'||$5||'%')
         ORDER BY s.sale_date DESC LIMIT $6",
    )
    .bind(from).bind(to).bind(q.etims_status.filter(|s| !s.is_empty())).bind(site).bind(q.q.filter(|s| !s.trim().is_empty()))
    .bind(q.limit.unwrap_or(200).clamp(1, 2000))
    .fetch_all(&st.db).await?;
    Ok(Json(rows_json(&rows)))
}

async fn get_one(State(st): State<AppState>, _u: AuthUser, Path(id): Path<Uuid>) -> ApiResult<Value> {
    let (sale, items) = load(&st.db, id).await?;
    Ok(Json(json!({ "sale": sale, "items": items, "etims_configured": st.etims.is_some() })))
}

/// The exact JSON body that is (or will be) POSTed to eTIMS /invoices.
async fn etims_body(State(st): State<AppState>, _u: AuthUser, Path(id): Path<Uuid>) -> ApiResult<Value> {
    let (sale, items) = load(&st.db, id).await?;
    Ok(Json(etims_request(&sale, &items, st.etims.as_ref().and_then(|e| e.control_unit_id.as_deref()))))
}

#[derive(Deserialize)]
struct CancelReq {
    reason: String,
}

async fn cancel(State(st): State<AppState>, u: AuthUser, Path(id): Path<Uuid>, Json(b): Json<CancelReq>) -> ApiResult<Value> {
    u.require(&["supervisor"])?;
    let (sale, _) = load(&st.db, id).await?;
    if matches!(sale.etims_status.as_str(), "SIGNED" | "SUBMITTED") {
        return Err(bad("This invoice is with KRA. Issue a credit note in eTIMS instead of cancelling it here."));
    }
    sqlx::query("UPDATE sales SET status='CANCELED', etims_status='DISABLED', remark = COALESCE(remark,'') || ' [cancelled: ' || $2 || ']', updated_at=now() WHERE id=$1")
        .bind(id).bind(b.reason.trim()).execute(&st.db).await?;
    audit(&st.db, &u.actor(), "cancel", "sale", &id.to_string(), json!({ "reason": b.reason })).await;
    Ok(Json(json!({ "ok": true })))
}

async fn retry(State(st): State<AppState>, u: AuthUser, Path(id): Path<Uuid>) -> ApiResult<Value> {
    u.require(&["supervisor"])?;
    if st.etims.is_none() {
        return Err(bad("eTIMS is not configured on this server (ETIMS_URL / ETIMS_API_KEY)"));
    }
    let n = sqlx::query("UPDATE sales SET etims_status = CASE WHEN etims_status='DISABLED' THEN 'PENDING' ELSE etims_status END, etims_next_try_at = now(), etims_attempts = 0, updated_at=now()
                         WHERE id=$1 AND status='APPROVED' AND sale_type='NORMAL' AND etims_status IN ('FAILED','PENDING','SUBMITTED','DISABLED')")
        .bind(id).execute(&st.db).await?.rows_affected();
    if n == 0 {
        return Err(bad("Nothing to retry for this sale"));
    }
    crate::etims::kick(&st);
    Ok(Json(json!({ "ok": true })))
}

async fn receipt_text(State(st): State<AppState>, _u: AuthUser, Path(id): Path<Uuid>) -> Result<impl IntoResponse, AppError> {
    let (sale, items) = load(&st.db, id).await?;
    let site: Option<(String, String, String, String)> = match sale.site_id {
        Some(s) => sqlx::query_as("SELECT name, address, phone, kra_pin FROM sites WHERE id=$1").bind(s).fetch_optional(&st.db).await?,
        None => None,
    };
    Ok(([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], render_receipt(&sale, &items, site.as_ref())))
}

/// 42-column tax invoice. Before KRA signs it, it's marked as not a valid tax invoice.
pub fn render_receipt(s: &Sale, items: &[SaleItem], site: Option<&(String, String, String, String)>) -> String {
    const W: usize = 42;
    let c = |t: &str| format!("{:^W$}", t.chars().take(W).collect::<String>());
    let lr = |l: &str, r: &str| format!("{l}{:>w$}", r, w = W.saturating_sub(l.chars().count()));
    let m = |v: f64| format!("{v:.2}");
    let mut o = Vec::new();
    if let Some((name, addr, phone, pin)) = site {
        o.push(c(&name.to_uppercase()));
        if !addr.is_empty() { o.push(c(addr)); }
        if !phone.is_empty() { o.push(c(&format!("Tel: {phone}"))); }
        if !pin.is_empty() { o.push(c(&format!("PIN: {pin}"))); }
    }
    o.push(c(if s.sale_type == "NORMAL" { "TAX INVOICE" } else { &s.sale_type }));
    if s.status == "CANCELED" { o.push(c("*** CANCELLED ***")); }
    o.push("-".repeat(W));
    o.push(lr("Invoice:", &s.sale_no));
    o.push(lr("Date:", &s.sale_date.with_timezone(&chrono::Local).format("%d/%m/%Y %H:%M").to_string()));
    o.push(lr("Buyer:", &s.buyer_name));
    if let Some(t) = &s.buyer_tin { o.push(lr("Buyer PIN:", t)); }
    o.push("-".repeat(W));
    for it in items {
        o.push(it.name.chars().take(W).collect());
        o.push(lr(&format!("  {} {} x {}", it.quantity, it.quantity_unit, m(it.prc)), &format!("{} {}", m(it.tot_amt), it.tax_type_code)));
        if it.dc_amt > 0.0 { o.push(lr("  Discount", &format!("-{}", m(it.dc_amt)))); }
    }
    o.push("-".repeat(W));
    o.push(lr("TAXABLE", &m(s.taxable_amount)));
    o.push(lr("VAT", &m(s.tax_amount)));
    o.push(lr("TOTAL", &m(s.total_amount)));
    o.push(lr(&s.payment_type.to_uppercase(), &m(s.total_amount)));
    o.push("-".repeat(W));
    match (&s.etims_status[..], &s.etims_scu) {
        ("SIGNED", Some(scu)) => {
            o.push(c("SCU INFORMATION"));
            let g = |k: &str| scu.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
            o.push(lr("SCU ID:", &g("scuId")));
            o.push(lr("CU Invoice No:", &g("cuInvoiceNo")));
            o.push(lr("Internal Data:", ""));
            o.push(g("internalData"));
            o.push(lr("Receipt Signature:", ""));
            o.push(g("receiptSignature"));
            if !g("qrUrl").is_empty() { o.push(format!("Verify: {}", g("qrUrl"))); }
        }
        ("DISABLED", _) => o.push(c("(not submitted to KRA eTIMS)")),
        _ => {
            o.push(c("AWAITING KRA SIGNATURE"));
            o.push(c("Not a valid tax invoice yet"));
        }
    }
    o.push(lr("Served by:", &s.actor_name));
    o.join("\n")
}

