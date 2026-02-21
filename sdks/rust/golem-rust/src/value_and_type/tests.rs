// Copyright 2024-2025 Golem Cloud
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

use proptest::prop_assert_eq;
use proptest::proptest;
use proptest::strategy::Strategy;
use std::collections::{BTreeMap, BTreeSet, HashSet, LinkedList, VecDeque};
use std::num::{
    NonZeroI16, NonZeroI32, NonZeroI64, NonZeroI8, NonZeroU16, NonZeroU32, NonZeroU64, NonZeroU8,
};
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;
use test_r::test;

use crate::value_and_type::{FromValueAndType, IntoValue};
use golem_rust_macro::{FromValueAndType, IntoValue};
use golem_wasm::golem_rpc_0_2_x::types::ValueAndType;
use golem_wasm::{Value, WitValue};

#[derive(IntoValue, FromValueAndType, PartialEq, Debug, Clone)]
enum MyEnum {
    Simple,
    Complex1(i32),
    Complex2(i32, String),
    Complex3 { x: String, y: bool },
}

#[test]
fn test_into_value_derivation_enum() {
    let simple_value = MyEnum::Simple.into_value();

    let complex1_value = MyEnum::Complex1(42).into_value();

    let complex2_value = MyEnum::Complex2(7, "hello".to_string()).into_value();

    let complex3_value = MyEnum::Complex3 {
        x: "world".to_string(),
        y: true,
    }
    .into_value();

    let expected_simple = Value::Variant {
        case_idx: 0,
        case_value: None,
    };

    let expected_complex1 = Value::Variant {
        case_idx: 1,
        case_value: Some(Box::new(Value::S32(42))),
    };

    let expected_complex2 = Value::Variant {
        case_idx: 2,
        case_value: Some(Box::new(Value::Tuple(vec![
            Value::S32(7),
            Value::String("hello".to_string()),
        ]))),
    };

    let expected_complex3 = Value::Variant {
        case_idx: 3,
        case_value: Some(Box::new(Value::Record(vec![
            Value::String("world".to_string()),
            Value::Bool(true),
        ]))),
    };

    assert_eq!(simple_value, WitValue::from(expected_simple));
    assert_eq!(complex1_value, WitValue::from(expected_complex1));
    assert_eq!(complex2_value, WitValue::from(expected_complex2));
    assert_eq!(complex3_value, WitValue::from(expected_complex3));
}

#[test]
fn test_from_value_derivation_enum() {
    let enum_type = MyEnum::get_type();

    let simple1_value = WitValue::from(Value::Variant {
        case_idx: 0,
        case_value: None,
    });

    let simple1_value_and_type = ValueAndType {
        value: simple1_value,
        typ: enum_type.clone(),
    };

    let complex1_value = WitValue::from(Value::Variant {
        case_idx: 1,
        case_value: Some(Box::new(Value::S32(42))),
    });

    let complex1_value_and_type = ValueAndType {
        value: complex1_value,
        typ: enum_type.clone(),
    };

    let complex2_value = WitValue::from(Value::Variant {
        case_idx: 2,
        case_value: Some(Box::new(Value::Tuple(vec![
            Value::S32(7),
            Value::String("hello".to_string()),
        ]))),
    });

    let complex2_value_and_type = ValueAndType {
        value: complex2_value,
        typ: enum_type.clone(),
    };

    let complex3_value = WitValue::from(Value::Variant {
        case_idx: 3,
        case_value: Some(Box::new(Value::Record(vec![
            Value::String("world".to_string()),
            Value::Bool(true),
        ]))),
    });

    let complex3_value_and_type = ValueAndType {
        value: complex3_value,
        typ: enum_type.clone(),
    };

    let expected_simple = MyEnum::Simple;

    let expected_complex1 = MyEnum::Complex1(42);

    let expected_complex2 = MyEnum::Complex2(7, "hello".to_string());

    let expected_complex3 = MyEnum::Complex3 {
        x: "world".to_string(),
        y: true,
    };

    assert_eq!(
        MyEnum::from_value_and_type(simple1_value_and_type).unwrap(),
        expected_simple
    );
    assert_eq!(
        MyEnum::from_value_and_type(complex1_value_and_type).unwrap(),
        expected_complex1
    );
    assert_eq!(
        MyEnum::from_value_and_type(complex2_value_and_type).unwrap(),
        expected_complex2
    );
    assert_eq!(
        MyEnum::from_value_and_type(complex3_value_and_type).unwrap(),
        expected_complex3
    );
}

#[test]
fn test_round_trip_enum_derivation() {
    let simple = MyEnum::Simple;
    let complex1 = MyEnum::Complex1(42);
    let complex2 = MyEnum::Complex2(7, "hello".to_string());
    let complex3 = MyEnum::Complex3 {
        x: "world".to_string(),
        y: true,
    };

    let typ = MyEnum::get_type();

    let simple_value = ValueAndType {
        value: simple.clone().into_value(),
        typ: typ.clone(),
    };

    let complex1_value = ValueAndType {
        value: complex1.clone().into_value(),
        typ: typ.clone(),
    };

    let complex2_value = ValueAndType {
        value: complex2.clone().into_value(),
        typ: typ.clone(),
    };

    let complex3_value = ValueAndType {
        value: complex3.clone().into_value(),
        typ: typ.clone(),
    };

    assert_eq!(MyEnum::from_value_and_type(simple_value).unwrap(), simple);
    assert_eq!(
        MyEnum::from_value_and_type(complex1_value).unwrap(),
        complex1
    );
    assert_eq!(
        MyEnum::from_value_and_type(complex2_value).unwrap(),
        complex2
    );
    assert_eq!(
        MyEnum::from_value_and_type(complex3_value).unwrap(),
        complex3
    );
}

// Macros are now provided by test_macros module - imported via macro_use in lib.rs
use crate::{roundtrip_test, roundtrip_test_deref, roundtrip_test_map};

// Primitive types
roundtrip_test!(prop_roundtrip_u8, u8, 0u8..);
roundtrip_test!(prop_roundtrip_u16, u16, 0u16..);
roundtrip_test!(prop_roundtrip_u32, u32, 0u32..);
roundtrip_test!(prop_roundtrip_u64, u64, 0u64..);
roundtrip_test!(prop_roundtrip_i8, i8, i8::MIN..=i8::MAX);
roundtrip_test!(prop_roundtrip_i16, i16, i16::MIN..=i16::MAX);
roundtrip_test!(prop_roundtrip_i32, i32, i32::MIN..=i32::MAX);
roundtrip_test!(prop_roundtrip_i64, i64, i64::MIN..=i64::MAX);
roundtrip_test!(prop_roundtrip_f32, f32, -1e6f32..=1e6f32);
roundtrip_test!(prop_roundtrip_f64, f64, -1e15f64..=1e15f64);
roundtrip_test!(prop_roundtrip_bool, bool, proptest::bool::ANY);
roundtrip_test!(prop_roundtrip_char, char, proptest::char::any());
roundtrip_test!(prop_roundtrip_string, String, ".*");

// Option types
roundtrip_test!(
    prop_roundtrip_option_u32,
    Option<u32>,
    proptest::option::of(0u32..)
);
roundtrip_test!(
    prop_roundtrip_option_string,
    Option<String>,
    proptest::option::of(".*")
);

// Result types - use manual proptest! blocks since result strategies are tricky
#[test]
fn prop_roundtrip_result_u32_string() {
    proptest!(|(ok_val in 0u32.., err_val in ".*", is_ok in proptest::bool::ANY)| {
        let result: Result<u32, String> = if is_ok { Ok(ok_val) } else { Err(err_val.clone()) };
        let typ = Result::<u32, String>::get_type();
        let value_and_type = ValueAndType {
            value: result.clone().into_value(),
            typ,
        };
        let recovered = Result::<u32, String>::from_value_and_type(value_and_type)
            .expect("roundtrip conversion should succeed");
        prop_assert_eq!(recovered, result);
    });
}

#[test]
fn prop_roundtrip_result_string_u32() {
    proptest!(|(ok_val in ".*", err_val in 0u32.., is_ok in proptest::bool::ANY)| {
        let result: Result<String, u32> = if is_ok { Ok(ok_val.clone()) } else { Err(err_val) };
        let typ = Result::<String, u32>::get_type();
        let value_and_type = ValueAndType {
            value: result.clone().into_value(),
            typ,
        };
        let recovered = Result::<String, u32>::from_value_and_type(value_and_type)
            .expect("roundtrip conversion should succeed");
        prop_assert_eq!(recovered, result);
    });
}

// NonZero types (with mapped strategies)
roundtrip_test_map!(
    prop_roundtrip_nonzero_u8,
    NonZeroU8,
    1u8..=u8::MAX,
    |value| NonZeroU8::new(value).unwrap()
);
roundtrip_test_map!(
    prop_roundtrip_nonzero_u16,
    NonZeroU16,
    1u16..=u16::MAX,
    |value| NonZeroU16::new(value).unwrap()
);
roundtrip_test_map!(
    prop_roundtrip_nonzero_u32,
    NonZeroU32,
    1u32..=u32::MAX,
    |value| NonZeroU32::new(value).unwrap()
);
roundtrip_test_map!(
    prop_roundtrip_nonzero_u64,
    NonZeroU64,
    1u64..=u64::MAX,
    |value| NonZeroU64::new(value).unwrap()
);
roundtrip_test_map!(
    prop_roundtrip_nonzero_i8,
    NonZeroI8,
    (1i8..=i8::MAX).prop_union(i8::MIN..=-1i8),
    |value| NonZeroI8::new(value).unwrap()
);
roundtrip_test_map!(
    prop_roundtrip_nonzero_i16,
    NonZeroI16,
    (1i16..=i16::MAX).prop_union(i16::MIN..=-1i16),
    |value| NonZeroI16::new(value).unwrap()
);
roundtrip_test_map!(
    prop_roundtrip_nonzero_i32,
    NonZeroI32,
    (1i32..=i32::MAX).prop_union(i32::MIN..=-1i32),
    |value| NonZeroI32::new(value).unwrap()
);
roundtrip_test_map!(
    prop_roundtrip_nonzero_i64,
    NonZeroI64,
    (1i64..=i64::MAX).prop_union(i64::MIN..=-1i64),
    |value| NonZeroI64::new(value).unwrap()
);

#[test]
fn prop_roundtrip_duration() {
    proptest!(|(secs in 0u64..=1_000_000, nanos in 0u32..1_000_000_000)| {
        let duration = Duration::new(secs, nanos);
        let typ = Duration::get_type();
        let value_and_type = ValueAndType {
            value: duration.into_value(),
            typ,
        };
        let recovered = Duration::from_value_and_type(value_and_type)
            .expect("roundtrip conversion should succeed");
        prop_assert_eq!(recovered, duration);
    });
}

#[test]
fn prop_roundtrip_range_u32() {
    proptest!(|(start in 0u32.., end in 0u32..)| {
        let range = Range { start, end };
        let typ = Range::<u32>::get_type();
        let value_and_type = ValueAndType {
            value: range.clone().into_value(),
            typ,
        };
        let recovered = Range::<u32>::from_value_and_type(value_and_type)
            .expect("roundtrip conversion should succeed");
        prop_assert_eq!(recovered, range);
    });
}

// Collection types
roundtrip_test!(
    prop_roundtrip_vec_u32,
    Vec<u32>,
    proptest::collection::vec(0u32.., 0..100)
);
roundtrip_test!(
    prop_roundtrip_vec_string,
    Vec<String>,
    proptest::collection::vec(".*", 0..50)
);
roundtrip_test!(
    prop_roundtrip_hashset_u32,
    HashSet<u32>,
    proptest::collection::hash_set(0u32.., 0..100)
);
roundtrip_test!(prop_roundtrip_btreemap_u32_string, BTreeMap<u32, String>, proptest::collection::btree_map(0u32.., ".*", 0..50));
roundtrip_test!(
    prop_roundtrip_btreeset_u32,
    BTreeSet<u32>,
    proptest::collection::btree_set(0u32.., 0..100)
);
roundtrip_test_map!(
    prop_roundtrip_vecdeque_u32,
    VecDeque<u32>,
    proptest::collection::vec(0u32.., 0..100),
    |vec| VecDeque::from(vec)
);
roundtrip_test_map!(
    prop_roundtrip_linkedlist_u32,
    LinkedList<u32>,
    proptest::collection::vec(0u32.., 0..100),
    |vec| LinkedList::from_iter(vec)
);

roundtrip_test_deref!(prop_roundtrip_rc_u32, Rc<u32>, 0u32.., |value| Rc::new(
    value
));
roundtrip_test_deref!(prop_roundtrip_arc_u32, Arc<u32>, 0u32.., |value| Arc::new(
    value
));
