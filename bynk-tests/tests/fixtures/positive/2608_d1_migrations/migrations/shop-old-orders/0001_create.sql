-- #2608: no `D1` instance's database is `shop-old-orders` (`shop.orders`'s `Orders` is `shop-orders-orders`), so this is never
-- applied, and the build warns.
CREATE TABLE old (id TEXT);
