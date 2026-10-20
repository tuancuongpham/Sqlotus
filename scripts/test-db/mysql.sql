CREATE TABLE customers (id INT AUTO_INCREMENT PRIMARY KEY, name VARCHAR(100) NOT NULL, country CHAR(2) NULL);
CREATE TABLE orders (id INT AUTO_INCREMENT PRIMARY KEY, customer_id INT NOT NULL, total DECIMAL(12,2) NOT NULL, order_date DATE NOT NULL, CONSTRAINT fk_orders_customers FOREIGN KEY (customer_id) REFERENCES customers(id));
INSERT INTO customers (name, country) VALUES ('Nguyễn Văn A', 'VN'), ('John Smith', 'US');
INSERT INTO orders (customer_id, total, order_date) VALUES (1, 150.50, '2024-05-01'), (2, 999.99, '2024-07-03');
CREATE VIEW customer_totals AS SELECT c.id, c.name, SUM(o.total) AS total FROM customers c JOIN orders o ON o.customer_id = c.id GROUP BY c.id, c.name;
DELIMITER //
CREATE PROCEDURE get_customer_orders(IN p_customer_id INT) BEGIN SELECT * FROM orders WHERE customer_id = p_customer_id; END //
DELIMITER ;
