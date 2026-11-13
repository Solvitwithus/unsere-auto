CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);

CREATE TABLE users (
    id BLOB PRIMARY KEY, username TEXT NOT NULL UNIQUE, full_name TEXT NOT NULL, password_hash TEXT NOT NULL,
    role TEXT NOT NULL, active INTEGER NOT NULL
);

CREATE TABLE parties (
    id BLOB PRIMARY KEY, code TEXT, name TEXT NOT NULL, kind TEXT NOT NULL, tin TEXT, phone TEXT NOT NULL, email TEXT NOT NULL,
    address TEXT NOT NULL, active INTEGER NOT NULL, updated_at TEXT NOT NULL
);

CREATE TABLE products (
    id BLOB PRIMARY KEY, code TEXT, name TEXT NOT NULL, unit_price REAL NOT NULL, quantity_unit TEXT NOT NULL, item_code TEXT,
    classification_code TEXT NOT NULL, tax_type_code TEXT NOT NULL, packaging_unit TEXT NOT NULL, active INTEGER NOT NULL, updated_at TEXT NOT NULL
);

CREATE TABLE vehicles (
    id BLOB PRIMARY KEY, plate TEXT NOT NULL UNIQUE, stored_tare_kg REAL, party_id BLOB, driver_name TEXT NOT NULL,
    active INTEGER NOT NULL, updated_at TEXT NOT NULL
);

CREATE TABLE tickets (
    id BLOB PRIMARY KEY, ticket_no TEXT NOT NULL UNIQUE, site_id BLOB, station_id BLOB, trans_type TEXT NOT NULL, status TEXT NOT NULL,
    vehicle_plate TEXT NOT NULL, driver_name TEXT NOT NULL, party_id BLOB, party_name TEXT NOT NULL, product_id BLOB, product_name TEXT NOT NULL,
    source TEXT NOT NULL, destination TEXT NOT NULL, reference TEXT NOT NULL, memo TEXT NOT NULL,
    first_weight_kg REAL NOT NULL, first_weight_at TEXT NOT NULL, first_raw TEXT NOT NULL, first_manual INTEGER NOT NULL,
    second_weight_kg REAL, second_weight_at TEXT, second_raw TEXT NOT NULL, second_manual INTEGER NOT NULL,
    net_weight_kg REAL, charges REAL NOT NULL, operator_id BLOB, operator_name TEXT NOT NULL, void_reason TEXT NOT NULL,
    print_count INTEGER NOT NULL, created_at TEXT NOT NULL, updated_at TEXT NOT NULL,
    dirty INTEGER NOT NULL DEFAULT 1
);
CREATE INDEX tickets_status ON tickets (status);
CREATE INDEX tickets_plate ON tickets (vehicle_plate);
CREATE INDEX tickets_dirty ON tickets (dirty) WHERE dirty = 1;
CREATE INDEX tickets_created ON tickets (created_at);
