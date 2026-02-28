suberror StringError {
  StringError(String)
} derive(Show)

fn f() -> Unit raise StringError {
  raise StringError("xxx")
}

fn init {
  let tests : Array[() -> Unit raise StringError] = [f]
  for k = 0; k < tests.length(); k = k + 1 {
    let f = tests[k]
    let result = try? f()
    println(result)
  }
}
