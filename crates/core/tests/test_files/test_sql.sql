-- Comprehensive SQL test file

-- Drop existing tables
DROP TABLE IF EXISTS user_roles;
DROP TABLE IF EXISTS roles;
DROP TABLE IF EXISTS users;
DROP TABLE IF EXISTS orders;
DROP TABLE IF EXISTS products;
DROP TABLE IF EXISTS categories;

-- Create categories table
CREATE TABLE categories (
    id INT PRIMARY KEY AUTO_INCREMENT,
    name VARCHAR(100) NOT NULL,
    description TEXT,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP
);

-- Create products table
CREATE TABLE products (
    id INT PRIMARY KEY AUTO_INCREMENT,
    category_id INT,
    name VARCHAR(255) NOT NULL,
    description TEXT,
    price DECIMAL(10, 2) NOT NULL,
    stock INT DEFAULT 0,
    is_active BOOLEAN DEFAULT TRUE,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP,
    FOREIGN KEY (category_id) REFERENCES categories(id)
);

-- Create users table
CREATE TABLE users (
    id INT PRIMARY KEY AUTO_INCREMENT,
    username VARCHAR(50) UNIQUE NOT NULL,
    email VARCHAR(100) UNIQUE NOT NULL,
    password_hash VARCHAR(255) NOT NULL,
    first_name VARCHAR(50),
    last_name VARCHAR(50),
    age INT,
    is_active BOOLEAN DEFAULT TRUE,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP
);

-- Create roles table
CREATE TABLE roles (
    id INT PRIMARY KEY AUTO_INCREMENT,
    name VARCHAR(50) UNIQUE NOT NULL,
    description TEXT
);

-- Create user_roles junction table
CREATE TABLE user_roles (
    user_id INT,
    role_id INT,
    assigned_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (user_id, role_id),
    FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE,
    FOREIGN KEY (role_id) REFERENCES roles(id) ON DELETE CASCADE
);

-- Create orders table
CREATE TABLE orders (
    id INT PRIMARY KEY AUTO_INCREMENT,
    user_id INT NOT NULL,
    total DECIMAL(10, 2) NOT NULL,
    status ENUM('pending', 'processing', 'completed', 'cancelled') DEFAULT 'pending',
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP,
    FOREIGN KEY (user_id) REFERENCES users(id)
);

-- Insert categories
INSERT INTO categories (name, description) VALUES
    ('Electronics', 'Electronic devices and accessories'),
    ('Books', 'Books and publications'),
    ('Clothing', 'Clothing and apparel'),
    ('Home & Garden', 'Home and garden products');

-- Insert products
INSERT INTO products (category_id, name, description, price, stock) VALUES
    (1, 'Laptop', 'High-performance laptop', 999.99, 10),
    (1, 'Smartphone', 'Latest smartphone model', 699.99, 25),
    (2, 'Programming Book', 'Learn to program', 49.99, 100),
    (2, 'Novel', 'Bestselling novel', 19.99, 50),
    (3, 'T-Shirt', 'Cotton t-shirt', 19.99, 200),
    (3, 'Jeans', 'Denim jeans', 59.99, 75),
    (4, 'Garden Tools', 'Basic gardening tools', 89.99, 30);

-- Insert users
INSERT INTO users (username, email, password_hash, first_name, last_name, age, is_active) VALUES
    ('alice', 'alice@example.com', 'hash1', 'Alice', 'Johnson', 25, TRUE),
    ('bob', 'bob@example.com', 'hash2', 'Bob', 'Smith', 30, TRUE),
    ('charlie', 'charlie@example.com', 'hash3', 'Charlie', 'Brown', 35, FALSE);

-- Insert roles
INSERT INTO roles (name, description) VALUES
    ('admin', 'Administrator with full access'),
    ('user', 'Regular user'),
    ('moderator', 'Content moderator');

-- Insert user roles
INSERT INTO user_roles (user_id, role_id) VALUES
    (1, 1), -- alice is admin
    (1, 2), -- alice is also user
    (2, 2), -- bob is user
    (3, 2); -- charlie is user

-- Insert orders
INSERT INTO orders (user_id, total, status) VALUES
    (1, 1019.98, 'completed'),
    (1, 49.99, 'processing'),
    (2, 19.99, 'pending');

-- SELECT queries
SELECT * FROM users;
SELECT * FROM products WHERE price > 50 ORDER BY price DESC LIMIT 5;
SELECT u.username, o.total, o.status FROM users u JOIN orders o ON u.id = o.user_id;

-- JOIN examples
SELECT 
    p.name AS product,
    c.name AS category,
    p.price
FROM products p
INNER JOIN categories c ON p.category_id = c.id;

-- LEFT JOIN
SELECT 
    u.username,
    r.name AS role
FROM users u
LEFT JOIN user_roles ur ON u.id = ur.user_id
LEFT JOIN roles r ON ur.role_id = r.id;

-- Aggregate functions
SELECT 
    category_id,
    COUNT(*) AS product_count,
    AVG(price) AS avg_price,
    MIN(price) AS min_price,
    MAX(price) AS max_price,
    SUM(stock) AS total_stock
FROM products
GROUP BY category_id;

-- HAVING clause
SELECT 
    category_id,
    COUNT(*) AS product_count,
    AVG(price) AS avg_price
FROM products
GROUP BY category_id
HAVING COUNT(*) > 1;

-- Subquery
SELECT 
    name,
    price,
    (SELECT AVG(price) FROM products) AS overall_avg
FROM products
WHERE price > (SELECT AVG(price) FROM products);

-- UNION
SELECT username AS name FROM users
UNION
SELECT name AS name FROM roles;

-- CASE statement
SELECT 
    name,
    price,
    CASE 
        WHEN price < 50 THEN 'Low'
        WHEN price BETWEEN 50 AND 100 THEN 'Medium'
        ELSE 'High'
    END AS price_category
FROM products;

-- Window functions
SELECT 
    id,
    name,
    price,
    ROW_NUMBER() OVER (ORDER BY price) AS row_num,
    RANK() OVER (ORDER BY price) AS rank,
    DENSE_RANK() OVER (ORDER BY price) AS dense_rank,
    NTILE(4) OVER (ORDER BY price) AS quartile
FROM products;

-- CTE (Common Table Expression)
WITH high_value_products AS (
    SELECT * FROM products WHERE price > 100
)
SELECT 
    p.name,
    p.price,
    c.name AS category
FROM high_value_products p
JOIN categories c ON p.category_id = c.id;

-- UPDATE statement
UPDATE users SET last_login = CURRENT_TIMESTAMP WHERE id = 1;

-- DELETE statement
DELETE FROM user_roles WHERE user_id = 3;

-- Transaction example
START TRANSACTION;
INSERT INTO orders (user_id, total, status) VALUES (2, 59.99, 'pending');
UPDATE products SET stock = stock - 1 WHERE id = 6;
COMMIT;

-- Stored procedure (simplified)
DELIMITER //
CREATE PROCEDURE get_user_orders(IN user_id_param INT)
BEGIN
    SELECT 
        o.id AS order_id,
        o.total,
        o.status,
        o.created_at
    FROM orders o
    WHERE o.user_id = user_id_param
    ORDER BY o.created_at DESC;
END //
DELIMITER ;

-- Call stored procedure
CALL get_user_orders(1);

-- View creation
CREATE VIEW active_users AS
SELECT * FROM users WHERE is_active = TRUE;

-- Trigger example (simplified)
DELIMITER //
CREATE TRIGGER update_product_timestamp
BEFORE UPDATE ON products
FOR EACH ROW
BEGIN
    SET NEW.updated_at = CURRENT_TIMESTAMP;
END //
DELIMITER ;

-- Index creation
CREATE INDEX idx_products_category ON products(category_id);
CREATE INDEX idx_products_price ON products(price);
CREATE INDEX idx_users_email ON users(email);

-- Complex query with multiple joins
SELECT 
    u.username,
    p.name AS product,
    c.name AS category,
    o.total,
    o.status
FROM orders o
JOIN users u ON o.user_id = u.id
JOIN order_items oi ON o.id = oi.order_id
JOIN products p ON oi.product_id = p.id
JOIN categories c ON p.category_id = c.id
WHERE u.is_active = TRUE
ORDER BY o.created_at DESC;
