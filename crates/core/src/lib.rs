//! Types shared by the station (SQLite) and the central server (PostgreSQL).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub mod sale;

pub const TRANS_TYPES: &[&str] = &["purchase", "sale", "transfer", "other"];
pub const ROLES: &[&str] = &["admin", "supervisor", "operator", "viewer"];

#[derive(Clone, Debug, Serialize, Deserialize, sqlx::FromRow)]
pub struct Ticket {
    pub id: Uuid,
    pub ticket_no: String,
    pub site_id: Option<Uuid>,
    pub station_id: Option<Uuid>,
    pub trans_type: String,
    pub status: String,
    pub vehicle_plate: String,
    pub driver_name: String,
    pub party_id: Option<Uuid>,
    pub party_name: String,
    pub product_id: Option<Uuid>,
    pub product_name: String,
    pub source: String,
    pub destination: String,
    pub reference: String,
    pub memo: String,
    pub first_weight_kg: f64,
    pub first_weight_at: DateTime<Utc>,
    pub first_raw: String,
    pub first_manual: bool,
    pub second_weight_kg: Option<f64>,
    pub second_weight_at: Option<DateTime<Utc>>,
    pub second_raw: String,
    pub second_manual: bool,
    #[sqlx(default)]
    pub net_weight_kg: Option<f64>,
    pub charges: f64,
    pub operator_id: Option<Uuid>,
    pub operator_name: String,
    pub void_reason: String,
    pub print_count: i32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Ticket {
    pub fn net(&self) -> Option<f64> {
        self.second_weight_kg.map(|s| (s - self.first_weight_kg).abs())
    }
    /// Gross is the heavier weighing, tare the lighter.
    pub fn gross_tare(&self) -> (f64, Option<f64>) {
        match self.second_weight_kg {
            Some(s) => (s.max(self.first_weight_kg), Some(s.min(self.first_weight_kg))),
            None => (self.first_weight_kg, None),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, sqlx::FromRow)]
pub struct SyncUser {
    pub id: Uuid,
    pub username: String,
    pub full_name: String,
    pub password_hash: String,
    pub role: String,
    pub active: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, sqlx::FromRow)]
pub struct Party {
    pub id: Uuid,
    pub code: Option<String>,
    pub name: String,
    pub kind: String,
    /// KRA PIN, sent to eTIMS as buyer.tin
    pub tin: Option<String>,
    pub phone: String,
    pub email: String,
    pub address: String,
    pub active: bool,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, Deserialize, sqlx::FromRow)]
pub struct Product {
    pub id: Uuid,
    pub code: Option<String>,
    pub name: String,
    /// Price per `quantity_unit` (TNE = tonne, KG = kilogram, U = each)
    pub unit_price: f64,
    pub quantity_unit: String,
    /// Registered eTIMS item code (optional); else classification + name are sent
    pub item_code: Option<String>,
    pub classification_code: String,
    pub tax_type_code: String,
    pub packaging_unit: String,
    pub active: bool,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, Deserialize, sqlx::FromRow)]
pub struct Vehicle {
    pub id: Uuid,
    pub plate: String,
    pub stored_tare_kg: Option<f64>,
    pub party_id: Option<Uuid>,
    pub driver_name: String,
    pub active: bool,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, Deserialize, sqlx::FromRow)]
pub struct Site {
    pub id: Uuid,
    pub code: String,
    pub name: String,
    pub address: String,
    pub phone: String,
    pub ticket_footer: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PullResponse {
    pub now: Option<DateTime<Utc>>,
    pub station_id: Option<Uuid>,
    pub station_code: String,
    pub site: Option<Site>,
    pub users: Vec<SyncUser>,
    pub parties: Vec<Party>,
    pub products: Vec<Product>,
    pub vehicles: Vec<Vehicle>,
    pub tickets: Vec<Ticket>,
    pub scale_config: Option<serde_json::Value>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PushRequest {
    pub tickets: Vec<Ticket>,
    /// Live station health (scale state, version) for the dashboard.
    pub status: Option<serde_json::Value>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PushResponse {
    pub accepted: Vec<Uuid>,
}

/// Normalise a number plate: upper case, no spaces/dashes. "kbc 123a" -> "KBC123A".
pub fn normalize_plate(p: &str) -> String {
    p.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_ascii_uppercase()
}

/// Fixed-width text ticket (42 columns, fits 80 mm thermal printers).
pub fn render_ticket_text(t: &Ticket, site: Option<&Site>, reprint: bool) -> String {
    const W: usize = 42;
    let center = |s: &str| format!("{:^W$}", s.chars().take(W).collect::<String>());
    let line = |k: &str, v: &str| {
        let v: String = v.chars().take(W - k.len() - 1).collect();
        format!("{k}{:>width$}", v, width = W - k.len())
    };
    let kg = |v: f64| format!("{} kg", group(v));
    let mut o = Vec::new();
    if let Some(s) = site {
        o.push(center(&s.name.to_uppercase()));
        if !s.address.is_empty() {
            o.push(center(&s.address));
        }
        if !s.phone.is_empty() {
            o.push(center(&format!("Tel: {}", s.phone)));
        }
    }
    o.push(center("WEIGHBRIDGE TICKET"));
    if reprint {
        o.push(center("*** REPRINT ***"));
    }
    if t.status == "void" {
        o.push(center("*** VOID ***"));
    }
    o.push("-".repeat(W));
    o.push(line("Ticket No:", &t.ticket_no));
    o.push(line("Type:", &t.trans_type.to_uppercase()));
    o.push(line("Vehicle:", &t.vehicle_plate));
    if !t.driver_name.is_empty() {
        o.push(line("Driver:", &t.driver_name));
    }
    if !t.party_name.is_empty() {
        o.push(line(if t.trans_type == "purchase" { "Supplier:" } else { "Customer:" }, &t.party_name));
    }
    if !t.product_name.is_empty() {
        o.push(line("Product:", &t.product_name));
    }
    if !t.source.is_empty() {
        o.push(line("From:", &t.source));
    }
    if !t.destination.is_empty() {
        o.push(line("To:", &t.destination));
    }
    if !t.reference.is_empty() {
        o.push(line("Ref:", &t.reference));
    }
    o.push("-".repeat(W));
    let fmt_dt = |d: &DateTime<Utc>| d.with_timezone(&chrono::Local).format("%d/%m/%Y %H:%M").to_string();
    o.push(line(&format!("1st {}", fmt_dt(&t.first_weight_at)), &kg(t.first_weight_kg)));
    if let (Some(w), Some(at)) = (t.second_weight_kg, t.second_weight_at) {
        o.push(line(&format!("2nd {}", fmt_dt(&at)), &kg(w)));
        let (g, tare) = t.gross_tare();
        o.push("-".repeat(W));
        o.push(line("GROSS:", &kg(g)));
        o.push(line("TARE:", &kg(tare.unwrap_or(0.0))));
        o.push(line("NET:", &kg(t.net().unwrap_or(0.0))));
    } else {
        o.push(center("(awaiting second weight)"));
    }
    if t.first_manual || t.second_manual {
        o.push(center("Manual weight entry"));
    }
    if t.charges > 0.0 {
        o.push(line("Charges:", &format!("{:.2}", t.charges)));
    }
    if !t.memo.is_empty() {
        o.push(format!("Note: {}", t.memo));
    }
    o.push("-".repeat(W));
    o.push(line("Operator:", &t.operator_name));
    o.push(String::new());
    o.push("Driver sign: ______________________".into());
    o.push(String::new());
    if let Some(s) = site {
        o.push(center(&s.ticket_footer));
    }
    o.join("\n")
}

/// 12345.0 -> "12,345"
pub fn group(v: f64) -> String {
    let neg = v < 0.0;
    let s = format!("{:.0}", v.abs());
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    if neg {
        format!("-{out}")
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn plates_and_grouping() {
        assert_eq!(normalize_plate("kbc 123a"), "KBC123A");
        assert_eq!(group(1234567.0), "1,234,567");
        assert_eq!(group(980.0), "980");
    }
}

/// Argon2id password hashing, shared so stations can verify logins offline.
pub mod password {
    use argon2::password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
    use argon2::Argon2;

    pub fn hash(pw: &str) -> String {
        Argon2::default().hash_password(pw.as_bytes(), &SaltString::generate(&mut OsRng)).expect("argon2").to_string()
    }

    pub fn verify(pw: &str, hash: &str) -> bool {
        PasswordHash::new(hash).map(|h| Argon2::default().verify_password(pw.as_bytes(), &h).is_ok()).unwrap_or(false)
    }
}
