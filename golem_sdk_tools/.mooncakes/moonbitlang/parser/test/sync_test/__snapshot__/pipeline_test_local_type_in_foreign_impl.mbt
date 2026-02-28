struct T(Int) derive(Show)

// local type inside an `impl` for foreign trait
impl @json.FromJson for T with from_json(json, _path) {
  struct S { x : Int }
  // the `if` is used to surpress unbox optimization
  let s : S = if json == 42 {
    { x: 42 }
  } else {
    { x: 24 }
  }
  s.x
}

fn init {
  println((try? @json.from_json(42) : Result[T, _]))
}
