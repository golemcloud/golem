;; The measurement contains no capabilities. Drop glue retains these imports,
;; but calling any of them must fail instead of hiding a resource operation.
(module
  (func (export "[resource-drop]schema-value-stream") (param i32) unreachable)
  (func (export "[resource-drop]secret") (param i32) unreachable)
  (func (export "[resource-drop]quota-token") (param i32) unreachable)
  (func (export "[resource-drop]permission-card") (param i32) unreachable))
