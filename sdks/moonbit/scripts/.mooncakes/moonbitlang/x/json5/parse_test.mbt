// Copyright 2025 International Digital Economy Academy
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

///|
#callsite(autofill(loc))
fn test_parse(input : String, loc~ : SourceLoc) -> Json raise Error {
  let v = try? @json5.parse(input)
  match v {
    Ok(v) => v
    Err(err) => fail("Parse failed, \{loc}, \{err}")
  }
}

//region objects

///|
test "parses empty objects" {
  let json = test_parse("{}")
  assert_eq(json, Json::object(Map::of([])))
}

///|
test "parses double string property names" {
  let json = test_parse("{\"a\":1}")
  assert_eq(json, Json::object(Map::of([("a", Json::number(1.0))])))
}

///|
test "parses single string property names" {
  let json = test_parse("{a:1}")
  assert_eq(json, Json::object(Map::of([("a", Json::number(1.0))])))
}

///|
test "parses unquoted property names" {
  let json = test_parse("{a:1}")
  assert_eq(json, Json::object(Map::of([("a", Json::number(1.0))])))
}

///|
test "parses special character property names" {
  let json = test_parse("{$_:1,_$:2,a\u200C:3}")
  assert_eq(
    json,
    Json::object(
      Map::of([
        ("$_", Json::number(1.0)),
        ("_$", Json::number(2.0)),
        ("a\u200C", Json::number(3.0)),
      ]),
    ),
  )
}

///|
test "parses unicode property names" {
  let json = test_parse("{ùńîċõďë:9}")
  assert_eq(
    json,
    Json::object(Map::of([("ùńîċõďë", Json::number(9.0))])),
  )
}

///|
test "parses escaped property names" {
  let json = test_parse("{\\u0061\\u0062:1,\\u0024\\u005F:2,\\u005F\\u0024:3}")
  assert_eq(
    json,
    Json::object(
      Map::of([
        ("ab", Json::number(1.0)),
        ("$_", Json::number(2.0)),
        ("_$", Json::number(3.0)),
      ]),
    ),
  )
}

///|
test "preserves __proto__ property names" {
  let json = test_parse("{\"__proto__\":1}")
  assert_eq(json, Json::object(Map::of([("__proto__", Json::number(1.0))])))
}

///|
test "parses multiple properties" {
  let json = test_parse("{abc:1,def:2}")
  assert_eq(
    json,
    Json::object(
      Map::of([("abc", Json::number(1.0)), ("def", Json::number(2.0))]),
    ),
  )
}

///|
test "parses nested objects" {
  let json = test_parse("{a:{b:2}}")
  assert_eq(
    json,
    Json::object(
      Map::of([("a", Json::object(Map::of([("b", Json::number(2.0))])))]),
    ),
  )
}
//endregion

//region arrays

///|
test "parses empty arrays" {
  let json = test_parse("[]")
  assert_eq(json, Json::array([]))
}

///|
test "parses array values" {
  let json = test_parse("[1]")
  assert_eq(json, Json::array([Json::number(1.0)]))
}

///|
test "parses multiple array values" {
  let json = test_parse("[1,2]")
  assert_eq(json, Json::array([Json::number(1.0), Json::number(2.0)]))
}

///|
test "parses nested arrays" {
  let json = test_parse("[1,[2,3]]")
  assert_eq(
    json,
    Json::array([
      Json::number(1.0),
      Json::array([Json::number(2.0), Json::number(3.0)]),
    ]),
  )
}
//endregion

//region nulls

///|
test "parses nulls" {
  let json = test_parse("null")
  assert_eq(json, Json::null())
}
//endregion

//region booleans

///|
test "parses true" {
  let json = test_parse("true")
  assert_eq(json, Json::boolean(true))
}

///|
test "parses false" {
  let json = test_parse("false")
  assert_eq(json, Json::boolean(false))
}
//endregion

//region numbers

///|
test "parses leading zeroes" {
  let json = test_parse("[0,0.,0e0]")
  assert_eq(
    json,
    Json::array([Json::number(0.0), Json::number(0.0), Json::number(0.0)]),
  )
}

///|
test "parses integers" {
  let json = test_parse("[1,23,456,7890]")
  assert_eq(
    json,
    Json::array([
      Json::number(1.0),
      Json::number(23.0),
      Json::number(456.0),
      Json::number(7890.0),
    ]),
  )
}

///|
test "parses signed numbers" {
  let json = test_parse("[-1,+2,-.1,-0]")
  assert_eq(
    json,
    Json::array([
      Json::number(-1.0),
      Json::number(2.0),
      Json::number(-0.1),
      Json::number(-0.0),
    ]),
  )
}

///|
test "parses leading decimal points" {
  let json = test_parse("[.1,.23]")
  assert_eq(json, Json::array([Json::number(0.1), Json::number(0.23)]))
}

///|
test "parses fractional numbers" {
  let json = test_parse("[1.0,1.23]")
  assert_eq(json, Json::array([Json::number(1.0), Json::number(1.23)]))
}

///|
test "parses exponents" {
  let json = test_parse("[1e0,1e1,1e01,1.e0,1.1e0,1e-1,1e+1]")
  assert_eq(
    json,
    Json::array([
      Json::number(1.0),
      Json::number(10.0),
      Json::number(10.0),
      Json::number(1.0),
      Json::number(1.1),
      Json::number(0.1),
      Json::number(10.0),
    ]),
  )
}

///|
test "parses hexadecimal numbers" {
  let json = test_parse("[0x1,0x10,0xff,0xFF]")
  assert_eq(
    json,
    Json::array([
      Json::number(1.0),
      Json::number(16.0),
      Json::number(255.0),
      Json::number(255.0),
    ]),
  )
}

///|
test "parses signed and unsigned Infinity" {
  let json = test_parse("[Infinity,-Infinity]")
  assert_eq(
    json,
    Json::array([
      Json::number(@double.infinity),
      Json::number(@double.neg_infinity),
    ]),
  )
}

///|
test "parses NaN" {
  let json = test_parse("NaN")
  let ok = match json {
    Json::Number(n, ..) => n.is_nan()
    _ => false
  }
  assert_true(ok)
}

///|
test "parses signed NaN" {
  let json = test_parse("-NaN")
  let ok = match json {
    Json::Number(n, ..) => n.is_nan()
    _ => false
  }
  assert_true(ok)
}

///|
test "parses 1" {
  let json = test_parse("1")
  assert_eq(json, Json::number(1.0))
}

///|
test "parses +1.23e100" {
  let json = test_parse("+1.23e100")
  assert_eq(json, Json::number(1.23e100))
}

///|
test "parses bare hexadecimal number" {
  let json = test_parse("0x1")
  assert_eq(json, Json::number(1.0))
}

///|
test "parses bare long hexadecimal number" {
  let json = test_parse("-0x0123456789abcdefABCDEF")
  assert_eq(json, Json::number(-0x0123456789abcdefABCDEF.0))
}
//endregion

//region strings

///|
test "parses double quoted strings" {
  let json = test_parse("\"abc\"")
  assert_eq(json, Json::string("abc"))
}

///|
test "parses single quoted strings" {
  let json = test_parse("'abc'")
  assert_eq(json, Json::string("abc"))
}

///|
test "parses quotes in strings" {
  let json = test_parse("[\"\\\"\",\"'\"]")
  assert_eq(json, Json::array([Json::string("\""), Json::string("'")]))
}

///|
test "parses escaped characters" {
  let json = test_parse("'\\b\\f\\n\\r\\t\\v\\0\\x0f\\u01fF\\a\\'\\\"'")
  assert_eq(
    json,
    Json::string("\u0008\u000C\n\r\t\u000B\u0000\u000F\u01FFa'\""),
  )
}
// test "parses line and paragraph separators with a warning" {
//   let json = test_parse("'\u2028\u2029'")!
//   assert_eq(json, Json::string("\u2028\u2029"))!
// }
//endregion

//region comments

///|
test "parses single-line comments" {
  let json = test_parse("{//comment\n}")
  assert_eq(json, Json::object(Map::of([])))
}

///|
test "parses single-line comments at end of input" {
  let json = test_parse("{}//comment")
  assert_eq(json, Json::object(Map::of([])))
}

///|
test "parses multi-line comments" {
  let json = test_parse("{/*comment\n** */}")
  assert_eq(json, Json::object(Map::of([])))
}
//endregion

//region whitespace

///|
test "parses whitespace" {
  let json = test_parse("{\t\u000b\u000c \u00A0\uFEFF\n\r\u2028\u2029\u2003}")
  assert_eq(json, Json::object(Map::of([])))
}
//endregion
