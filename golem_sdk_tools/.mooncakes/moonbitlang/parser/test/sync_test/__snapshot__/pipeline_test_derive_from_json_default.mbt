//!build: -w -27

struct MyType {
  a: Int
  b: Int
} derive(FromJson(default), Show, Eq)

struct MyTypeOnlyADefault {
  a: Int
  b: Int
} derive(FromJson(fields(a(default))), Show, Eq)

struct MyTypeOnlyADefault2 {
  a: Int
  b: Int
} derive(FromJson(fields(a(default=65536))), Show, Eq)

struct MyTypeOnlyADefault3 {
  a: Int
  b: Int
} derive(FromJson(fields(a(default=generate_default_a()))), Show, Eq)

let gen_func_calls : Ref[Int] = Ref::new(0)

fn generate_default_a() -> Int {
  gen_func_calls.update(fn(x) { x + 1 })
  65536
}

fn init {
  println("All fields set")
  let input : Json = { "a": 100, "b": 200 }
  let expected : Result[MyType, _] = Ok({ a: 100, b: 200 })
  let actual : Result[MyType, _] = try? @json.from_json(input)
  println("\{actual == expected}, \{actual}, \{expected}")
  //
  println("Struct default, only a set")
  let input : Json = { "a": 100 }
  let expected : Result[MyType, _] = Ok({ a: 100, b: 0 })
  let actual : Result[MyType, _] = try? @json.from_json(input)
  println("\{actual == expected}, \{actual}, \{expected}")
  //
  println("Struct default, only b set")
  let input : Json = { "b": 200 }
  let expected : Result[MyType, _] = Ok({ a: 0, b: 200 })
  let actual : Result[MyType, _] = try? @json.from_json(input)
  println("\{actual == expected}, \{actual}, \{expected}")
  //
  println("Struct default, both fields missing")
  let input : Json = {}
  let expected : Result[MyType, _] = Ok({ a: 0, b: 0 })
  let actual : Result[MyType, _] = try? @json.from_json(input)
  println("\{actual == expected}, \{actual}, \{expected}")
  //
  println("Field default, both fields set")
  let input : Json = { "a": 100, "b": 200 }
  let expected : Result[MyTypeOnlyADefault, _] = Ok({ a: 100, b: 200 })
  let actual : Result[MyTypeOnlyADefault, _] = try? @json.from_json(input)
  println("\{actual == expected}, \{actual}, \{expected}")
  //
  println("Field default, only b set")
  let input : Json = { "b": 200 }
  let expected : Result[MyTypeOnlyADefault, _] = Ok({ a: 0, b: 200 })
  let actual : Result[MyTypeOnlyADefault, _] = try? @json.from_json(input)
  println("\{actual == expected}, \{actual}, \{expected}")
  //
  println("Field default, only a set, should error")
  let input : Json = { "a": 100 }
  let actual : Result[MyTypeOnlyADefault, _] = try? @json.from_json(input)
  println("\{actual}")
  //
  println("Field default with value, both fields set")
  let input : Json = { "a": 100, "b": 200 }
  let expected : Result[MyTypeOnlyADefault2, _] = Ok({ a: 100, b: 200 })
  let actual : Result[MyTypeOnlyADefault2, _] = try? @json.from_json(input)
  println("\{actual == expected}, \{actual}, \{expected}")
  //
  println("Field default with value, only b set")
  let input : Json = { "b": 200 }
  let expected : Result[MyTypeOnlyADefault2, _] = Ok({ a: 65536, b: 200 })
  let actual : Result[MyTypeOnlyADefault2, _] = try? @json.from_json(input)
  println("\{actual == expected}, \{actual}, \{expected}")
  //
  println("Field default with function, both fields set")
  let input : Json = { "a": 100, "b": 200 }
  let before_calls = gen_func_calls.val
  let expected : Result[MyTypeOnlyADefault3, _] = Ok({ a: 100, b: 200 })
  let actual : Result[MyTypeOnlyADefault3, _] = try? @json.from_json(input)
  println("\{actual == expected}, \{actual}, \{expected}")
  let after_calls = gen_func_calls.val
  println(
    "\{before_calls == after_calls}, \{after_calls-before_calls}, should not call default function if value is provided",
  )
  //
  println("Field default with function, only b set")
  let input : Json = { "b": 200 }
  let before_calls = gen_func_calls.val
  let expected : Result[MyTypeOnlyADefault3, _] = Ok({ a: 65536, b: 200 })
  let actual : Result[MyTypeOnlyADefault3, _] = try? @json.from_json(input)
  println("\{actual == expected}, \{actual}, \{expected}")
  let after_calls = gen_func_calls.val
  println(
    "\{before_calls == after_calls - 1}, \{after_calls-before_calls}, should call default function if value is not provided",
  )
}

enum MyEnumDefault {
  A(Int)
} derive(FromJson(default, style="legacy"), Show, Eq)

// enum MyEnumDefault2 {
//   A(Int)
// } derive(FromJson(variants(
//     A(fields(
//       0(default=65536))
//       ))
//   ), Show, Eq)

fn init {
  println("Enum default, variant set")
  let input : Json = { "$tag": "A", "0": 100 }
  let expected : Result[MyEnumDefault, _] = Ok(MyEnumDefault::A(100))
  let actual : Result[MyEnumDefault, _] = try? @json.from_json(input)
  println("\{actual == expected}, \{actual}, \{expected}")
  //
  println("Enum default, variant default")
  let input : Json = { "$tag": "A" }
  let expected : Result[MyEnumDefault, _] = Ok(MyEnumDefault::A(0))
  let actual : Result[MyEnumDefault, _] = try? @json.from_json(input)
  println("\{actual == expected}, \{actual}, \{expected}")
  //
  // println("Enum default, variant default with value")
  // let input : Json = { "$tag": "A" }
  // let expected : Result[MyEnumDefault2, _] = Ok(MyEnumDefault2::A(65536))
  // let actual : Result[MyEnumDefault2, _] = try? @json.from_jsoninput)
  // println("\{actual == expected}, \{actual}, \{expected}")
}
