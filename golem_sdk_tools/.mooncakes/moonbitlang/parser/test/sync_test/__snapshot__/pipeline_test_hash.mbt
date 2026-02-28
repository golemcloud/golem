fn[T : Hash] stable_hash(value : T) -> Int {
  let hasher = Hasher::new(seed = 42)
  hasher.combine(value)
  hasher.finalize()
}

fn init {
  println(stable_hash(""))
  println(stable_hash("a"))
  println(stable_hash("abc"))
  println(stable_hash("message digest"))
  println(stable_hash("abcdefghijklmnopqrstuvwxyz"))
  println(
    stable_hash("ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789"),
  )
  println(
    stable_hash("12345678901234567890123456789012345678901234567890123456789012345678901234567890"),
  )
  println(stable_hash(false))
  println(stable_hash((123)))
  println(stable_hash(4614253070214989087L))
  println(stable_hash(3.14))
}

