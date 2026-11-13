//! Sends sales to eTIMS (etims-rust `POST /invoices`) and collects the KRA signature.
//!
//! * PENDING / FAILED: POST the invoice. The sale number is the idempotency
//!   key, so a repeat (after a timeout, say) returns the same invoice.
//! * A reply with a non-null `receipt` means KRA signed it: SIGNED, with
//!   `receipt.scu` (cuInvoiceNo, internalData, receiptSignature, qrUrl) stored.
//! * Accepted but not yet signed (eTIMS queued it): SUBMITTED; we then poll
//!   `GET /invoices/{id}/receipt` until it is printable.
//! * Errors back off (30 s, 1 min, 2 min ... capped at 30 min). A 4xx
//!   validation error waits an hour; fix the data and press Retry.

use crate::state::{AppState, EtimsConfig};
use serde_json::Value;
use std::sync::OnceLock;
use std::time::Duration;
use tokio::sync::Notify;
use uuid::Uuid;
use wb_core::sale::etims_request;

static KICK: OnceLock<Notify> = OnceLock::new();

/// Wake the worker now (after a new sale or a retry).
pub fn kick(_st: &AppState) {
    KICK.get_or_init(Notify::new).notify_one();
}

pub fn spawn(st: AppState) {
    let Some(cfg) = st.etims.clone() else {
        tracing::info!("eTIMS not configured (set ETIMS_URL and ETIMS_API_KEY to send sales to KRA)");
        return;
    };
    tracing::info!("eTIMS worker sending to {}", cfg.base_url);
    tokio::spawn(async move {
        let notify = KICK.get_or_init(Notify::new);
        loop {
            if let Err(e) = tick(&st, &cfg).await {
                tracing::warn!("eTIMS worker: {e:#}");
            }
            let _ = tokio::time::timeout(Duration::from_secs(15), notify.notified()).await;
        }
    });
}

async fn tick(st: &AppState, cfg: &EtimsConfig) -> anyhow::Result<()> {
    let due: Vec<(Uuid, String, Option<String>, i32)> = sqlx::query_as(
        "SELECT id, etims_status, etims_invoice_id, etims_attempts FROM sales
         WHERE status='APPROVED' AND sale_type='NORMAL' AND etims_status IN ('PENDING','SUBMITTED','FAILED')
           AND (etims_next_try_at IS NULL OR etims_next_try_at <= now())
         ORDER BY created_at LIMIT 25",
    )
    .fetch_all(&st.db)
    .await?;
    for (id, status, inv_id, attempts) in due {
        let r = if status == "SUBMITTED" && inv_id.is_some() {
            poll_receipt(st, cfg, id, inv_id.as_deref().unwrap(), attempts).await
        } else {
            send(st, cfg, id, attempts).await
        };
        if let Err(e) = r {
            tracing::warn!("eTIMS sale {id}: {e:#}");
        }
    }
    Ok(())
}

fn backoff(attempts: i32) -> i64 {
    (30i64 << attempts.clamp(0, 6)).min(1800)
}

async fn fail(st: &AppState, id: Uuid, attempts: i32, err: &str, wait_secs: i64, resp: Option<&Value>) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE sales SET etims_status='FAILED', etims_error=$2, etims_attempts=$3, etims_next_try_at = now() + make_interval(secs => $4),
                etims_response = COALESCE($5, etims_response), updated_at=now() WHERE id=$1",
    )
    .bind(id).bind(err.chars().take(1000).collect::<String>()).bind(attempts + 1).bind(wait_secs as f64).bind(resp)
    .execute(&st.db).await?;
    Ok(())
}

fn error_text(v: &Value, status: u16) -> String {
    let base = v.get("error").or_else(|| v.get("message")).and_then(|e| e.as_str()).unwrap_or("").to_string();
    let details = v.get("details").map(|d| format!(" {d}")).unwrap_or_default();
    if base.is_empty() && details.is_empty() {
        format!("eTIMS HTTP {status}")
    } else {
        format!("{base}{details}")
    }
}

async fn send(st: &AppState, cfg: &EtimsConfig, id: Uuid, attempts: i32) -> anyhow::Result<()> {
    let (sale, items) = crate::routes::sales::load(&st.db, id).await.map_err(|e| anyhow::anyhow!("{e}"))?;
    let body = etims_request(&sale, &items, cfg.control_unit_id.as_deref());
    let res = st
        .http
        .post(format!("{}/invoices", cfg.base_url))
        .bearer_auth(&cfg.api_key)
        .json(&body)
        .timeout(Duration::from_secs(45))
        .send()
        .await;
    let res = match res {
        Ok(r) => r,
        Err(e) => return fail(st, id, attempts, &format!("eTIMS unreachable: {e}"), backoff(attempts), None).await,
    };
    let code = res.status().as_u16();
    let v: Value = res.json().await.unwrap_or(Value::Null);
    if !(200..300).contains(&code) || v.get("success") == Some(&Value::Bool(false)) {
        let wait = if (400..500).contains(&code) && code != 429 && code != 408 { 3600 } else { backoff(attempts) };
        return fail(st, id, attempts, &error_text(&v, code), wait, Some(&v)).await;
    }
    let data = v.get("data").cloned().unwrap_or(Value::Null);
    let inv_id = data.get("id").and_then(|x| x.as_str()).map(String::from);
    let inv_code = data.get("invoiceCode").and_then(|x| x.as_str()).map(String::from);
    let scu = v.get("receipt").and_then(|r| r.get("scu")).cloned().filter(|s| !s.is_null());
    let signed = scu.as_ref().is_some_and(|s| s.get("receiptSignature").is_some_and(|x| !x.is_null()));
    sqlx::query(
        "UPDATE sales SET etims_status=$2, etims_invoice_id=$3, etims_invoice_code=$4, etims_scu=$5, etims_response=$6, etims_error=NULL,
                etims_attempts=$7, etims_next_try_at = now() + interval '20 seconds', updated_at=now() WHERE id=$1",
    )
    .bind(id).bind(if signed { "SIGNED" } else { "SUBMITTED" }).bind(inv_id).bind(inv_code).bind(scu).bind(&v).bind(attempts + 1)
    .execute(&st.db).await?;
    tracing::info!("eTIMS sale {} -> {}", sale.sale_no, if signed { "SIGNED" } else { "SUBMITTED" });
    Ok(())
}

async fn poll_receipt(st: &AppState, cfg: &EtimsConfig, id: Uuid, inv_id: &str, attempts: i32) -> anyhow::Result<()> {
    let res = st
        .http
        .get(format!("{}/invoices/{inv_id}/receipt", cfg.base_url))
        .bearer_auth(&cfg.api_key)
        .timeout(Duration::from_secs(20))
        .send()
        .await;
    let wait = backoff(attempts.min(4));
    let Ok(res) = res else {
        sqlx::query("UPDATE sales SET etims_next_try_at = now() + make_interval(secs => $2) WHERE id=$1").bind(id).bind(wait as f64).execute(&st.db).await?;
        return Ok(());
    };
    let v: Value = res.json().await.unwrap_or(Value::Null);
    let data = v.get("data").cloned().unwrap_or(Value::Null);
    let printable = data.get("printable").and_then(|p| p.as_bool()).unwrap_or(false);
    if printable {
        sqlx::query("UPDATE sales SET etims_status='SIGNED', etims_scu=$2, etims_error=NULL, updated_at=now() WHERE id=$1")
            .bind(id).bind(data.get("scu")).execute(&st.db).await?;
    } else {
        let reason = data.get("notPrintableReason").and_then(|r| r.as_str()).map(String::from);
        sqlx::query("UPDATE sales SET etims_attempts = etims_attempts + 1, etims_error = $3, etims_next_try_at = now() + make_interval(secs => $2) WHERE id=$1")
            .bind(id).bind(wait as f64).bind(reason).execute(&st.db).await?;
    }
    Ok(())
}
