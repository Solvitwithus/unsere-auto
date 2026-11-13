# How the code fits together

This guide follows one truck from the indicator all the way to a KRA-signed invoice,
naming the file that handles each step. Each source file also starts with a comment
explaining its purpose.

## 1. Reading the indicator (`crates/scale`)

| Step | File | What happens |
|---|---|---|
| Open the link | `transport.rs` | `Transport::Serial { port, baud, framing }` or `Transport::Tcp { host, port }`. DTR/RTS are switched off, as in the old app. |
| Cut frames | `profile.rs` `CompiledProfile::split` | The byte stream is cut at the profile's separators (CR, LF, STX, ETX by default; Yaohua also uses `=`). |
| Parse a frame | `profile.rs` `parse` | Either a regex with a `weight` group (plus `sign`, `unit`, stable/unstable markers), or the binary Toledo status-byte format. Units are converted to kg. |
| Motion check | `stability.rs` | A reading is **stable** only if the indicator doesn't report motion **and** the weight stayed within `tolerance_kg` for `window_ms`. |
| Background loop | `engine.rs` `run` | Reconnects forever, keeps the last 4 KB of raw bytes, publishes `ScaleState` on a `watch` channel. 3 s without a valid frame means "not receiving". |
| Auto-detect | `detect.rs` `detect_serial` | Listens on every port × {9600, 2400, 4800, 1200, 19200} × {8N1, 7E1}, scores every profile, best match first. |
| Learn mode | `detect.rs` `learn` | Given raw bytes and the number on the display, finds which number in the frame it is (also reversed digits, ×10ⁿ) and builds a regex profile. |

Built-in profiles: `toledo`, `st-gs`, `yaohua-reversed`, `weight-unit`, `sign6`, `generic-number`.
A new indicator family needs only a new profile (data), not a code change.

## 2. Weighing at the station (`crates/station`)

| Step | File | What happens |
|---|---|---|
| Start-up | `main.rs` | Opens `station.db` (SQLite, WAL, `synchronous=FULL` so a power cut loses nothing), runs migrations, starts the scale reader and sync loop, serves the API and the UI. |
| Settings | `config.rs` | `station.json`: server URL, station key, scale settings, printer. Scale settings pushed from the dashboard replace the local ones when they change. |
| Login | `api.rs` `login` | Checks against users synced from the server (Argon2 hashes), so login works offline. Sessions are kept in memory. |
| Live weight | `api.rs` `ws_weight` | WebSocket `/ws/weight` streams `ScaleState` to the screen. |
| First weight | `api.rs` `first_weight` | Refuses a second open ticket for the same plate. The weight comes from `take_weight` (live, stable, in range), never from the browser. Ticket number `STATIONCODE-000123`. |
| Second weight | `api.rs` `second_weight` | Completes the ticket, net = \|first − second\|, stores the vehicle's empty weight for next time. |
| Single pass | `first_weight` with `use_stored_tare` | Uses the stored tare as the first weight and the live reading as the second. |
| Printing | `print.rs` | ESC/POS to a network (9100), serial or file printer, or the browser print dialog. |
| Sync | `sync.rs` | Every 10 s (and right after each ticket): push tickets with `dirty=1`, then pull users, items, vehicles, office voids and scale settings. |

## 3. Central server (`crates/server`)

| Area | File | Notes |
|---|---|---|
| Start-up and commands | `main.rs` | `serve`, `create-admin`, `apply-preset`, `import`. Migrations use the direct (non-pooler) Neon URL. |
| Auth | `auth.rs` | Dashboard: JWT (12 h), checked against `users.active` on every request. Stations: `X-Station-Key: wbk_<id>_<secret>`; only the SHA-256 of the secret is stored. |
| Station sync | `routes/sync.rs` | Push = upsert, newest `updated_at` wins, and site/station always come from the key. A typed plate becomes a vehicle. Optional auto-invoice. |
| Tickets and reports | `routes/tickets.rs` | List and filter, CSV export, void (blocked when a KRA-signed invoice exists), dashboard summary. |
| Sales | `routes/sales.rs` | eTIMS-format invoices from a ticket (product line in TNE/KG plus weighing-fee line) or entered by hand. |
| eTIMS | `etims.rs` | Background sender: `POST {ETIMS_URL}/invoices` (sale number as idempotency key), then polls `/invoices/{id}/receipt` until it is signed. Failures retry with backoff. |
| Users, sites, items, stations | `routes/masters.rs` | CRUD plus the audit log. |
| Presets and import | `import.rs` | `apply-preset presets/x.json` creates a site and station; `import <kind> file.csv` accepts the old field names. |

### Sale format (`crates/core/src/sale.rs`)

Identical to `etims-rust`'s `POST /invoices`: `idempotencyKey`, `buyer{name,tin,phone,email}`,
`sale{type,paymentType,date,status,remark}`, `items[]` (`itemCode`, or `name` + `classification` +
`vatRate` + units, with `quantity`, `unitPrice`, discount), `pricesIncludeTax` and `actor`.
The VAT maths is a copy of `etims-rust/src/kra/tax.rs`, with JavaScript `Math.round` rounding,
and has the same unit tests.

## 4. Data model (`crates/server/migrations/0001_init.sql`)

`sites` (client company) → `stations` (weighbridge PCs, with `scale_config` JSON) →
`tickets` (one per truck visit) → `sales` + `sale_items` (eTIMS invoices).
Masters: `parties` (customers and suppliers, KRA PIN), `products` (price, unit, eTIMS item fields),
`vehicles` (stored tare). Plus `users`, `audit_log` and `counters` (invoice numbers without gaps).

Ticket IDs are UUIDs created at the station, so offline tickets never clash.

## 5. Why the main choices were made

- **SQLite at the station, PostgreSQL at the centre.** The station must keep working
  with no network and no database administrator. SQLite is a single file inside the
  app, and with WAL and full sync it survives power cuts.
- **Weights are captured by the station, not the browser.** The screen asks for "the
  current weight" and the station decides whether it is stable. A typed weight is
  possible only for supervisors, only when enabled, and is flagged on the ticket.
- **Profiles are data.** Supporting a new indicator never needs a new build of the program.
