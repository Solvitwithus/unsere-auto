# Weighbridge

The replacement for the old Java/Swing weighbridge app (`../sourcedir`). There is
**one** program for every client: the seven copied `weighBridge*.java` screens are
now settings for each site and station.

- **Manual (install, setup, daily use):** [`docs/Weighbridge-Manual.pdf`](docs/Weighbridge-Manual.pdf)
- **How the code fits together:** [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md)

```
 WEIGHBRIDGE PC                                   CENTRAL SERVER
┌──────────────────────────────┐   HTTPS sync   ┌──────────────────────────────┐
│ wb-station                   │ ─────────────► │ wb-server (Rust, axum)       │
│  scale engine ◄── indicator  │  tickets up    │  PostgreSQL (Neon)           │
│  SQLite (keeps working       │ ◄───────────── │  dashboard (web/out)         │
│   offline)                   │  users, items, │  sales ──► etims-rust ──► KRA│
│  operator screen (web/out)   │  scale settings│                              │
│  ticket printer              │                │                              │
└──────────────────────────────┘                └──────────────────────────────┘
```

| Folder | What it is |
|---|---|
| `crates/scale` | `wb-scale`: talks to any indicator over serial, USB or TCP. Protocol profiles, stability check, auto-detect, learn mode. Also the `wb-scale` command-line tool and the `wb-sim` simulator. |
| `crates/core` | Types shared by server and station: tickets, the eTIMS sale format and VAT maths (copied from `etims-rust`), ticket layout, password hashing. |
| `crates/server` | `wb-server`: the central API and dashboard host, eTIMS sender, client presets and CSV import. |
| `crates/station` | `wb-station`: runs on the weighbridge PC. Local database, scale reader, operator screen, printing, sync. |
| `web` | Next.js UI, exported as static files. The same build serves both the dashboard and the operator screen. |
| `presets` | One file per old client variant (Joy, Mama, Farisi, Spenza, Uwezo, Jirani). |
| `deploy` | systemd units, nginx config, Windows install script. |

## Quick start (development)

```bash
cp .env.example .env                       # set DATABASE_URL and JWT_SECRET
(cd web && pnpm install && pnpm build)     # UI -> web/out
cargo run -p wb-server -- create-admin --username admin --password 'something-long'
cargo run -p wb-server                     # http://localhost:8080 (migrates the DB first)

# a fake indicator, then a station that reads it
cargo run -p wb-scale --bin wb-sim -- --format toledo --tcp 127.0.0.1:4001
cargo run -p wb-station -- --data-dir station-data --listen 127.0.0.1:7878
```

In the dashboard, add a site and a station whose scale is set to network `127.0.0.1:4001`
with the Toledo protocol. Then open http://127.0.0.1:7878 and enter the server address
and the station key.

## Tests

```bash
cargo test --workspace     # protocol parsing, stability, auto-detect, learn mode, VAT maths, import
```
