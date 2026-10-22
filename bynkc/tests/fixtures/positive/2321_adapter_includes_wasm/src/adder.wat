;; The module adder.wasm is assembled from (no wat2wasm in the toolchain; the
;; 41 bytes are written out by hand). Exports `add(i32, i32) -> i32`.
(module
  (func (export "add") (param i32 i32) (result i32)
    local.get 0
    local.get 1
    i32.add))
