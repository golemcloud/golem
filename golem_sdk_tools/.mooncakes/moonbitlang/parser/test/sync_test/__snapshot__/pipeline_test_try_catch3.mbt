fn init {
  let v = try { raise IntError(1 + 2) } catch { _ => 3}
  println(v)
}

suberror IntError {
  IntError(Int)
}
