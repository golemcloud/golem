///|
test "pretty array" {
  let ls = [
    "aaaaaaaaaaaaaaaa", "bbbbbbbbbbbbbbbbbbbbbb", "1111111111111111222222", "3333333333333333",
    "4444",
  ]
  inspect(
    @prettyprinter.render(ls),
    content=(
      #|[
      #|  "aaaaaaaaaaaaaaaa",
      #|  "bbbbbbbbbbbbbbbbbbbbbb",
      #|  "1111111111111111222222",
      #|  "3333333333333333",
      #|  "4444"
      #|]
    ),
  )
}

///|
test "pretty map" {
  let map = {
    "1000": Some([1, 2, 3, 4, 5]),
    "200": Some([6, 7, 8, 9, 10]),
    "300": None,
    "410": None,
    "50": None,
  }
  let map = { "aaa": Some(map), "bbb": None, "cc": Some({ "0": None }) }
  inspect(
    @prettyprinter.render(map),
    content=(
      #|{
      #|  "aaa": Some({
      #|    "1000": Some([1, 2, 3, 4, 5]),
      #|    "200": Some([6, 7, 8, 9, 10]),
      #|    "300": None,
      #|    "410": None,
      #|    "50": None
      #|  }),
      #|  "bbb": None,
      #|  "cc": Some({"0": None})
      #|}
    ),
  )
}

///|
test "json" {
  let json : Json = {
    "key1": 1.0,
    "key2": 2099889.0,
    "key3": 11100991.0,
    "key4": 20000000.0,
    "obj": { "a": true, "b": true },
    "obj2": {
      "a": true,
      "b": true,
      "obj3": { "a": true, "b": true },
      "obj4": { "a": true, "b": true },
    },
    "key3": [true, false, Null],
  }
  inspect(
    @prettyprinter.render(json),
    content=(
      #|{
      #|  "key1": 1,
      #|  "key2": 2099889,
      #|  "key3": [true, false, null],
      #|  "key4": 20000000,
      #|  "obj": {"a": true, "b": true},
      #|  "obj2": {
      #|    "a": true,
      #|    "b": true,
      #|    "obj3": {"a": true, "b": true},
      #|    "obj4": {"a": true, "b": true}
      #|  }
      #|}
    ),
  )
}
