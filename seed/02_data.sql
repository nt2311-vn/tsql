-- Sample ERP data: 4 customers, 6 products, 5 sales orders with line items,
-- linked work orders, invoices, and payments. Numbers are deterministic so
-- tests and screenshots stay stable across runs.

INSERT INTO customers (id, name, email, created_at) VALUES
    (1, 'Acme Industrial',     'orders@acme.test',      '2025-01-05 09:15:00'),
    (2, 'Beacon Logistics',    'ap@beacon.test',        '2025-02-12 14:30:00'),
    (3, 'Cobalt Robotics',     'finance@cobalt.test',   '2025-03-03 11:00:00'),
    (4, 'Delta Manufacturing', 'purchasing@delta.test', '2025-04-18 16:45:00');

INSERT INTO products (id, sku, name, unit_price) VALUES
    (1, 'WIDGET-A',   'Standard Widget',         12.50),
    (2, 'WIDGET-B',   'Heavy-Duty Widget',       29.00),
    (3, 'GEAR-S',     'Steel Gear, 12-tooth',    45.75),
    (4, 'GEAR-T',     'Titanium Gear, 24-tooth', 189.99),
    (5, 'BEARING-1',  'Roller Bearing 6204',      8.20),
    (6, 'CONTROL-XR', 'XR Controller Module',   349.00);

INSERT INTO sales_orders (id, order_number, customer_id, order_date, status, total_amount) VALUES
    (1, 'SO-1001', 1, '2025-05-02 10:00:00', 'shipped',   687.50),
    (2, 'SO-1002', 2, '2025-05-08 13:20:00', 'shipped',  1279.95),
    (3, 'SO-1003', 3, '2025-05-15 09:45:00', 'confirmed', 698.00),
    (4, 'SO-1004', 1, '2025-05-21 11:10:00', 'draft',     250.00),
    (5, 'SO-1005', 4, '2025-05-28 15:00:00', 'cancelled', 379.99);

INSERT INTO sales_order_items (id, sales_order_id, product_id, quantity, unit_price, line_total) VALUES
    (1, 1, 1, 30, 12.50,  375.00),
    (2, 1, 5, 15,  8.20,  123.00),
    (3, 1, 3,  4, 45.75,  183.00),
    (4, 2, 4,  5,189.99,  949.95),
    (5, 2, 6,  1,330.00,  330.00),
    (6, 3, 2, 20, 29.00,  580.00),
    (7, 3, 5, 10,  8.20,   82.00),
    (8, 3, 1,  3, 12.00,   36.00),
    (9, 4, 1, 20, 12.50,  250.00),
    (10,5, 4,  2,189.99,  379.98);

INSERT INTO work_orders (id, work_order_number, sales_order_id, status, scheduled_start, scheduled_end, completed_at) VALUES
    (1, 'WO-2001', 1, 'completed',   '2025-05-03 08:00:00', '2025-05-04 17:00:00', '2025-05-04 16:20:00'),
    (2, 'WO-2002', 2, 'completed',   '2025-05-09 08:00:00', '2025-05-12 17:00:00', '2025-05-12 14:05:00'),
    (3, 'WO-2003', 3, 'in_progress', '2025-05-16 08:00:00', '2025-05-19 17:00:00', NULL),
    (4, 'WO-2004', 3, 'planned',     '2025-05-20 08:00:00', '2025-05-21 17:00:00', NULL),
    (5, 'WO-2005', 5, 'cancelled',   '2025-05-29 08:00:00', '2025-05-30 17:00:00', NULL);

INSERT INTO invoices (id, invoice_number, sales_order_id, customer_id, issued_date, due_date, status, total_amount) VALUES
    (1, 'INV-3001', 1, 1, '2025-05-05 09:00:00', '2025-06-04 09:00:00', 'paid',    687.50),
    (2, 'INV-3002', 2, 2, '2025-05-13 09:00:00', '2025-06-12 09:00:00', 'paid',   1279.95),
    (3, 'INV-3003', 3, 3, '2025-05-20 09:00:00', '2025-06-19 09:00:00', 'sent',    698.00),
    (4, 'INV-3004', 5, 4, '2025-05-29 09:00:00', '2025-06-28 09:00:00', 'void',    379.99);

INSERT INTO payments (id, invoice_id, paid_at, amount, method) VALUES
    (1, 1, '2025-05-15 10:30:00',  687.50, 'bank_transfer'),
    (2, 2, '2025-05-20 11:00:00',  500.00, 'card'),
    (3, 2, '2025-06-01 14:15:00',  779.95, 'bank_transfer'),
    (4, 3, '2025-06-10 09:45:00',  200.00, 'cheque');
