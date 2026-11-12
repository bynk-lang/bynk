-- The project keeps a MySQL schema, which switches the statement check on.
CREATE TABLE orders (id BIGINT UNSIGNED AUTO_INCREMENT PRIMARY KEY, name VARCHAR(64) NOT NULL UNIQUE, qty INT NOT NULL DEFAULT 1);
