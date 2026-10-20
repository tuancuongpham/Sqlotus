CREATE DATABASE shop;
\c shop
CREATE SCHEMA sales;
CREATE TABLE customers (id serial PRIMARY KEY, name text NOT NULL, country char(2), created_at timestamptz NOT NULL DEFAULT now());
CREATE TABLE orders (id serial PRIMARY KEY, customer_id int NOT NULL REFERENCES customers(id), total numeric(12,2) NOT NULL, order_date date NOT NULL);
CREATE TABLE sales.regions (code char(2) PRIMARY KEY, name text NOT NULL);
INSERT INTO customers (name, country) VALUES ('Nguyễn Văn A', 'VN'), ('John Smith', 'US');
INSERT INTO orders (customer_id, total, order_date) VALUES (1, 150.50, '2024-05-01'), (2, 999.99, '2024-07-03');
CREATE VIEW customer_totals AS SELECT c.id, c.name, sum(o.total) AS total FROM customers c JOIN orders o ON o.customer_id = c.id GROUP BY c.id, c.name;
CREATE FUNCTION order_count(p_customer_id int) RETURNS bigint LANGUAGE sql AS $$ SELECT count(*) FROM orders WHERE customer_id = p_customer_id $$;
CREATE PROCEDURE touch_customer(p_id int) LANGUAGE plpgsql AS $$ BEGIN RAISE NOTICE 'touching %', p_id; UPDATE customers SET name = name WHERE id = p_id; END $$;
