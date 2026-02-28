test {
  // This will test whether the byte literal is correctly in the first token set of simple expression
  assert_eq((b'\xFF' << 8).to_int(), 0)
}
