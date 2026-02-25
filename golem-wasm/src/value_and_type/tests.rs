// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use super::*;
use bigdecimal::BigDecimal;
use bit_vec::BitVec;
use chrono::{DateTime, NaiveDate, NaiveDateTime, NaiveTime, Utc};
use golem_wasm_derive::{FromValue, IntoValue};
use proptest::prelude::*;
use proptest_arbitrary_interop::arb_sized;
use std::collections::Bound;
use std::str::FromStr;
use url::Url;

use test_r::test;

// For the derivation macros
mod golem_wasm {
    pub use crate::*;
}

#[derive(Debug, Clone, PartialEq, IntoValue, FromValue)]
struct TestStruct {
    a: u32,
    b: String,
}

#[derive(Debug, Clone, PartialEq, IntoValue, FromValue)]
struct TestNewtype(u64);

#[derive(Debug, Clone, PartialEq, IntoValue, FromValue)]
struct TestTuple(u32, String);

#[derive(Debug, Clone, PartialEq, IntoValue, FromValue)]
enum TestEnum {
    A,
    B,
    C,
}

#[derive(Debug, Clone, PartialEq, IntoValue, FromValue)]
enum TestVariant {
    Unit,
    Single(u32),
    Named { x: String, y: i32 },
    Tuple(u32, String),
}

#[derive(Debug, Clone, PartialEq, IntoValue, FromValue)]
#[wit_transparent]
struct TransparentNewtype(String);

#[derive(Debug, Clone, PartialEq, IntoValue, FromValue)]
#[wit_transparent]
struct TransparentNewtype2 {
    value: String,
}

#[derive(Debug, Clone, PartialEq, IntoValue, FromValue)]
struct TestWithRenamedField {
    #[wit_field(rename = "custom_name")]
    field: u32,
}

#[derive(Debug, Clone, PartialEq)]
struct WrappedU32(u32);

#[allow(clippy::from_over_into)]
impl Into<u32> for WrappedU32 {
    fn into(self) -> u32 {
        self.0
    }
}

#[allow(clippy::from_over_into)]
impl Into<WrappedU32> for u32 {
    fn into(self) -> WrappedU32 {
        WrappedU32(self)
    }
}

#[derive(Debug, Clone, PartialEq, IntoValue, FromValue)]
struct TestWithConvertedFieldAndSkip {
    #[wit_field(convert = u32)]
    field: WrappedU32,
    #[wit_field(skip)]
    other: bool,
}

// Custom strategies for chrono types to ensure valid dates/times
fn arb_naive_date() -> impl Strategy<Value = NaiveDate> {
    (1900..=2100i32, 1..=12u32, 1..=28u32)
        .prop_map(|(y, m, d)| NaiveDate::from_ymd_opt(y, m, d).unwrap())
}

fn arb_naive_time() -> impl Strategy<Value = NaiveTime> {
    (0..=23u32, 0..=59u32, 0..=59u32, 0..=999999999u32)
        .prop_map(|(h, m, s, n)| NaiveTime::from_hms_nano_opt(h, m, s, n).unwrap())
}

fn arb_naive_datetime() -> impl Strategy<Value = NaiveDateTime> {
    (arb_naive_date(), arb_naive_time()).prop_map(|(d, t)| NaiveDateTime::new(d, t))
}

fn arb_datetime_utc() -> impl Strategy<Value = DateTime<Utc>> {
    (arb_naive_datetime(), -12..=12i32)
        .prop_map(|(dt, _offset)| DateTime::from_naive_utc_and_offset(dt, Utc))
}

const CASES: u32 = 10000;
const SIZE: usize = 4096;

proptest! {
    #![proptest_config(ProptestConfig {
        cases: CASES, .. ProptestConfig::default()
    })]

    #[test]
    fn u8_roundtrip(value in any::<u8>()) {
        let val = value.into_value();
        let back = u8::from_value(val).unwrap();
        prop_assert_eq!(back, value);
    }

    #[test]
    fn u16_roundtrip(value in any::<u16>()) {
        let val = value.into_value();
        let back = u16::from_value(val).unwrap();
        prop_assert_eq!(back, value);
    }

    #[test]
    fn u32_roundtrip(value in any::<u32>()) {
        let val = value.into_value();
        let back = u32::from_value(val).unwrap();
        prop_assert_eq!(back, value);
    }

    #[test]
    fn u64_roundtrip(value in any::<u64>()) {
        let val = value.into_value();
        let back = u64::from_value(val).unwrap();
        prop_assert_eq!(back, value);
    }

    #[test]
    fn i8_roundtrip(value in any::<i8>()) {
        let val = value.into_value();
        let back = i8::from_value(val).unwrap();
        prop_assert_eq!(back, value);
    }

    #[test]
    fn i16_roundtrip(value in any::<i16>()) {
        let val = value.into_value();
        let back = i16::from_value(val).unwrap();
        prop_assert_eq!(back, value);
    }

    #[test]
    fn i32_roundtrip(value in any::<i32>()) {
        let val = value.into_value();
        let back = i32::from_value(val).unwrap();
        prop_assert_eq!(back, value);
    }

    #[test]
    fn i64_roundtrip(value in any::<i64>()) {
        let val = value.into_value();
        let back = i64::from_value(val).unwrap();
        prop_assert_eq!(back, value);
    }

    #[test]
    fn f32_roundtrip(value in any::<f32>().prop_filter("not nan", |x| !x.is_nan())) {
        let val = value.into_value();
        let back = f32::from_value(val).unwrap();
        prop_assert_eq!(back, value);
    }

    #[test]
    fn f64_roundtrip(value in any::<f64>().prop_filter("not nan", |x| !x.is_nan())) {
        let val = value.into_value();
        let back = f64::from_value(val).unwrap();
        prop_assert_eq!(back, value);
    }

    #[test]
    fn bool_roundtrip(value in any::<bool>()) {
        let val = value.into_value();
        let back = bool::from_value(val).unwrap();
        prop_assert_eq!(back, value);
    }

    #[test]
    fn char_roundtrip(value in any::<char>()) {
        let val = value.into_value();
        let back = char::from_value(val).unwrap();
        prop_assert_eq!(back, value);
    }

    #[test]
    fn string_roundtrip(value in any::<String>()) {
            let original = value.clone();
            let val = value.into_value();
            let back = String::from_value(val).unwrap();
        prop_assert_eq!(back, original);
    }

    #[test]
    fn option_u32_roundtrip(value in proptest::option::of(any::<u32>())) {
        let val = value.into_value();
        let back = Option::<u32>::from_value(val).unwrap();
        prop_assert_eq!(back, value);
    }

    #[test]
    fn vec_string_roundtrip(value in proptest::collection::vec(any::<String>(), 0..10)) {
            let original = value.clone();
            let val = value.into_value();
            let back = Vec::<String>::from_value(val).unwrap();
        prop_assert_eq!(back, original);
    }

    #[test]
    fn result_u32_string_roundtrip(value in proptest::result::maybe_ok(any::<u32>(), any::<String>())) {
            let original = value.clone();
            let val = value.into_value();
            let back = Result::<u32, String>::from_value(val).unwrap();
        prop_assert_eq!(back, original);
    }

    #[test]
    fn tuple_u32_string_roundtrip(value in (any::<u32>(), any::<String>())) {
            let original = value.clone();
            let val = value.into_value();
            let back = <(u32, String)>::from_value(val).unwrap();
        prop_assert_eq!(back, original);
    }

    #[test]
    fn bound_u32_roundtrip(value in proptest::sample::select(vec![Bound::Included(0u32), Bound::Excluded(1), Bound::Unbounded])) {
            let val = value.into_value();
            let back = Bound::<u32>::from_value(val).unwrap();
        prop_assert_eq!(back, value);
    }

    #[test]
    fn bigdecimal_roundtrip(value in any::<String>().prop_map(|s| BigDecimal::from_str(&s).unwrap_or(BigDecimal::from(0)))) {
            let original = value.clone();
            let val = value.into_value();
            let back = BigDecimal::from_value(val).unwrap();
        prop_assert_eq!(back, original);
    }

    #[test]
    fn naive_date_roundtrip(value in arb_naive_date()) {
        let val = value.into_value();
        let back = NaiveDate::from_value(val).unwrap();
        prop_assert_eq!(back, value);
    }

    #[test]
    fn naive_time_roundtrip(value in arb_naive_time()) {
        let val = value.into_value();
        let back = NaiveTime::from_value(val).unwrap();
        prop_assert_eq!(back, value);
    }

    #[test]
    fn naive_datetime_roundtrip(value in arb_naive_datetime()) {
        let val = value.into_value();
        let back = NaiveDateTime::from_value(val).unwrap();
        prop_assert_eq!(back, value);
    }

    #[test]
    fn datetime_utc_roundtrip(value in arb_datetime_utc()) {
        let val = value.into_value();
        let back = DateTime::<Utc>::from_value(val).unwrap();
        prop_assert_eq!(back, value);
    }

    #[test]
    fn bitvec_roundtrip(value in proptest::collection::vec(any::<bool>(), 0..100).prop_map(BitVec::from_iter)) {
            let original = value.clone();
            let val = value.into_value();
            let back = BitVec::from_value(val).unwrap();
        prop_assert_eq!(back, original);
    }

    #[test]
    fn url_roundtrip(value in proptest::sample::select(vec![
        url::Url::parse("https://example.com").unwrap(),
        url::Url::parse("http://localhost:8080/path").unwrap(),
        url::Url::parse("https://test.com?query=value").unwrap(),
    ])) {
        let original = value.clone();
            let val = value.into_value();
            let back = Url::from_value(val).unwrap();
            prop_assert_eq!(back, original);
        }

        #[test]
        fn uri_roundtrip(value in any::<String>().prop_map(|s| crate::Uri { value: s })) {
            let original = value.clone();
            let val = value.into_value();
            let back = crate::Uri::from_value(val).unwrap();
            prop_assert_eq!(back, original);
        }

    #[test]
    fn duration_roundtrip(value in any::<u64>().prop_map(std::time::Duration::from_nanos)) {
        let val = value.into_value();
        let back = std::time::Duration::from_value(val).unwrap();
        prop_assert_eq!(back, value);
    }

    #[test]
    fn resource_mode_roundtrip(value in proptest::sample::select(vec![crate::ResourceMode::Owned, crate::ResourceMode::Borrowed])) {
        let original = value;
        let val = value.into_value();
        let back = crate::ResourceMode::from_value(val).unwrap();
        prop_assert_eq!(back, original);
    }

    #[test]
    fn box_u32_roundtrip(value in any::<u32>()) {
        let boxed = Box::new(value);
        let original = boxed.clone();
        let val = boxed.into_value();
        let back = Box::<u32>::from_value(val).unwrap();
        prop_assert_eq!(back, original);
    }

    #[test]
    fn hashmap_string_u32_roundtrip(value in proptest::collection::hash_map(any::<String>(), any::<u32>(), 0..10)) {
        let original = value.clone();
        let val = value.into_value();
        let back = std::collections::HashMap::<String, u32>::from_value(val).unwrap();
        prop_assert_eq!(back, original);
    }

    #[test]
    fn btreemap_u32_string_roundtrip(value in proptest::collection::btree_map(any::<u32>(), any::<String>(), 0..10)) {
        let original = value.clone();
        let val = value.into_value();
        let back = std::collections::BTreeMap::<u32, String>::from_value(val).unwrap();
        prop_assert_eq!(back, original);
    }

    #[test]
    fn btreeset_u32_roundtrip(value in proptest::collection::btree_set(any::<u32>(), 0..10)) {
        let original = value.clone();
        let val = value.into_value();
        let back = std::collections::BTreeSet::<u32>::from_value(val).unwrap();
        prop_assert_eq!(back, original);
    }

    #[test]
    fn value_round_trip(value in arb_sized::<Value>(SIZE).prop_filter("Value must be equal to itself", |v| v.eq(v))) {
        let val = value.clone().into_value();
        let back: Value = Value::from_value(val).unwrap();
        prop_assert_eq!(back, value);
    }
}

#[test]
fn value_roundtrip() {
    let value = crate::Value::U32(42);
    let val = value.clone().into_value();
    let back = crate::Value::from_value(val).unwrap();
    assert_eq!(back, value);
}

#[test]
fn value_and_type_roundtrip() {
    let value = crate::ValueAndType::new(
        crate::Value::String("test".to_string()),
        crate::analysis::analysed_type::str(),
    );
    let val = value.clone().into_value();
    let back = crate::ValueAndType::from_value(val).unwrap();
    assert_eq!(back, value);
}

#[test]
fn analysed_type_roundtrip() {
    let typ = crate::analysis::analysed_type::u32();
    let val = typ.clone().into_value();
    let back = crate::analysis::AnalysedType::from_value(val).unwrap();
    assert_eq!(back, typ);
}

#[test]
fn test_struct_roundtrip() {
    let original = TestStruct {
        a: 42,
        b: "hello".to_string(),
    };
    let val = original.clone().into_value();
    let back = TestStruct::from_value(val).unwrap();
    assert_eq!(back, original);
}

#[test]
fn test_newtype_roundtrip() {
    let original = TestNewtype(12345);
    let val = original.clone().into_value();
    let back = TestNewtype::from_value(val).unwrap();
    assert_eq!(back, original);
}

#[test]
fn test_tuple_roundtrip() {
    let original = TestTuple(100, "world".to_string());
    let val = original.clone().into_value();
    let back = TestTuple::from_value(val).unwrap();
    assert_eq!(back, original);
}

#[test]
fn test_enum_roundtrip() {
    for variant in [TestEnum::A, TestEnum::B, TestEnum::C] {
        let val = variant.clone().into_value();
        let back = TestEnum::from_value(val).unwrap();
        assert_eq!(back, variant);
    }
}

#[test]
fn test_variant_roundtrip() {
    let variants = vec![
        TestVariant::Unit,
        TestVariant::Single(42),
        TestVariant::Named {
            x: "test".to_string(),
            y: -10,
        },
        TestVariant::Tuple(99, "tuple".to_string()),
    ];

    for variant in variants {
        let val = variant.clone().into_value();
        let back = TestVariant::from_value(val).unwrap();
        assert_eq!(back, variant);
    }
}

#[test]
fn test_transparent_newtype_roundtrip() {
    let original = TransparentNewtype("transparent".to_string());
    let val = original.clone().into_value();
    // Since it's transparent, it should serialize as the inner String
    assert_eq!(val, Value::String("transparent".to_string()));
    let back = TransparentNewtype::from_value(val).unwrap();
    assert_eq!(back, original);
}

#[test]
fn test_transparent_newtype_roundtrip2() {
    let original = TransparentNewtype2 {
        value: "transparent".to_string(),
    };
    let val = original.clone().into_value();
    // Since it's transparent, it should serialize as the inner String
    assert_eq!(val, Value::String("transparent".to_string()));
    let back = TransparentNewtype2::from_value(val).unwrap();
    assert_eq!(back, original);
}

#[test]
fn test_renamed_field_roundtrip() {
    let original = TestWithRenamedField { field: 42 };
    let val = original.clone().into_value();
    let back = TestWithRenamedField::from_value(val).unwrap();
    assert_eq!(back, original);
}

#[test]
fn test_converted_and_skipped_fields_roundtrip() {
    let original = TestWithConvertedFieldAndSkip {
        field: WrappedU32(42),
        other: false,
    };
    let val = original.clone().into_value();
    let back = TestWithConvertedFieldAndSkip::from_value(val).unwrap();
    assert_eq!(back, original);
}

#[derive(Debug, Clone, PartialEq, IntoValue, FromValue)]
enum TestEnumWithSkipAndConvert {
    VariantA {
        #[wit_field(convert = u32)]
        field: WrappedU32,
        #[wit_field(skip)]
        skipped: bool,
    },
    VariantB {
        value: String,
    },
}

#[test]
fn test_enum_struct_variant_skip_and_convert_roundtrip() {
    let original = TestEnumWithSkipAndConvert::VariantA {
        field: WrappedU32(42),
        skipped: false,
    };
    let val = original.clone().into_value();
    let back = TestEnumWithSkipAndConvert::from_value(val).unwrap();
    assert_eq!(back, original);

    let original2 = TestEnumWithSkipAndConvert::VariantB {
        value: "hello".to_string(),
    };
    let val2 = original2.clone().into_value();
    let back2 = TestEnumWithSkipAndConvert::from_value(val2).unwrap();
    assert_eq!(back2, original2);
}

#[derive(Debug, Clone, PartialEq, IntoValue, FromValue)]
struct TestWithSkipFirst {
    #[wit_field(skip)]
    skipped: bool,
    #[wit_field(convert = u32)]
    field: WrappedU32,
}

#[test]
fn test_skip_first_field_roundtrip() {
    let original = TestWithSkipFirst {
        skipped: false,
        field: WrappedU32(99),
    };
    let val = original.clone().into_value();
    let back = TestWithSkipFirst::from_value(val).unwrap();
    assert_eq!(back, original);
}
