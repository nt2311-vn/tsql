-- Lite ERP schema, MySQL / MariaDB flavour.
-- Mirrors seed/01_schema.sql, with two engine-specific changes:
--   * TEXT replaced by sized VARCHARs so UNIQUE indexes don't trip
--     over key-prefix limits.
--   * Inline `REFERENCES` clauses promoted to explicit FOREIGN KEY
--     constraints — MySQL silently ignores inline REFERENCES, so the
--     ERD view would otherwise see zero edges.
-- Loaded automatically by the mysql container via /docker-entrypoint-initdb.d.

CREATE TABLE customers (
    id          INTEGER PRIMARY KEY,
    name        VARCHAR(255) NOT NULL,
    email       VARCHAR(191) NOT NULL UNIQUE,
    created_at  TIMESTAMP NOT NULL
);

CREATE TABLE products (
    id          INTEGER PRIMARY KEY,
    sku         VARCHAR(191) NOT NULL UNIQUE,
    name        VARCHAR(255) NOT NULL,
    unit_price  NUMERIC(12, 2) NOT NULL CHECK (unit_price >= 0)
);

CREATE TABLE sales_orders (
    id            INTEGER PRIMARY KEY,
    order_number  VARCHAR(191) NOT NULL UNIQUE,
    customer_id   INTEGER NOT NULL,
    order_date    TIMESTAMP NOT NULL,
    status        VARCHAR(32) NOT NULL CHECK (status IN ('draft', 'confirmed', 'shipped', 'cancelled')),
    total_amount  NUMERIC(12, 2) NOT NULL CHECK (total_amount >= 0),
    CONSTRAINT fk_sales_orders_customer FOREIGN KEY (customer_id) REFERENCES customers (id)
);

CREATE INDEX idx_sales_orders_customer ON sales_orders (customer_id);

CREATE TABLE sales_order_items (
    id              INTEGER PRIMARY KEY,
    sales_order_id  INTEGER NOT NULL,
    product_id      INTEGER NOT NULL,
    quantity        INTEGER NOT NULL CHECK (quantity > 0),
    unit_price      NUMERIC(12, 2) NOT NULL CHECK (unit_price >= 0),
    line_total      NUMERIC(12, 2) NOT NULL CHECK (line_total >= 0),
    CONSTRAINT fk_soi_order   FOREIGN KEY (sales_order_id) REFERENCES sales_orders (id) ON DELETE CASCADE,
    CONSTRAINT fk_soi_product FOREIGN KEY (product_id)     REFERENCES products (id)
);

CREATE INDEX idx_sales_order_items_order ON sales_order_items (sales_order_id);

CREATE TABLE work_orders (
    id                INTEGER PRIMARY KEY,
    work_order_number VARCHAR(191) NOT NULL UNIQUE,
    sales_order_id    INTEGER NOT NULL,
    status            VARCHAR(32) NOT NULL CHECK (status IN ('planned', 'in_progress', 'completed', 'cancelled')),
    scheduled_start   TIMESTAMP NOT NULL,
    scheduled_end     TIMESTAMP NOT NULL,
    completed_at      TIMESTAMP NULL,
    CONSTRAINT fk_work_orders_order FOREIGN KEY (sales_order_id) REFERENCES sales_orders (id)
);

CREATE INDEX idx_work_orders_sales_order ON work_orders (sales_order_id);

CREATE TABLE invoices (
    id              INTEGER PRIMARY KEY,
    invoice_number  VARCHAR(191) NOT NULL UNIQUE,
    sales_order_id  INTEGER NOT NULL,
    customer_id     INTEGER NOT NULL,
    issued_date     TIMESTAMP NOT NULL,
    due_date        TIMESTAMP NOT NULL,
    status          VARCHAR(32) NOT NULL CHECK (status IN ('draft', 'sent', 'paid', 'overdue', 'void')),
    total_amount    NUMERIC(12, 2) NOT NULL CHECK (total_amount >= 0),
    CONSTRAINT fk_invoices_order    FOREIGN KEY (sales_order_id) REFERENCES sales_orders (id),
    CONSTRAINT fk_invoices_customer FOREIGN KEY (customer_id)    REFERENCES customers (id)
);

CREATE INDEX idx_invoices_customer ON invoices (customer_id);
CREATE INDEX idx_invoices_sales_order ON invoices (sales_order_id);

CREATE TABLE payments (
    id          INTEGER PRIMARY KEY,
    invoice_id  INTEGER NOT NULL,
    paid_at     TIMESTAMP NOT NULL,
    amount      NUMERIC(12, 2) NOT NULL CHECK (amount > 0),
    method      VARCHAR(32) NOT NULL CHECK (method IN ('cash', 'card', 'bank_transfer', 'cheque')),
    CONSTRAINT fk_payments_invoice FOREIGN KEY (invoice_id) REFERENCES invoices (id) ON DELETE CASCADE
);

CREATE INDEX idx_payments_invoice ON payments (invoice_id);
