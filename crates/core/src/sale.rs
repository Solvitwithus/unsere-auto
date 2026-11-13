//! Sales in the eTIMS format used by `etims-rust` (`POST /invoices`).
//!
//! The VAT math is a line-for-line copy of `etims-rust/src/kra/tax.rs`
//! (itself a port of the Node `tax.utils.ts`): f64 with JavaScript
//! `Math.round` semantics, rounded at exactly the same steps, so amounts here
//! match what eTIMS computes to the cent.
//!
//! KRA field names:
//!   prc       unit price (VAT inclusive)
//!   splyAmt   supply amount = quantity x price, before discount
//!   dcAmt     discount amount
//!   taxblAmt  taxable amount = supply - discount
//!   taxAmt    VAT on the taxable amount
//!   totAmt    line total

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use uuid::Uuid;

pub const SALE_TYPES: &[&str] = &["NORMAL", "CREDIT_NOTE", "PROFORMA", "TRAINING"];
pub const SALE_STATUSES: &[&str] = &["APPROVED", "CANCELED"];

/// KRA tax types: code, VAT %, label.
pub const TAX_TYPES: &[(&str, f64, &str)] = &[
    ("A", 0.0, "A-Exempt"),
    ("B", 16.0, "B-16%"),
    ("C", 0.0, "C-Zero rated"),
    ("D", 0.0, "D-Non VAT"),
    ("E", 8.0, "E-8%"),
];

pub fn vat_rate_for(code: &str) -> f64 {
    TAX_TYPES.iter().find(|(c, _, _)| *c == code).map(|t| t.1).unwrap_or(16.0)
}

/// `Math.round` - rounds half up (towards +inf), unlike `f64::round`.
pub fn js_round(x: f64) -> f64 {
    (x + 0.5).floor()
}

/// `Math.round(n * 100) / 100`
pub fn r2(n: f64) -> f64 {
    js_round(n * 100.0) / 100.0
}

#[derive(Debug, Clone, Default)]
pub struct ItemTaxInput {
    pub quantity: f64,
    pub unit_price: f64,
    pub vat_rate: f64,
    pub discount_rate: f64,
    pub discount_amount: Option<f64>,
    pub tax_amount_override: Option<f64>,
    pub prices_include_tax: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct ItemTaxResult {
    pub prc: f64,
    pub sply_amt: f64,
    pub dc_amt: f64,
    pub taxbl_amt: f64,
    pub tax_amt: f64,
    pub tot_amt: f64,
}

/// Same as `etims-rust::kra::tax::calculate_item_amounts`.
pub fn calculate_item_amounts(input: &ItemTaxInput) -> ItemTaxResult {
    let ItemTaxInput { quantity, unit_price, vat_rate, discount_rate, .. } = *input;
    let multiplier = 1.0 + vat_rate / 100.0;
    if input.prices_include_tax {
        let prc = r2(unit_price);
        let gross_tot_amt = r2(quantity * unit_price);
        let tot_amt = r2(if let Some(d) = input.discount_amount {
            quantity * unit_price - d
        } else if discount_rate > 0.0 {
            gross_tot_amt * (1.0 - discount_rate / 100.0)
        } else {
            gross_tot_amt
        });
        let taxbl_amt = r2(tot_amt / multiplier);
        let tax_amt = r2(tot_amt - taxbl_amt);
        let sply_amt = r2(gross_tot_amt / multiplier);
        let dc_amt = r2(sply_amt - taxbl_amt);
        ItemTaxResult { prc, sply_amt, dc_amt, taxbl_amt, tax_amt, tot_amt }
    } else {
        let sply_amt = r2(quantity * unit_price);
        let dc_amt = if let Some(d) = input.discount_amount {
            r2(d)
        } else if discount_rate > 0.0 {
            r2(sply_amt * discount_rate / 100.0)
        } else {
            0.0
        };
        let taxbl_amt = r2(sply_amt - dc_amt);
        let tax_amt = match input.tax_amount_override {
            Some(t) => r2(t),
            None => r2(taxbl_amt * vat_rate / 100.0),
        };
        let tot_amt = r2(taxbl_amt + tax_amt);
        let prc = if quantity > 0.0 { r2(tot_amt / quantity) } else { r2(unit_price * multiplier) };
        ItemTaxResult { prc, sply_amt, dc_amt, taxbl_amt, tax_amt, tot_amt }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Totals {
    pub taxable_amount: f64,
    pub tax_amount: f64,
    pub total_amount: f64,
}

pub fn calculate_invoice_totals(items: &[ItemTaxResult]) -> Totals {
    Totals {
        taxable_amount: r2(items.iter().fold(0.0, |s, i| s + i.taxbl_amt)),
        tax_amount: r2(items.iter().fold(0.0, |s, i| s + i.tax_amt)),
        total_amount: r2(items.iter().fold(0.0, |s, i| s + i.tot_amt)),
    }
}

/// Buyer block, as in eTIMS (`buyer.name` <= 60, `tin` = KRA PIN).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Buyer {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tin: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phone: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
}

/// One requested line (eTIMS `items[]`). Either `item_code` (a registered
/// eTIMS item) or `name` + `classification` + `unit_price`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaleLineInput {
    #[serde(default)]
    pub product_id: Option<Uuid>,
    #[serde(default)]
    pub item_code: Option<String>,
    #[serde(default)]
    pub classification: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    pub quantity: f64,
    #[serde(default)]
    pub unit_price: Option<f64>,
    #[serde(default)]
    pub discount_rate: f64,
    #[serde(default)]
    pub discount_amount: Option<f64>,
    #[serde(default)]
    pub vat_rate: Option<f64>,
    #[serde(default)]
    pub tax_type_code: Option<String>,
    #[serde(default)]
    pub packaging_unit: Option<String>,
    #[serde(default)]
    pub quantity_unit: Option<String>,
}

/// A fully resolved and priced line, as stored.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct SaleItem {
    pub seq: i32,
    pub product_id: Option<Uuid>,
    pub item_code: Option<String>,
    pub classification: String,
    pub name: String,
    pub quantity: f64,
    pub quantity_unit: String,
    pub packaging_unit: String,
    pub unit_price: f64,
    pub discount_rate: f64,
    pub discount_amount: Option<f64>,
    pub vat_rate: f64,
    pub tax_type_code: String,
    pub prc: f64,
    pub sply_amt: f64,
    pub dc_amt: f64,
    pub taxbl_amt: f64,
    pub tax_amt: f64,
    pub tot_amt: f64,
}

impl SaleItem {
    /// Price a line. Missing VAT rate comes from the tax type (default B, 16%).
    pub fn price(seq: i32, l: &SaleLineInput, prices_include_tax: bool) -> Result<SaleItem, String> {
        let name = l.name.clone().unwrap_or_default();
        if name.trim().is_empty() && l.item_code.as_deref().unwrap_or("").is_empty() {
            return Err(format!("line {seq}: give itemCode or name"));
        }
        if !(l.quantity > 0.0) {
            return Err(format!("line {seq}: quantity must be positive"));
        }
        let unit_price = l.unit_price.ok_or(format!("line {seq}: unitPrice is required"))?;
        if unit_price < 0.0 {
            return Err(format!("line {seq}: unitPrice can't be negative"));
        }
        let tax_type_code = l.tax_type_code.clone().filter(|s| !s.is_empty()).unwrap_or_else(|| "B".into());
        let vat_rate = l.vat_rate.unwrap_or_else(|| vat_rate_for(&tax_type_code));
        // KRA sends quantities as NUMBER(13,2)
        let quantity = r2(l.quantity);
        if let Some(d) = l.discount_amount {
            if js_round(d * 100.0) > js_round(r2(quantity * unit_price) * 100.0) {
                return Err(format!("line {seq}: discount is more than the line total"));
            }
        }
        let calc = calculate_item_amounts(&ItemTaxInput {
            quantity,
            unit_price,
            vat_rate,
            discount_rate: l.discount_rate,
            discount_amount: l.discount_amount,
            tax_amount_override: None,
            prices_include_tax,
        });
        let discount_rate = match l.discount_amount {
            Some(_) if calc.sply_amt > 0.0 => r2(calc.dc_amt / calc.sply_amt * 100.0),
            Some(_) => 0.0,
            None => l.discount_rate,
        };
        Ok(SaleItem {
            seq,
            product_id: l.product_id,
            item_code: l.item_code.clone().filter(|s| !s.is_empty()),
            classification: l.classification.clone().unwrap_or_default(),
            name,
            quantity,
            quantity_unit: l.quantity_unit.clone().filter(|s| !s.is_empty()).unwrap_or_else(|| "U".into()),
            packaging_unit: l.packaging_unit.clone().filter(|s| !s.is_empty()).unwrap_or_else(|| "NT".into()),
            unit_price,
            discount_rate,
            discount_amount: l.discount_amount,
            vat_rate,
            tax_type_code,
            prc: calc.prc,
            sply_amt: calc.sply_amt,
            dc_amt: calc.dc_amt,
            taxbl_amt: calc.taxbl_amt,
            tax_amt: calc.tax_amt,
            tot_amt: calc.tot_amt,
        })
    }

    pub fn amounts(&self) -> ItemTaxResult {
        ItemTaxResult {
            prc: self.prc,
            sply_amt: self.sply_amt,
            dc_amt: self.dc_amt,
            taxbl_amt: self.taxbl_amt,
            tax_amt: self.tax_amt,
            tot_amt: self.tot_amt,
        }
    }
}

/// A sale (invoice) header as stored.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct Sale {
    pub id: Uuid,
    /// Our number; sent to eTIMS as `idempotencyKey` (<= 30 chars).
    pub sale_no: String,
    pub ticket_id: Option<Uuid>,
    pub site_id: Option<Uuid>,
    pub sale_type: String,
    pub status: String,
    pub payment_type: String,
    pub sale_date: DateTime<Utc>,
    pub buyer_name: String,
    pub buyer_tin: Option<String>,
    pub buyer_phone: Option<String>,
    pub buyer_email: Option<String>,
    pub party_id: Option<Uuid>,
    pub prices_include_tax: bool,
    pub taxable_amount: f64,
    pub tax_amount: f64,
    pub total_amount: f64,
    pub remark: Option<String>,
    pub actor_name: String,
    pub actor_id: String,
    /// DISABLED (no eTIMS configured) | PENDING | SUBMITTED | SIGNED | FAILED
    pub etims_status: String,
    pub etims_invoice_id: Option<String>,
    pub etims_invoice_code: Option<String>,
    /// The `receipt.scu` block returned by eTIMS (cuInvoiceNo, internalData, receiptSignature, qrUrl, ...)
    pub etims_scu: Option<Value>,
    pub etims_error: Option<String>,
    pub etims_attempts: i32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

fn cut(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// The exact `POST /invoices` body etims-rust expects.
pub fn etims_request(sale: &Sale, items: &[SaleItem], control_unit_id: Option<&str>) -> Value {
    let mut body = Map::new();
    if let Some(cu) = control_unit_id.filter(|s| !s.is_empty()) {
        body.insert("controlUnitId".into(), json!(cu));
    }
    body.insert("idempotencyKey".into(), json!(cut(&sale.sale_no, 30)));
    let mut buyer = Map::new();
    buyer.insert("name".into(), json!(cut(if sale.buyer_name.trim().is_empty() { "Walk-in Customer" } else { &sale.buyer_name }, 60)));
    if let Some(t) = sale.buyer_tin.as_deref().filter(|s| !s.is_empty()) {
        buyer.insert("tin".into(), json!(t.to_uppercase()));
    }
    if let Some(p) = sale.buyer_phone.as_deref().filter(|s| !s.is_empty()) {
        buyer.insert("phone".into(), json!(cut(p, 20)));
    }
    if let Some(e) = sale.buyer_email.as_deref().filter(|s| !s.is_empty()) {
        buyer.insert("email".into(), json!(cut(e, 50)));
    }
    body.insert("buyer".into(), Value::Object(buyer));
    let mut s = Map::new();
    s.insert("type".into(), json!(sale.sale_type));
    s.insert("paymentType".into(), json!(sale.payment_type));
    s.insert("date".into(), json!(sale.sale_date.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)));
    s.insert("status".into(), json!(sale.status));
    if let Some(r) = sale.remark.as_deref().filter(|s| !s.is_empty()) {
        s.insert("remark".into(), json!(cut(r, 400)));
    }
    body.insert("sale".into(), Value::Object(s));
    let lines: Vec<Value> = items
        .iter()
        .map(|i| {
            let mut m = Map::new();
            match &i.item_code {
                Some(code) => {
                    m.insert("itemCode".into(), json!(cut(code, 20)));
                }
                None => {
                    m.insert("name".into(), json!(cut(&i.name, 200)));
                    m.insert("classification".into(), json!(cut(&i.classification, 20)));
                    m.insert("vatRate".into(), num(i.vat_rate));
                    m.insert("packagingUnit".into(), json!(cut(&i.packaging_unit, 5)));
                    m.insert("quantityUnit".into(), json!(cut(&i.quantity_unit, 5)));
                }
            }
            m.insert("quantity".into(), num(i.quantity));
            m.insert("unitPrice".into(), num(i.unit_price));
            match i.discount_amount {
                Some(d) => {
                    m.insert("discountAmount".into(), num(d));
                }
                None if i.discount_rate > 0.0 => {
                    m.insert("discountRate".into(), num(i.discount_rate));
                }
                None => {}
            }
            Value::Object(m)
        })
        .collect();
    body.insert("items".into(), Value::Array(lines));
    body.insert("pricesIncludeTax".into(), json!(sale.prices_include_tax));
    body.insert("actor".into(), json!({ "name": cut(&sale.actor_name, 60), "id": cut(&sale.actor_id, 20) }));
    Value::Object(body)
}

/// A number as JSON.stringify would write it (`1000`, not `1000.0`).
pub fn num(v: f64) -> Value {
    if v.is_finite() && v.fract() == 0.0 && v.abs() < 9_007_199_254_740_992.0 {
        Value::from(v as i64)
    } else {
        json!(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The same cases as etims-rust/src/kra/tax.rs
    #[test]
    fn ex_vat() {
        let r = calculate_item_amounts(&ItemTaxInput { quantity: 2.0, unit_price: 500.0, vat_rate: 16.0, ..Default::default() });
        assert_eq!((r.sply_amt, r.taxbl_amt, r.tax_amt, r.tot_amt, r.prc), (1000.0, 1000.0, 160.0, 1160.0, 580.0));
    }

    #[test]
    fn vat_inclusive_pins_total() {
        let r = calculate_item_amounts(&ItemTaxInput { quantity: 1.0, unit_price: 5000.0, vat_rate: 16.0, prices_include_tax: true, ..Default::default() });
        assert_eq!((r.tot_amt, r.sply_amt, r.tax_amt), (5000.0, 4310.34, 689.66));
    }

    #[test]
    fn discounts_add_up() {
        let inc = |q: f64, p: f64, d: Option<f64>| {
            calculate_item_amounts(&ItemTaxInput { quantity: q, unit_price: p, vat_rate: 16.0, discount_amount: d, prices_include_tax: true, ..Default::default() })
        };
        let r = inc(10.0, 300.0, Some(750.0));
        assert_eq!((r.taxbl_amt, r.tax_amt, r.tot_amt), (1939.66, 310.34, 2250.0));
        let r = inc(4.0, 300.0, Some(180.0));
        assert_eq!((r.taxbl_amt, r.tax_amt, r.tot_amt), (879.31, 140.69, 1020.0));
    }

    #[test]
    fn tonnage_line_and_request_shape() {
        let l = SaleLineInput {
            name: Some("Ballast 3/4".into()),
            classification: Some("1111150000".into()),
            quantity: 12.345,
            unit_price: Some(1500.0),
            quantity_unit: Some("TNE".into()),
            ..Default::default()
        };
        let item = SaleItem::price(1, &l, true).unwrap();
        assert_eq!(item.quantity, 12.35);
        assert_eq!(item.tot_amt, 18525.0);
        assert_eq!(r2(item.taxbl_amt + item.tax_amt), item.tot_amt);
        let now = Utc::now();
        let sale = Sale {
            id: Uuid::new_v4(), sale_no: "WB-MAIN-000001".into(), ticket_id: None, site_id: None,
            sale_type: "NORMAL".into(), status: "APPROVED".into(), payment_type: "Cash".into(), sale_date: now,
            buyer_name: "".into(), buyer_tin: Some("p051234567x".into()), buyer_phone: None, buyer_email: None, party_id: None,
            prices_include_tax: true, taxable_amount: item.taxbl_amt, tax_amount: item.tax_amt, total_amount: item.tot_amt,
            remark: None, actor_name: "Jane".into(), actor_id: "jane".into(), etims_status: "PENDING".into(),
            etims_invoice_id: None, etims_invoice_code: None, etims_scu: None, etims_error: None, etims_attempts: 0,
            created_at: now, updated_at: now,
        };
        let body = etims_request(&sale, &[item], None);
        assert_eq!(body["idempotencyKey"], "WB-MAIN-000001");
        assert_eq!(body["buyer"]["name"], "Walk-in Customer");
        assert_eq!(body["buyer"]["tin"], "P051234567X");
        assert_eq!(body["items"][0]["quantity"], json!(12.35));
        assert_eq!(body["items"][0]["unitPrice"], json!(1500));
        assert_eq!(body["items"][0]["quantityUnit"], "TNE");
        assert_eq!(body["pricesIncludeTax"], true);
    }
}
