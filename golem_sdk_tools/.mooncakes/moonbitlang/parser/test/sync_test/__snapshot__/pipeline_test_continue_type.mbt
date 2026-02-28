fn f(i: Int) -> String {
  for {
    if i == 0 {
      continue
    } else {
      return "xxx"
    }
  }
}


fn init {
  let _ = f(42)
}
