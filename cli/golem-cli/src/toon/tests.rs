// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Test suite for the TOON encoder.
//!
//! The base tests follow the TOON specification v3.3
//! (<https://github.com/toon-format/spec>), in particular Appendix A.
//! Since `serde_json::Map` preserves keys in sorted order, expected encoder
//! outputs use sorted key order; decoder tests use spec examples verbatim.
//!
//! The property tests use the strict decoder as a round-trip oracle and
//! generate values covering the whole `serde_json::Value` model (and through
//! `serde_json::to_value`, every serde-serializable shape).

use super::decode::decode;
use super::encode::{ToonEncodeError, encode, encode_value};
use indoc::indoc;
use proptest::prelude::*;
use serde_json::{Number, Value, json};
use test_r::test;

fn enc(value: &Value) -> String {
    encode_value(value).unwrap()
}

fn dec(input: &str) -> Value {
    decode(input).unwrap_or_else(|err| panic!("failed to decode {input:?}: {err}"))
}

/// Asserts that the value encodes to the expected document and that the
/// document decodes back to an equal value.
fn assert_encodes(value: Value, expected: &str) {
    let encoded = enc(&value);
    pretty_assertions::assert_eq!(encoded, expected);
    assert_roundtrip(&value);
}

fn assert_roundtrip(value: &Value) {
    let encoded = enc(value);
    let decoded =
        decode(&encoded).unwrap_or_else(|err| panic!("failed to decode {encoded:?}: {err}"));
    assert!(
        model_eq(&decoded, value),
        "round-trip mismatch:\nvalue:   {value:?}\nencoded: {encoded:?}\ndecoded: {decoded:?}"
    );
}

/// JSON-model equality per spec §2: numbers compare by mathematical value
/// (so `2.0` equals `2`), everything else compares structurally.
fn model_eq(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => match (int_value(x), int_value(y)) {
            (Some(x), Some(y)) => x == y,
            _ => x.as_f64() == y.as_f64(),
        },
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(a, b)| model_eq(a, b))
        }
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len()
                && x.iter()
                    .zip(y)
                    .all(|((ka, va), (kb, vb))| ka == kb && model_eq(va, vb))
        }
        _ => a == b,
    }
}

fn int_value(n: &Number) -> Option<i128> {
    n.as_i64()
        .map(i128::from)
        .or_else(|| n.as_u64().map(i128::from))
}

// --- Base tests: spec Appendix A examples ---------------------------------

#[test]
fn spec_objects() {
    assert_encodes(
        json!({"id": 123, "name": "Ada", "active": true}),
        indoc! {"
            active: true
            id: 123
            name: Ada"},
    );
}

#[test]
fn spec_nested_objects() {
    assert_encodes(
        json!({"user": {"id": 123, "name": "Ada"}}),
        indoc! {"
            user:
              id: 123
              name: Ada"},
    );
}

#[test]
fn spec_primitive_arrays() {
    assert_encodes(
        json!({"tags": ["admin", "ops", "dev"]}),
        "tags[3]: admin,ops,dev",
    );
}

#[test]
fn spec_arrays_of_arrays() {
    assert_encodes(
        json!({"pairs": [[1, 2], [3, 4]]}),
        indoc! {"
            pairs[2]:
              - [2]: 1,2
              - [2]: 3,4"},
    );
}

#[test]
fn spec_tabular_arrays() {
    assert_encodes(
        json!({"items": [
            {"sku": "A1", "qty": 2, "price": 9.99},
            {"sku": "B2", "qty": 1, "price": 14.5}
        ]}),
        indoc! {"
            items[2]{price,qty,sku}:
              9.99,2,A1
              14.5,1,B2"},
    );
}

#[test]
fn spec_mixed_arrays() {
    assert_encodes(
        json!({"items": [1, {"a": 1}, "text"]}),
        indoc! {"
            items[3]:
              - 1
              - a: 1
              - text"},
    );
}

#[test]
fn spec_objects_as_list_items() {
    assert_encodes(
        json!({"items": [
            {"id": 1, "name": "First"},
            {"id": 2, "name": "Second", "extra": true}
        ]}),
        indoc! {"
            items[2]:
              - id: 1
                name: First
              - extra: true
                id: 2
                name: Second"},
    );
}

#[test]
fn spec_tabular_array_as_first_field_of_list_item() {
    // §10: the tabular header goes on the hyphen line, rows at depth +2 and
    // the other fields at depth +1
    assert_encodes(
        json!({"items": [{
            "agents": [{"id": 1, "name": "Ada"}, {"id": 2, "name": "Bob"}],
            "status": "active"
        }]}),
        indoc! {"
            items[1]:
              - agents[2]{id,name}:
                  1,Ada
                  2,Bob
                status: active"},
    );
}

#[test]
fn spec_tabular_array_as_non_first_field_of_list_item() {
    assert_encodes(
        json!({"items": [{
            "status": "active",
            "users": [{"id": 1, "name": "Ada"}, {"id": 2, "name": "Bob"}]
        }]}),
        indoc! {"
            items[1]:
              - status: active
                users[2]{id,name}:
                  1,Ada
                  2,Bob"},
    );
}

#[test]
fn spec_quoted_colons_in_tabular_rows() {
    let value = json!({"links": [
        {"id": 1, "url": "http://a:b"},
        {"id": 2, "url": "https://example.com?q=a:b"}
    ]});
    let expected = indoc! {r#"
        links[2]{id,url}:
          1,"http://a:b"
          2,"https://example.com?q=a:b""#};
    assert_encodes(value.clone(), expected);
    assert_eq!(dec(expected), value);
}

#[test]
fn spec_edge_cases() {
    assert_encodes(json!({"name": ""}), r#"name: """#);
    assert_encodes(json!({"tags": []}), "tags: []");
    assert_encodes(
        json!({"version": "123", "enabled": "true"}),
        indoc! {r#"
            enabled: "true"
            version: "123""#},
    );
    assert_encodes(
        json!({"root": {"level1": {"level2": {"level3": {
            "items": [{"id": 1, "val": "a"}, {"id": 2, "val": "b"}]
        }}}}}),
        indoc! {"
            root:
              level1:
                level2:
                  level3:
                    items[2]{id,val}:
                      1,a
                      2,b"},
    );
    assert_encodes(
        json!({"message": "Hello 世界 👋", "tags": ["🎉", "🎊", "🎈"]}),
        indoc! {"
            message: Hello 世界 👋
            tags[3]: 🎉,🎊,🎈"},
    );
    assert_encodes(
        json!({"bignum": 9007199254740992u64, "decimal": 0.3333333333333333}),
        indoc! {"
            bignum: 9007199254740992
            decimal: 0.3333333333333333"},
    );
}

#[test]
fn spec_quoted_keys_with_arrays() {
    assert_encodes(json!({"my-key": [1, 2, 3]}), r#""my-key"[3]: 1,2,3"#);
    assert_encodes(
        json!({"x-items": [{"id": 1, "name": "Ada"}, {"id": 2, "name": "Bob"}]}),
        indoc! {r#"
            "x-items"[2]{id,name}:
              1,Ada
              2,Bob"#},
    );
    assert_encodes(
        json!({"x-items": [{"id": 1}, {"id": 2, "label": "archived"}]}),
        indoc! {r#"
            "x-items"[2]:
              - id: 1
              - id: 2
                label: archived"#},
    );
}

#[test]
fn spec_empty_objects_and_empty_object_list_items() {
    assert_encodes(json!({}), "");
    assert_encodes(json!({"empty": {}}), "empty:");
    assert_encodes(
        json!({"items": [{}, {}]}),
        indoc! {"
            items[2]:
              -
              -"},
    );
    // Arrays containing an empty object never use the tabular form
    assert_encodes(
        json!({"items": [{"a": 1}, {}]}),
        indoc! {"
            items[2]:
              - a: 1
              -"},
    );
}

#[test]
fn spec_root_forms() {
    assert_encodes(json!("hello"), "hello");
    assert_encodes(json!(42), "42");
    assert_encodes(json!(true), "true");
    assert_encodes(json!("true"), r#""true""#);
    assert_encodes(json!(""), r#""""#);
    assert_encodes(json!(null), "null");
    assert_encodes(json!([]), "[]");
    assert_encodes(json!([1, 2, 3]), "[3]: 1,2,3");
    assert_encodes(
        json!([{"id": 1, "name": "Ada"}, {"id": 2, "name": "Bob"}]),
        indoc! {"
            [2]{id,name}:
              1,Ada
              2,Bob"},
    );
    assert_encodes(
        json!([[1], {"a": 1}]),
        indoc! {"
            [2]:
              - [1]: 1
              - a: 1"},
    );
}

#[test]
fn spec_nested_arrays_as_list_items_use_expanded_form() {
    // §9.4: tabular form is not available for arrays in list item position
    assert_encodes(
        json!({"groups": [[{"id": 1}, {"id": 2}], [true]]}),
        indoc! {"
            groups[2]:
              - [2]:
                - id: 1
                - id: 2
              - [1]: true"},
    );
    assert_encodes(
        json!({"groups": [[], [[1, 2]]]}),
        indoc! {"
            groups[2]:
              - [0]:
              - [1]:
                - [2]: 1,2"},
    );
}

// --- Base tests: numbers (§2) ----------------------------------------------

#[test]
fn spec_number_canonical_form() {
    assert_encodes(json!(0), "0");
    assert_encodes(json!(-0.0), "0");
    assert_encodes(json!(1.0), "1");
    assert_encodes(json!(1.5), "1.5");
    assert_encodes(json!(-3.25), "-3.25");
    assert_encodes(json!(1e6), "1000000");
    assert_encodes(json!(1e-6), "0.000001");
    assert_encodes(json!(1e20), "100000000000000000000");
    assert_encodes(json!(i64::MIN), "-9223372036854775808");
    assert_encodes(json!(i64::MAX), "9223372036854775807");
    assert_encodes(json!(u64::MAX), "18446744073709551615");
}

#[test]
fn spec_number_exponent_form_outside_canonical_range() {
    assert_encodes(json!(1e-7), "1e-7");
    assert_encodes(json!(-2.5e-9), "-2.5e-9");
    assert_encodes(json!(1e21), "1e21");
    assert_encodes(json!(-1e21), "-1e21");
    assert_encodes(json!(1e308), "1e308");
    assert_encodes(json!(5e-324), "5e-324");
}

// --- Base tests: string quoting (§7.2) and keys (§7.3) ----------------------

#[test]
fn spec_string_value_quoting() {
    let quoted = [
        ("", r#""""#),
        (" a", r#"" a""#),
        ("a ", r#""a ""#),
        ("\u{a0}a", "\"\u{a0}a\""), // non-ASCII leading whitespace
        ("true", r#""true""#),
        ("false", r#""false""#),
        ("null", r#""null""#),
        ("42", r#""42""#),
        ("-3.14", r#""-3.14""#),
        ("05", r#""05""#),
        ("1e-6", r#""1e-6""#),
        ("1E+3", r#""1E+3""#),
        ("a:b", r#""a:b""#),
        (r#"a"b"#, r#""a\"b""#),
        (r"a\b", r#""a\\b""#),
        ("a[b", r#""a[b""#),
        ("a]b", r#""a]b""#),
        ("a{b", r#""a{b""#),
        ("a}b", r#""a}b""#),
        ("a,b", r#""a,b""#),
        ("a\nb", r#""a\nb""#),
        ("a\rb", r#""a\rb""#),
        ("a\tb", r#""a\tb""#),
        ("\u{7}", r#""\u0007""#),
        ("-", r#""-""#),
        ("- item", r#""- item""#),
        ("[]", r#""[]""#),
    ];
    for (value, expected) in quoted {
        assert_encodes(json!({ "v": value }), &format!("v: {expected}"));
    }

    let unquoted = [
        "hello",
        "hello world",
        "héllo 🦀",
        "a.b",
        "a-b",
        "0x5",
        "e10",
        "1.2.3",
        "x'y",
        "#comment",
    ];
    for value in unquoted {
        assert_encodes(json!({ "v": value }), &format!("v: {value}"));
    }
}

#[test]
fn spec_key_encoding() {
    assert_encodes(json!({"_under": 1}), "_under: 1");
    assert_encodes(json!({"a.b.c": 1}), "a.b.c: 1");
    assert_encodes(json!({"Key9": 1}), "Key9: 1");
    assert_encodes(json!({"true": 1}), "true: 1");
    assert_encodes(json!({"a b": 1}), r#""a b": 1"#);
    assert_encodes(json!({"": 1}), r#""": 1"#);
    assert_encodes(json!({"0k": 1}), r#""0k": 1"#);
    assert_encodes(json!({"a-b": 1}), r#""a-b": 1"#);
    assert_encodes(json!({"ключ": 1}), r#""ключ": 1"#);
    assert_encodes(json!({"a\nb": 1}), r#""a\nb": 1"#);
    assert_encodes(
        json!({"items": [{"a key": 1, "true": 2}, {"a key": 3, "true": 4}]}),
        indoc! {r#"
            items[2]{"a key",true}:
              1,2
              3,4"#},
    );
}

// --- Base tests: decoding spec forms and strictness -------------------------

#[test]
fn spec_decode_examples() {
    // §4 numeric decoding
    assert_eq!(dec("a: 1.5000"), json!({"a": 1.5}));
    assert_eq!(dec("a: -1E+03"), json!({"a": -1000.0}));
    assert_eq!(dec("a: -0"), json!({"a": 0}));
    // §4: forbidden leading zeros decode as strings
    assert_eq!(dec("version: 05"), json!({"version": "05"}));
    assert_eq!(dec("a: 0.5"), json!({"a": 0.5}));
    // Empty document and root forms
    assert_eq!(dec(""), json!({}));
    assert_eq!(dec("[]"), json!([]));
    assert_eq!(dec("hello"), json!("hello"));
    // Legacy empty array forms
    assert_eq!(dec("key[0]:"), json!({"key": []}));
    assert_eq!(dec("[0]:"), json!([]));
    // Inline arrays preserve empty tokens as empty strings
    assert_eq!(dec("a[3]: x,,y"), json!({"a": ["x", "", "y"]}));
    // Surrounding spaces around tokens are trimmed
    assert_eq!(dec("a[2]: x , y"), json!({"a": ["x", "y"]}));
}

#[test]
fn spec_decode_errors() {
    let invalid = [
        // A non key-value line in a multi-line document (a single `key value`
        // line decodes as a root primitive per §5)
        indoc! {"
            key value
            other: 1"},
        "name: \"bad\\xescape\"",
        "name: \"unterminated",
        "name: \"a\"b",
        "tags[5]: a,b,c",
        "tags[2]: a,b,c",
        indoc! {"
            items[3]{id,name}:
              1,Alice
              2,Bob"},
        indoc! {"
            items[1]{id,name}:
              1,Alice,extra"},
        indoc! {"
            items[1]:
               - value"},
        indoc! {"
            items[1]:
            \t- value"},
        indoc! {"
            a: 1

            b: 2"},
        indoc! {"
            a: 1
            a: 2"},
        "a: \"\\ud800\"", // lone surrogate
        "a:1",            // missing space after colon
        "a[x]: 1",        // invalid length
        "a[01]: 1",       // leading zero length
        "a[1]extra: 1",   // content between brackets and colon
    ];
    for input in invalid {
        assert!(
            decode(input).is_err(),
            "expected decode error for {input:?}, got {:?}",
            decode(input)
        );
    }
}

#[test]
fn encode_fails_on_too_deep_nesting() {
    let deep = (0..300).fold(json!(1), |acc, _| json!({ "a": acc }));
    assert_eq!(encode_value(&deep), Err(ToonEncodeError::MaxDepthExceeded));

    let deep_arrays = (0..300).fold(json!(1), |acc, _| json!([acc]));
    assert_eq!(
        encode_value(&deep_arrays),
        Err(ToonEncodeError::MaxDepthExceeded)
    );
}

#[test]
fn encode_accepts_any_serialize_value() {
    #[derive(serde::Serialize)]
    struct View {
        name: String,
        count: u32,
        tags: Vec<String>,
    }
    let view = View {
        name: "test".to_string(),
        count: 2,
        tags: vec!["a".to_string(), "b".to_string()],
    };
    assert_eq!(
        encode(&view).unwrap(),
        indoc! {"
            count: 2
            name: test
            tags[2]: a,b"}
    );
}

// --- Property tests ----------------------------------------------------------

const EDGE_STRINGS: &[&str] = &[
    "",
    " ",
    "  ",
    "a ",
    " a",
    "true",
    "false",
    "null",
    "-",
    "--",
    "- item",
    "42",
    "-3.14",
    "05",
    "0",
    "-0",
    "1e-6",
    "1E+3",
    "0.5e1",
    "a,b",
    "a:b",
    "a: b",
    "a\"b",
    "a\\b",
    "\\",
    "\"",
    "[3]: x",
    "{a}",
    "[]",
    "[0]:",
    "key:",
    "\n",
    "\r\n",
    "\t",
    "a\nb",
    "\u{7}",
    "\u{1f}",
    "\u{a0}",
    "héllo",
    "世界 👋",
    "🎉",
    "a.b",
    "0x5",
    "e10",
    "infinity",
    "NaN",
];

const EDGE_KEYS: &[&str] = &[
    "", " ", "a b", "0", "9k", "true", "null", "-", "a,b", "a:b", "a\"b", "a}b", "key", "a.b.c",
    "_x", "ключ", "🦀", "a\nb",
];

fn arb_string() -> impl Strategy<Value = String> {
    prop_oneof![
        any::<String>(),
        "[ -~]{0,8}",
        "-?0?[0-9]{1,4}(\\.[0-9]{0,3})?([eE][+-]?[0-9]{0,2})?",
        prop::sample::select(EDGE_STRINGS).prop_map(str::to_string),
    ]
}

fn arb_key() -> impl Strategy<Value = String> {
    prop_oneof![
        "[a-z_][a-zA-Z0-9_.]{0,6}",
        any::<String>(),
        prop::sample::select(EDGE_KEYS).prop_map(str::to_string),
    ]
}

fn arb_f64() -> impl Strategy<Value = f64> {
    prop_oneof![
        any::<f64>().prop_filter("finite", |f| f.is_finite()),
        prop::sample::select(vec![
            0.0,
            -0.0,
            1.0,
            -1.5,
            0.1,
            1.0 / 3.0,
            1e-7,
            -1e-7,
            1e-6,
            9.9e20,
            1e21,
            -1e21,
            1e308,
            5e-324,
            f64::MIN_POSITIVE,
            (1u64 << 60) as f64,
            9007199254740992.0,
            -9007199254740993.0,
        ]),
    ]
}

fn arb_number() -> impl Strategy<Value = Number> {
    prop_oneof![
        any::<i64>().prop_map(Number::from),
        any::<u64>().prop_map(Number::from),
        arb_f64().prop_map(|f| Number::from_f64(f).expect("finite floats are valid numbers")),
    ]
}

fn arb_primitive() -> impl Strategy<Value = Value> {
    prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        arb_number().prop_map(Value::Number),
        arb_string().prop_map(Value::String),
    ]
}

/// Arrays of uniform objects, biased towards the tabular form (§9.3).
fn arb_uniform_object_array() -> impl Strategy<Value = Value> {
    (
        prop::collection::btree_set(arb_key(), 1..4),
        prop::collection::vec(prop::collection::vec(arb_primitive(), 4), 1..5),
    )
        .prop_map(|(keys, rows)| {
            Value::Array(
                rows.into_iter()
                    .map(|row| Value::Object(keys.iter().cloned().zip(row).collect()))
                    .collect(),
            )
        })
}

/// Arbitrary JSON values covering the whole `serde_json::Value` model.
fn arb_value() -> impl Strategy<Value = Value> {
    arb_primitive().prop_recursive(6, 64, 5, |inner| {
        prop_oneof![
            3 => prop::collection::vec(inner.clone(), 0..5).prop_map(Value::Array),
            3 => prop::collection::vec((arb_key(), inner), 0..5)
                .prop_map(|fields| Value::Object(fields.into_iter().collect())),
            1 => arb_uniform_object_array(),
        ]
    })
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 512, .. ProptestConfig::default()
    })]

    #[test]
    fn proptest_roundtrip_arbitrary_values(value in arb_value()) {
        roundtrip_property(&value)?;
    }

    #[test]
    fn proptest_roundtrip_primitives(value in arb_primitive()) {
        roundtrip_property(&value)?;
    }

    #[test]
    fn proptest_roundtrip_primitive_arrays(values in prop::collection::vec(arb_primitive(), 0..8)) {
        roundtrip_property(&Value::Array(values))?;
    }

    #[test]
    fn proptest_roundtrip_arrays_of_arrays(
        values in prop::collection::vec(prop::collection::vec(arb_primitive(), 0..4), 0..4)
    ) {
        roundtrip_property(&Value::Array(values.into_iter().map(Value::Array).collect()))?;
    }

    #[test]
    fn proptest_roundtrip_uniform_object_arrays(value in arb_uniform_object_array()) {
        roundtrip_property(&value)?;
        roundtrip_property(&json!({ "items": [value] }))?;
    }

    #[test]
    fn proptest_roundtrip_objects(
        fields in prop::collection::vec((arb_key(), arb_primitive()), 0..6)
    ) {
        roundtrip_property(&Value::Object(fields.into_iter().collect()))?;
    }

    #[test]
    fn proptest_encoding_is_deterministic(value in arb_value()) {
        prop_assert_eq!(encode_value(&value).unwrap(), encode_value(&value).unwrap());
    }

    #[test]
    fn proptest_format_invariants(value in arb_value()) {
        let encoded = encode_value(&value).unwrap();
        prop_assert!(!encoded.ends_with('\n'), "trailing newline: {:?}", encoded);
        for line in encoded.split('\n') {
            prop_assert!(!line.ends_with(' '), "trailing whitespace: {:?}", line);
            let indent = line.len() - line.trim_start_matches(' ').len();
            prop_assert!(indent % 2 == 0, "odd indentation: {:?}", line);
            prop_assert!(!line[indent..].starts_with('\t'), "tab indentation: {:?}", line);
        }
    }
}

fn roundtrip_property(value: &Value) -> Result<(), TestCaseError> {
    let encoded = encode_value(value).unwrap();
    let decoded = match decode(&encoded) {
        Ok(decoded) => decoded,
        Err(err) => {
            return Err(TestCaseError::fail(format!(
                "failed to decode:\nvalue:   {value:?}\nencoded: {encoded:?}\nerror:   {err}"
            )));
        }
    };
    prop_assert!(
        model_eq(&decoded, value),
        "round-trip mismatch:\nvalue:   {:?}\nencoded: {:?}\ndecoded: {:?}",
        value,
        encoded,
        decoded
    );
    Ok(())
}
