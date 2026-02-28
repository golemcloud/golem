fn init {
  let _ = a => a += 1
  let _ = a => a[0] = 1
  let _ = a => raise a
  let _ = a => ...
  let _ = (a, b) => a += b
  let _ = (a, b) => a[0] = b
  let _ = (a, b) => raise a
  let _ = (a, b) => ...
  letrec f = a => raise a
}