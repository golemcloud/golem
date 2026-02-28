pub fn test_iff(b : Bool) -> Int {
  if (if b {
    true
  } else {
    false
  }) {
    1
  } else {
    2
  }
}
