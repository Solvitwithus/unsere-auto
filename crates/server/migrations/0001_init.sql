-- Weighbridge central schema

CREATE TABLE sites (
    id          UUID PRIMARY KEY,
    code        TEXT NOT NULL UNIQUE,
    name        TEXT NOT NULL,
    address     TEXT NOT NULL DEFAULT '',
    phone       TEXT NOT NULL DEFAULT '',
    ticket_footer TEXT NOT NULL DEFAULT 'Thank you',
    kra_pin     TEXT NOT NULL DEFAULT '',
    -- create a sale automatically when a priced 'sale' ticket is completed
    auto_invoice BOOLEAN NOT NULL DEFAULT FALSE,
    prices_include_tax BOOLEAN NOT NULL DEFAULT TRUE,
    weighing_fee NUMERIC(14,2) NOT NULL DEFAULT 0,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE users (
    id            UUID PRIMARY KEY,
    username      TEXT NOT NULL UNIQUE,
    full_name     TEXT NOT NULL DEFAULT '',
    password_hash TEXT NOT NULL,
    role          TEXT NOT NULL CHECK (role IN ('admin', 'supervisor', 'operator', 'viewer')),
    site_id       UUID REFERENCES sites(id),
    active        BOOLEAN NOT NULL DEFAULT TRUE,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Customers and suppliers share one table (the old app had both).
CREATE TABLE parties (
    id          UUID PRIMARY KEY,
    code        TEXT UNIQUE,
    name        TEXT NOT NULL,
    kind        TEXT NOT NULL DEFAULT 'customer' CHECK (kind IN ('customer', 'supplier', 'both')),
    tin         TEXT,
    phone       TEXT NOT NULL DEFAULT '',
    email       TEXT NOT NULL DEFAULT '',
    address     TEXT NOT NULL DEFAULT '',
    active      BOOLEAN NOT NULL DEFAULT TRUE,
    legacy_ref  TEXT,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX parties_name_idx ON parties (lower(name));

CREATE TABLE products (
    id            UUID PRIMARY KEY,
    code          TEXT UNIQUE,
    name          TEXT NOT NULL,
    -- eTIMS item fields (same meaning as in etims-rust)
    unit_price          NUMERIC(14,2) NOT NULL DEFAULT 0,
    quantity_unit       TEXT NOT NULL DEFAULT 'TNE',
    item_code           TEXT,
    classification_code TEXT NOT NULL DEFAULT '',
    tax_type_code       TEXT NOT NULL DEFAULT 'B' CHECK (tax_type_code IN ('A','B','C','D','E')),
    packaging_unit      TEXT NOT NULL DEFAULT 'NT',
    active        BOOLEAN NOT NULL DEFAULT TRUE,
    legacy_ref    TEXT,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE vehicles (
    id              UUID PRIMARY KEY,
    plate           TEXT NOT NULL UNIQUE,
    stored_tare_kg  DOUBLE PRECISION,
    tare_updated_at TIMESTAMPTZ,
    party_id        UUID REFERENCES parties(id),
    driver_name     TEXT NOT NULL DEFAULT '',
    active          BOOLEAN NOT NULL DEFAULT TRUE,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE stations (
    id            UUID PRIMARY KEY,
    site_id       UUID NOT NULL REFERENCES sites(id),
    code          TEXT NOT NULL UNIQUE,
    name          TEXT NOT NULL,
    key_hash      TEXT NOT NULL,
    scale_config  JSONB,
    last_seen_at  TIMESTAMPTZ,
    last_status   JSONB,
    active        BOOLEAN NOT NULL DEFAULT TRUE,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- One row per weighing transaction. IDs are generated at the station so
-- tickets can be created offline and synced later without conflicts.
CREATE TABLE tickets (
    id                UUID PRIMARY KEY,
    ticket_no         TEXT NOT NULL UNIQUE,
    site_id           UUID REFERENCES sites(id),
    station_id        UUID REFERENCES stations(id),
    trans_type        TEXT NOT NULL CHECK (trans_type IN ('purchase', 'sale', 'transfer', 'other')),
    status            TEXT NOT NULL CHECK (status IN ('open', 'complete', 'void')),
    vehicle_plate     TEXT NOT NULL,
    driver_name       TEXT NOT NULL DEFAULT '',
    party_id          UUID REFERENCES parties(id),
    party_name        TEXT NOT NULL DEFAULT '',
    product_id        UUID REFERENCES products(id),
    product_name      TEXT NOT NULL DEFAULT '',
    source            TEXT NOT NULL DEFAULT '',
    destination       TEXT NOT NULL DEFAULT '',
    reference         TEXT NOT NULL DEFAULT '',
    memo              TEXT NOT NULL DEFAULT '',
    first_weight_kg   DOUBLE PRECISION NOT NULL,
    first_weight_at   TIMESTAMPTZ NOT NULL,
    first_raw         TEXT NOT NULL DEFAULT '',
    first_manual      BOOLEAN NOT NULL DEFAULT FALSE,
    second_weight_kg  DOUBLE PRECISION,
    second_weight_at  TIMESTAMPTZ,
    second_raw        TEXT NOT NULL DEFAULT '',
    second_manual     BOOLEAN NOT NULL DEFAULT FALSE,
    net_weight_kg     DOUBLE PRECISION GENERATED ALWAYS AS (abs(second_weight_kg - first_weight_kg)) STORED,
    charges           NUMERIC(14,2) NOT NULL DEFAULT 0,
    operator_id       UUID,
    operator_name     TEXT NOT NULL DEFAULT '',
    void_reason       TEXT NOT NULL DEFAULT '',
    print_count       INTEGER NOT NULL DEFAULT 0,
    legacy_ref        TEXT,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    synced_at         TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX tickets_created_idx ON tickets (created_at DESC);
CREATE INDEX tickets_site_idx ON tickets (site_id, created_at DESC);
CREATE INDEX tickets_plate_idx ON tickets (vehicle_plate);
CREATE INDEX tickets_status_idx ON tickets (status) WHERE status = 'open';
CREATE INDEX tickets_updated_idx ON tickets (updated_at);

CREATE TABLE audit_log (
    id         BIGSERIAL PRIMARY KEY,
    at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    actor      TEXT NOT NULL,
    action     TEXT NOT NULL,
    entity     TEXT NOT NULL,
    entity_id  TEXT NOT NULL DEFAULT '',
    details    JSONB NOT NULL DEFAULT '{}'::jsonb
);
CREATE INDEX audit_at_idx ON audit_log (at DESC);

-- Sales in the eTIMS invoice format (etims-rust POST /invoices).
CREATE TABLE sales (
    id                  UUID PRIMARY KEY,
    sale_no             TEXT NOT NULL UNIQUE CHECK (length(sale_no) <= 30),
    ticket_id           UUID REFERENCES tickets(id),
    site_id             UUID REFERENCES sites(id),
    sale_type           TEXT NOT NULL DEFAULT 'NORMAL' CHECK (sale_type IN ('NORMAL','CREDIT_NOTE','PROFORMA','TRAINING')),
    status              TEXT NOT NULL DEFAULT 'APPROVED' CHECK (status IN ('APPROVED','CANCELED')),
    payment_type        TEXT NOT NULL DEFAULT 'Cash',
    sale_date           TIMESTAMPTZ NOT NULL DEFAULT now(),
    buyer_name          TEXT NOT NULL DEFAULT '',
    buyer_tin           TEXT,
    buyer_phone         TEXT,
    buyer_email         TEXT,
    party_id            UUID REFERENCES parties(id),
    prices_include_tax  BOOLEAN NOT NULL DEFAULT TRUE,
    taxable_amount      NUMERIC(14,2) NOT NULL,
    tax_amount          NUMERIC(14,2) NOT NULL,
    total_amount        NUMERIC(14,2) NOT NULL,
    remark              TEXT,
    actor_name          TEXT NOT NULL DEFAULT 'System',
    actor_id            TEXT NOT NULL DEFAULT 'SYSTEM',
    etims_status        TEXT NOT NULL DEFAULT 'DISABLED' CHECK (etims_status IN ('DISABLED','PENDING','SUBMITTED','SIGNED','FAILED')),
    etims_invoice_id    TEXT,
    etims_invoice_code  TEXT,
    etims_scu           JSONB,
    etims_response      JSONB,
    etims_error         TEXT,
    etims_attempts      INTEGER NOT NULL DEFAULT 0,
    etims_next_try_at   TIMESTAMPTZ,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX sales_ticket_uq ON sales (ticket_id) WHERE ticket_id IS NOT NULL AND status = 'APPROVED' AND sale_type = 'NORMAL';
CREATE INDEX sales_date_idx ON sales (sale_date DESC);
CREATE INDEX sales_etims_due_idx ON sales (etims_next_try_at) WHERE etims_status IN ('PENDING','SUBMITTED','FAILED');

CREATE TABLE sale_items (
    sale_id         UUID NOT NULL REFERENCES sales(id) ON DELETE CASCADE,
    seq             INTEGER NOT NULL,
    product_id      UUID REFERENCES products(id),
    item_code       TEXT,
    classification  TEXT NOT NULL DEFAULT '',
    name            TEXT NOT NULL,
    quantity        NUMERIC(13,2) NOT NULL,
    quantity_unit   TEXT NOT NULL,
    packaging_unit  TEXT NOT NULL,
    unit_price      NUMERIC(14,2) NOT NULL,
    discount_rate   NUMERIC(6,2) NOT NULL DEFAULT 0,
    discount_amount NUMERIC(14,2),
    vat_rate        NUMERIC(5,2) NOT NULL,
    tax_type_code   TEXT NOT NULL,
    prc             NUMERIC(14,2) NOT NULL,
    sply_amt        NUMERIC(14,2) NOT NULL,
    dc_amt          NUMERIC(14,2) NOT NULL,
    taxbl_amt       NUMERIC(14,2) NOT NULL,
    tax_amt         NUMERIC(14,2) NOT NULL,
    tot_amt         NUMERIC(14,2) NOT NULL,
    PRIMARY KEY (sale_id, seq)
);

CREATE TABLE counters (
    name   TEXT PRIMARY KEY,
    value  BIGINT NOT NULL
);
