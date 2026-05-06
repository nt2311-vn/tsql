-- Lite ERP schema. Portable across Postgres 17 and SQLite 3.
-- Loaded automatically by Postgres via /docker-entrypoint-initdb.d when the
-- container is created (see docker-compose.yml). Apply to SQLite with
-- `just seed-sqlite` to exercise the same data through the SQLite driver.

CREATE TABLE customers (
    id          INTEGER PRIMARY KEY,
    name        TEXT NOT NULL,
    email       TEXT NOT NULL UNIQUE,
    created_at  TIMESTAMP NOT NULL
);

CREATE TABLE products (
    id          INTEGER PRIMARY KEY,
    sku         TEXT NOT NULL UNIQUE,
    name        TEXT NOT NULL,
    unit_price  NUMERIC(12, 2) NOT NULL CHECK (unit_price >= 0)
);

CREATE TABLE sales_orders (
    id            INTEGER PRIMARY KEY,
    order_number  TEXT NOT NULL UNIQUE,
    customer_id   INTEGER NOT NULL REFERENCES customers (id),
    order_date    TIMESTAMP NOT NULL,
    status        TEXT NOT NULL CHECK (status IN ('draft', 'confirmed', 'shipped', 'cancelled')),
    total_amount  NUMERIC(12, 2) NOT NULL CHECK (total_amount >= 0)
);

CREATE INDEX idx_sales_orders_customer ON sales_orders (customer_id);

CREATE TABLE sales_order_items (
    id              INTEGER PRIMARY KEY,
    sales_order_id  INTEGER NOT NULL REFERENCES sales_orders (id) ON DELETE CASCADE,
    product_id      INTEGER NOT NULL REFERENCES products (id),
    quantity        INTEGER NOT NULL CHECK (quantity > 0),
    unit_price      NUMERIC(12, 2) NOT NULL CHECK (unit_price >= 0),
    line_total      NUMERIC(12, 2) NOT NULL CHECK (line_total >= 0)
);

CREATE INDEX idx_sales_order_items_order ON sales_order_items (sales_order_id);

CREATE TABLE work_orders (
    id                INTEGER PRIMARY KEY,
    work_order_number TEXT NOT NULL UNIQUE,
    sales_order_id    INTEGER NOT NULL REFERENCES sales_orders (id),
    status            TEXT NOT NULL CHECK (status IN ('planned', 'in_progress', 'completed', 'cancelled')),
    scheduled_start   TIMESTAMP NOT NULL,
    scheduled_end     TIMESTAMP NOT NULL,
    completed_at      TIMESTAMP
);

CREATE INDEX idx_work_orders_sales_order ON work_orders (sales_order_id);

CREATE TABLE invoices (
    id              INTEGER PRIMARY KEY,
    invoice_number  TEXT NOT NULL UNIQUE,
    sales_order_id  INTEGER NOT NULL REFERENCES sales_orders (id),
    customer_id     INTEGER NOT NULL REFERENCES customers (id),
    issued_date     TIMESTAMP NOT NULL,
    due_date        TIMESTAMP NOT NULL,
    status          TEXT NOT NULL CHECK (status IN ('draft', 'sent', 'paid', 'overdue', 'void')),
    total_amount    NUMERIC(12, 2) NOT NULL CHECK (total_amount >= 0)
);

CREATE INDEX idx_invoices_customer ON invoices (customer_id);
CREATE INDEX idx_invoices_sales_order ON invoices (sales_order_id);

CREATE TABLE payments (
    id          INTEGER PRIMARY KEY,
    invoice_id  INTEGER NOT NULL REFERENCES invoices (id) ON DELETE CASCADE,
    paid_at     TIMESTAMP NOT NULL,
    amount      NUMERIC(12, 2) NOT NULL CHECK (amount > 0),
    method      TEXT NOT NULL CHECK (method IN ('cash', 'card', 'bank_transfer', 'cheque'))
);

CREATE INDEX idx_payments_invoice ON payments (invoice_id);
