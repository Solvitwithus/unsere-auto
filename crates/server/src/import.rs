//! Client presets and CSV import from the old system.

use crate::state::AppState;
use chrono::{DateTime, FixedOffset, NaiveDate, NaiveDateTime, TimeZone, Utc};
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;
use uuid::Uuid;
use wb_core::normalize_plate;

// ---------------- presets ----------------

#[derive(Deserialize)]
struct Preset {
    site: PresetSite,
    station: PresetStation,
}

#[derive(Deserialize)]
struct PresetSite {
    code: String,
    name: String,
    #[serde(default)]
    address: String,
    #[serde(default)]
    phone: String,
    #[serde(default)]
    kra_pin: String,
    #[serde(default)]
    ticket_footer: Option<String>,
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

#[derive(Deserialize)]
struct PresetStation {
    code: String,
    name: String,
    scale_config: Value,
}

pub async fn apply_preset(st: &AppState, file: &str) -> anyhow::Result<()> {
    let p: Preset = serde_json::from_str(&std::fs::read_to_string(file)?)?;
    let site_id: Uuid = sqlx::query_scalar(
        "INSERT INTO sites (id, code, name, address, phone, kra_pin, ticket_footer, auto_invoice, prices_include_tax, weighing_fee)
         VALUES ($1,$2,$3,$4,$5,$6,COALESCE($7,'Thank you'),$8,$9,$10)
         ON CONFLICT (code) DO UPDATE SET name=EXCLUDED.name, address=EXCLUDED.address, phone=EXCLUDED.phone, kra_pin=EXCLUDED.kra_pin,
             ticket_footer=EXCLUDED.ticket_footer, auto_invoice=EXCLUDED.auto_invoice, prices_include_tax=EXCLUDED.prices_include_tax,
             weighing_fee=EXCLUDED.weighing_fee, updated_at=now()
         RETURNING id",
    )
    .bind(Uuid::new_v4()).bind(p.site.code.to_uppercase()).bind(&p.site.name).bind(&p.site.address).bind(&p.site.phone)
    .bind(p.site.kra_pin.to_uppercase()).bind(&p.site.ticket_footer).bind(p.site.auto_invoice).bind(p.site.prices_include_tax).bind(p.site.weighing_fee)
    .fetch_one(&st.db).await?;
    println!("site {} ({}) ready", p.site.code.to_uppercase(), p.site.name);

    let code = p.station.code.to_uppercase();
    let existing: Option<Uuid> = sqlx::query_scalar("SELECT id FROM stations WHERE code=$1").bind(&code).fetch_optional(&st.db).await?;
    match existing {
        Some(id) => {
            sqlx::query("UPDATE stations SET site_id=$2, name=$3, scale_config=$4, updated_at=now() WHERE id=$1")
                .bind(id).bind(site_id).bind(&p.station.name).bind(&p.station.scale_config).execute(&st.db).await?;
            println!("station {code} updated (scale config pushed on its next sync). Its key is unchanged; rotate it in the dashboard if needed.");
        }
        None => {
            let id = Uuid::new_v4();
            let (key, hash) = crate::auth::new_station_key(id);
            sqlx::query("INSERT INTO stations (id, site_id, code, name, key_hash, scale_config) VALUES ($1,$2,$3,$4,$5,$6)")
                .bind(id).bind(site_id).bind(&code).bind(&p.station.name).bind(hash).bind(&p.station.scale_config).execute(&st.db).await?;
            println!("station {code} created. Station key (enter it in the station setup; shown once):\n\n  {key}\n");
        }
    }
    Ok(())
}

// ---------------- CSV import ----------------

/// Header -> canonical name. Headers are compared lower-case without punctuation.
fn canonical(kind: &str, header: &str) -> Option<&'static str> {
    let h: String = header.to_lowercase().chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    let m: &[(&str, &[&str])] = match kind {
        "parties" => &[
            ("name", &["name", "suppliername", "clientname", "customername", "sourcename", "destname", "companyname"]),
            ("code", &["code", "suppliercode", "clientcode", "customercode", "accountno", "id"]),
            ("kind", &["kind", "type", "category"]),
            ("tin", &["tin", "pin", "krapin", "pinno", "taxpin"]),
            ("phone", &["phone", "tel", "telephone", "mobile", "phoneno", "contact"]),
            ("email", &["email", "emailaddress"]),
            ("address", &["address", "location", "postaladdress", "town"]),
        ],
        "products" => &[
            ("name", &["name", "itemname", "description", "productname", "item"]),
            ("code", &["code", "itemcode", "productcode", "id"]),
            ("unit_price", &["price", "unitprice", "pricepertonne", "priceperton", "sellingprice", "rate"]),
            ("quantity_unit", &["unit", "quantityunit", "uom"]),
            ("item_code", &["etimsitemcode", "kraitemcode"]),
            ("classification_code", &["classification", "classificationcode", "itemclassificationcode"]),
            ("tax_type_code", &["taxtype", "taxtypecode", "taxcode", "vatcode"]),
        ],
        "vehicles" => &[
            ("plate", &["plate", "vehicle", "vehicleno", "regno", "registration", "vehicleplate", "truck"]),
            ("tare", &["tare", "tareweight", "storedtarekg", "taredweight"]),
            ("driver", &["driver", "drivername"]),
        ],
        "tickets" => &[
            ("ticket_no", &["transno", "receiptno", "ticketno", "ticket", "serialno", "no"]),
            ("trans_type", &["transtype", "type", "transactiontype"]),
            ("vehicle", &["vehicle", "vehicleno", "vehicleplate", "plate", "regno", "truck"]),
            ("driver", &["driver", "drivername"]),
            ("source", &["sourcename", "source", "from"]),
            ("destination", &["destname", "destination", "to"]),
            ("product", &["product", "item", "itemname", "productname", "material"]),
            ("memo", &["memo", "remarks", "notes", "comment"]),
            ("first_weight", &["firstweight", "weight1", "grossweight", "first"]),
            ("second_weight", &["secondweight", "weight2", "tareweight", "second"]),
            ("charges", &["charges", "fee", "amount"]),
            ("date", &["date", "transdate", "createdat", "firstweightdate", "firstweightat", "timestamp", "datetime"]),
            ("second_date", &["secondweightdate", "secondweightat", "seconddate"]),
            ("operator", &["operator", "user", "username", "createdby"]),
            ("reference", &["reference", "ref", "loadingorder", "orderno", "transid"]),
        ],
        _ => &[],
    };
    m.iter().find(|(_, aliases)| aliases.contains(&h.as_str())).map(|(c, _)| *c)
}

fn parse_dt(s: &str) -> Option<DateTime<Utc>> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    if let Ok(d) = DateTime::parse_from_rfc3339(s) {
        return Some(d.with_timezone(&Utc));
    }
    // naive times in the old system are Nairobi local time
    let eat = FixedOffset::east_opt(3 * 3600).unwrap();
    for f in ["%Y-%m-%d %H:%M:%S%.f", "%Y-%m-%d %H:%M:%S", "%Y-%m-%d %H:%M", "%d/%m/%Y %H:%M:%S", "%d/%m/%Y %H:%M", "%m/%d/%Y %H:%M"] {
        if let Ok(n) = NaiveDateTime::parse_from_str(s, f) {
            return eat.from_local_datetime(&n).single().map(|d| d.with_timezone(&Utc));
        }
    }
    for f in ["%Y-%m-%d", "%d/%m/%Y"] {
        if let Ok(d) = NaiveDate::parse_from_str(s, f) {
            return eat.from_local_datetime(&d.and_hms_opt(12, 0, 0)?).single().map(|d| d.with_timezone(&Utc));
        }
    }
    None
}

fn num(s: &str) -> Option<f64> {
    let c: String = s.chars().filter(|c| c.is_ascii_digit() || *c == '.' || *c == '-').collect();
    c.parse().ok()
}

fn trans_type(s: &str) -> &'static str {
    let s = s.to_lowercase();
    if s.starts_with("purch") || s.starts_with("in") || s == "p" {
        "purchase"
    } else if s.starts_with("sal") || s.starts_with("out") || s == "s" {
        "sale"
    } else if s.starts_with("trans") {
        "transfer"
    } else {
        "other"
    }
}

pub async fn run(st: &AppState, kind: &str, file: &str, site: Option<&str>, dry_run: bool) -> anyhow::Result<()> {
    let mut rdr = csv::ReaderBuilder::new().flexible(true).trim(csv::Trim::All).from_path(file)?;
    let headers = rdr.headers()?.clone();
    let map: Vec<Option<&str>> = headers.iter().map(|h| canonical(kind, h)).collect();
    anyhow::ensure!(map.iter().any(|m| m.is_some()), "no recognised columns for '{kind}' in {file}. Headers: {:?}", headers);
    println!("columns: {}", headers.iter().zip(&map).map(|(h, m)| format!("{h}->{}", m.unwrap_or("(ignored)"))).collect::<Vec<_>>().join(", "));

    let site_id: Option<Uuid> = match site {
        Some(code) => Some(
            sqlx::query_scalar("SELECT id FROM sites WHERE code=$1").bind(code.to_uppercase()).fetch_optional(&st.db).await?
                .ok_or_else(|| anyhow::anyhow!("no site with code {code}"))?,
        ),
        None => None,
    };
    let (mut read, mut ok, mut skipped) = (0, 0, 0);
    let mut problems: Vec<String> = Vec::new();
    for (i, rec) in rdr.records().enumerate() {
        read += 1;
        let rec = match rec {
            Ok(r) => r,
            Err(e) => {
                skipped += 1;
                problems.push(format!("row {}: {e}", i + 2));
                continue;
            }
        };
        let mut row: HashMap<&str, String> = HashMap::new();
        for (v, m) in rec.iter().zip(&map) {
            if let Some(m) = m {
                row.entry(m).or_insert_with(|| v.to_string());
            }
        }
        let g = |k: &str| row.get(k).cloned().unwrap_or_default();
        let res: anyhow::Result<bool> = async {
            match kind {
                "parties" => {
                    let name = g("name");
                    anyhow::ensure!(!name.is_empty(), "no name");
                    if dry_run { return Ok(true); }
                    let kind = match g("kind").to_lowercase() { k if k.starts_with("sup") => "supplier", k if k.starts_with("both") => "both", _ => "customer" };
                    let code = Some(g("code")).filter(|s| !s.is_empty());
                    let exists: Option<Uuid> = sqlx::query_scalar("SELECT id FROM parties WHERE (code IS NOT NULL AND code=$1) OR lower(name)=lower($2) LIMIT 1")
                        .bind(&code).bind(&name).fetch_optional(&st.db).await?;
                    if exists.is_some() { return Ok(false); }
                    let tin = Some(g("tin").to_uppercase()).filter(|s| s.len() == 11);
                    sqlx::query("INSERT INTO parties (id, code, name, kind, tin, phone, email, address, legacy_ref) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)")
                        .bind(Uuid::new_v4()).bind(&code).bind(&name).bind(kind).bind(tin).bind(g("phone")).bind(g("email")).bind(g("address")).bind(&code)
                        .execute(&st.db).await?;
                    Ok(true)
                }
                "products" => {
                    let name = g("name");
                    anyhow::ensure!(!name.is_empty(), "no name");
                    if dry_run { return Ok(true); }
                    let code = Some(g("code")).filter(|s| !s.is_empty());
                    let exists: Option<Uuid> = sqlx::query_scalar("SELECT id FROM products WHERE (code IS NOT NULL AND code=$1) OR lower(name)=lower($2) LIMIT 1")
                        .bind(&code).bind(&name).fetch_optional(&st.db).await?;
                    if exists.is_some() { return Ok(false); }
                    let tax = Some(g("tax_type_code").to_uppercase()).filter(|t| ["A","B","C","D","E"].contains(&t.as_str())).unwrap_or_else(|| "B".into());
                    let unit = Some(g("quantity_unit").to_uppercase()).filter(|u| !u.is_empty() && u.len() <= 5).unwrap_or_else(|| "TNE".into());
                    sqlx::query("INSERT INTO products (id, code, name, unit_price, quantity_unit, item_code, classification_code, tax_type_code, legacy_ref) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)")
                        .bind(Uuid::new_v4()).bind(&code).bind(&name).bind(num(&g("unit_price")).unwrap_or(0.0)).bind(unit)
                        .bind(Some(g("item_code")).filter(|s| !s.is_empty())).bind(g("classification_code")).bind(tax).bind(&code)
                        .execute(&st.db).await?;
                    Ok(true)
                }
                "vehicles" => {
                    let plate = normalize_plate(&g("plate"));
                    anyhow::ensure!(plate.len() >= 3, "no plate");
                    if dry_run { return Ok(true); }
                    let tare = num(&g("tare")).filter(|t| *t > 0.0);
                    let r = sqlx::query("INSERT INTO vehicles (id, plate, stored_tare_kg, tare_updated_at, driver_name) VALUES ($1,$2,$3,CASE WHEN $3 IS NULL THEN NULL ELSE now() END,$4) ON CONFLICT (plate) DO NOTHING")
                        .bind(Uuid::new_v4()).bind(&plate).bind(tare).bind(g("driver")).execute(&st.db).await?;
                    Ok(r.rows_affected() > 0)
                }
                "tickets" => {
                    let legacy = g("ticket_no");
                    anyhow::ensure!(!legacy.is_empty(), "no ticket/receipt number");
                    let plate = normalize_plate(&g("vehicle"));
                    anyhow::ensure!(!plate.is_empty(), "no vehicle");
                    let first = num(&g("first_weight")).ok_or_else(|| anyhow::anyhow!("no first weight"))?;
                    let second = num(&g("second_weight")).filter(|w| *w > 0.0);
                    let at = parse_dt(&g("date")).ok_or_else(|| anyhow::anyhow!("bad or missing date '{}'", g("date")))?;
                    let second_at = second.map(|_| parse_dt(&g("second_date")).unwrap_or(at));
                    let tt = trans_type(&g("trans_type"));
                    let (source, dest) = (g("source"), g("destination"));
                    // supplier on purchases, customer on sales
                    let party_name = match tt { "purchase" => source.clone(), "sale" => dest.clone(), _ => String::new() };
                    if dry_run { return Ok(true); }
                    let product = g("product");
                    let product_id: Option<Uuid> = if product.is_empty() { None } else {
                        sqlx::query_scalar("SELECT id FROM products WHERE lower(name)=lower($1) OR code=$1 LIMIT 1").bind(&product).fetch_optional(&st.db).await?
                    };
                    let party_id: Option<Uuid> = if party_name.is_empty() { None } else {
                        sqlx::query_scalar("SELECT id FROM parties WHERE lower(name)=lower($1) LIMIT 1").bind(&party_name).fetch_optional(&st.db).await?
                    };
                    sqlx::query("INSERT INTO vehicles (id, plate) VALUES ($1,$2) ON CONFLICT (plate) DO NOTHING").bind(Uuid::new_v4()).bind(&plate).execute(&st.db).await?;
                    let r = sqlx::query(
                        "INSERT INTO tickets (id, ticket_no, site_id, trans_type, status, vehicle_plate, driver_name, party_id, party_name, product_id, product_name,
                            source, destination, reference, memo, first_weight_kg, first_weight_at, first_raw, second_weight_kg, second_weight_at, charges,
                            operator_name, legacy_ref, created_at, updated_at)
                         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,'imported',$18,$19,$20,$21,$22,$23,$23)
                         ON CONFLICT (ticket_no) DO NOTHING",
                    )
                    .bind(Uuid::new_v4()).bind(format!("OLD-{legacy}")).bind(site_id).bind(tt).bind(if second.is_some() { "complete" } else { "open" })
                    .bind(&plate).bind(g("driver")).bind(party_id).bind(&party_name).bind(product_id).bind(&product).bind(&source).bind(&dest)
                    .bind(g("reference")).bind(g("memo")).bind(first).bind(at).bind(second).bind(second_at).bind(num(&g("charges")).unwrap_or(0.0))
                    .bind(g("operator")).bind(&legacy).bind(at)
                    .execute(&st.db).await?;
                    Ok(r.rows_affected() > 0)
                }
                other => anyhow::bail!("unknown import kind '{other}' (parties, products, vehicles, tickets)"),
            }
        }
        .await;
        match res {
            Ok(true) => ok += 1,
            Ok(false) => skipped += 1,
            Err(e) => {
                skipped += 1;
                if problems.len() < 30 {
                    problems.push(format!("row {}: {e}", i + 2));
                }
            }
        }
    }
    println!("{kind}: {read} rows read, {ok} {}, {skipped} skipped (already there or invalid)", if dry_run { "valid" } else { "imported" });
    for p in problems {
        println!("  {p}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn headers_and_values() {
        assert_eq!(canonical("tickets", "first_weight"), Some("first_weight"));
        assert_eq!(canonical("tickets", "Dest Name"), Some("destination"));
        assert_eq!(trans_type("Purchases"), "purchase");
        assert_eq!(trans_type("Sales"), "sale");
        assert!(parse_dt("2024-03-01 14:05:00").is_some());
        assert!(parse_dt("01/03/2024 14:05").is_some());
        assert_eq!(num("12,340 kg"), Some(12340.0));
    }
}
