use crate::interpreter::comprehensive_test::{data_types, test_utils};
#[cfg(test)]
use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;

pub(crate) fn result_of_str() -> TypeAnnotatedValue {
    test_utils::get_type_annotated_value(&data_types::result_of_str_type(), "ok(\"foo\")")
}

pub(crate) fn result_of_number() -> TypeAnnotatedValue {
    test_utils::get_type_annotated_value(&data_types::result_of_number_type(), "ok(42)")
}

pub(crate) fn result_of_option() -> TypeAnnotatedValue {
    test_utils::get_type_annotated_value(&data_types::result_of_option_type(), "ok(some(\"foo\"))")
}

pub(crate) fn result_of_variant() -> TypeAnnotatedValue {
    test_utils::get_type_annotated_value(
        &data_types::result_of_variant_type(),
        "ok(case-str(\"foo\"))",
    )
}

pub(crate) fn result_of_enum() -> TypeAnnotatedValue {
    test_utils::get_type_annotated_value(&data_types::result_of_enum_type(), "ok(enum-a)")
}

pub(crate) fn result_of_tuple() -> TypeAnnotatedValue {
    let tuple_str = test_utils::convert_type_annotated_value_to_str(&tuple());
    let wave_str = format!("ok({})", tuple_str);
    test_utils::get_type_annotated_value(&data_types::result_of_tuple_type(), wave_str.as_str())
}

pub(crate) fn result_of_flag() -> TypeAnnotatedValue {
    test_utils::get_type_annotated_value(&data_types::result_of_flag_type(), "ok({featurex})")
}

pub(crate) fn result_of_record() -> TypeAnnotatedValue {
    let record_str = test_utils::convert_type_annotated_value_to_str(&record());
    let wave_str = format!("ok({})", &record_str);
    test_utils::get_type_annotated_value(&data_types::result_of_record_type(), wave_str.as_str())
}

pub(crate) fn result_of_list() -> TypeAnnotatedValue {
    test_utils::get_type_annotated_value(&data_types::result_of_list_type(), "ok([\"foo\"])")
}

pub(crate) fn list_of_number() -> TypeAnnotatedValue {
    test_utils::get_type_annotated_value(&data_types::list_of_number_type_type(), "[42]")
}

pub(crate) fn list_of_str() -> TypeAnnotatedValue {
    test_utils::get_type_annotated_value(&data_types::list_of_str_type(), "[\"foo\"]")
}

pub(crate) fn list_of_option() -> TypeAnnotatedValue {
    test_utils::get_type_annotated_value(&data_types::list_of_option_type(), "[some(\"foo\")]")
}

pub(crate) fn list_of_list() -> TypeAnnotatedValue {
    test_utils::get_type_annotated_value(&data_types::list_of_list_type(), "[[\"foo\"]]")
}

pub(crate) fn list_of_variant() -> TypeAnnotatedValue {
    test_utils::get_type_annotated_value(&data_types::list_of_variant_type(), "[case-str(\"foo\")]")
}

pub(crate) fn list_of_enum() -> TypeAnnotatedValue {
    test_utils::get_type_annotated_value(&data_types::list_of_enum_type(), "[enum-a]")
}

pub(crate) fn list_of_tuple() -> TypeAnnotatedValue {
    let tuple_str = test_utils::convert_type_annotated_value_to_str(&tuple());
    let wave_str = format!("[{}, {}]", &tuple_str, &tuple_str);
    test_utils::get_type_annotated_value(&data_types::list_of_tuple(), wave_str.as_str())
}

pub(crate) fn list_of_record() -> TypeAnnotatedValue {
    let record_str = test_utils::convert_type_annotated_value_to_str(&record());
    let wave_str = format!("[{}]", &record_str);
    test_utils::get_type_annotated_value(&data_types::list_of_record_type(), wave_str.as_str())
}

pub(crate) fn option_of_number() -> TypeAnnotatedValue {
    test_utils::get_type_annotated_value(&data_types::option_of_number_type(), "some(42)")
}

pub(crate) fn option_of_str() -> TypeAnnotatedValue {
    test_utils::get_type_annotated_value(&data_types::option_of_str_type(), "some(\"foo\")")
}

pub(crate) fn option_of_option() -> TypeAnnotatedValue {
    test_utils::get_type_annotated_value(
        &data_types::option_of_option_type(),
        "some(some(\"foo\"))",
    )
}

pub(crate) fn option_of_variant() -> TypeAnnotatedValue {
    test_utils::get_type_annotated_value(
        &data_types::option_of_variant_type(),
        "some(case-str(\"foo\"))",
    )
}

pub(crate) fn option_of_enum() -> TypeAnnotatedValue {
    test_utils::get_type_annotated_value(&data_types::option_of_enum_type(), "some(enum-a)")
}

pub(crate) fn option_of_tuple() -> TypeAnnotatedValue {
    let tuple_str = test_utils::convert_type_annotated_value_to_str(&tuple());
    let wave_str = format!("some({})", tuple_str);
    test_utils::get_type_annotated_value(&data_types::option_of_tuple(), wave_str.as_str())
}

pub(crate) fn option_of_record() -> TypeAnnotatedValue {
    let record_str = test_utils::convert_type_annotated_value_to_str(&record());
    let wave_str = format!("some({})", &record_str);
    test_utils::get_type_annotated_value(&data_types::option_of_record_type(), wave_str.as_str())
}

pub(crate) fn option_of_list() -> TypeAnnotatedValue {
    test_utils::get_type_annotated_value(&data_types::option_of_list(), "some([\"foo\"])")
}

pub(crate) fn tuple() -> TypeAnnotatedValue {
    test_utils::get_type_annotated_value(
        &data_types::tuple_type(),
        r#"
          ("foo", 42, 42, 42, 42, true, 'a', some(42), ok(42), [true], case-hello(42.0), {field-one: true, field-two: "foo"})"#,
    )
}

pub(crate) fn enum_data() -> TypeAnnotatedValue {
    test_utils::get_type_annotated_value(&data_types::enum_type(), "enum-a")
}

pub(crate) fn str_data() -> TypeAnnotatedValue {
    test_utils::get_type_annotated_value(&data_types::str_type(), "\"foo\"")
}

pub(crate) fn number_data() -> TypeAnnotatedValue {
    test_utils::get_type_annotated_value(&data_types::number_type(), "42")
}

pub(crate) fn flag() -> TypeAnnotatedValue {
    test_utils::get_type_annotated_value(&data_types::flag_type(), "{featurex}")
}

pub(crate) fn variant() -> TypeAnnotatedValue {
    test_utils::get_type_annotated_value(&data_types::variant_type(), "case-str(\"foo\")")
}

pub(crate) fn record() -> TypeAnnotatedValue {
    test_utils::get_type_annotated_value(
        &data_types::record_type(),
        r#"
          {
            string-headers: {authorization-string: "foo"},
            data-body: {
              str: "foo",
              list-of-str: ["foo"],
              list-of-option: ["foo"],
              list-of-list: [["foo"]],
              list-of-variant: [case-str("foo")],
              list-of-enum: [enum-a],
              list-of-tuple: [("foo", 42, 42, 42, 42, true, 'a', some(42), ok(42), [true], case-hello(42.0), {field-one: true, field-two: "foo"})],
              list-of-record: [{field-string-one: "foo", field-string-two: "foo"}],
              nested-record: {field-string-one: "foo", field-string-two: "foo"},
              option-of-str: some("foo"),
              option-of-option: some(some("foo")),
              option-of-variant: some(case-str("foo")),
              option-of-enum: some(enum-a),
              option-of-tuple: some(("foo", 42, 42, 42, 42, true, 'a', some(42), ok(42), [true], case-hello(42.0), {field-one: true, field-two: "foo"})),
              option-of-record: some({field-string-one: "foo", field-string-two: "foo"}),
              option-of-list: some(["foo"]),
              variant-data-a: case-str("foo")
              variant-data-b: case-str("foo"),
              variant-data-c: case-str("foo"),
              variant-data-d: case-str("foo"),
              variant-data-e: case-str("foo"),
              variant-data-f: case-str("foo"),
              variant-data-g: case-str("foo"),
              enum-data-a: enum-a,
              enum-data-b: enum-b,
              enum-data-c: enum-c,
              flags-data-a: { featurex },
              flags-data-b: { featurex, featurey },
              flags-data-c: { featurex, featurey, featurez },
              result-data-a: ok("foo"),
              result-data-b: ok(42),
              result-data-c: ok(enum-a),
              result-data-d: ok(case-str("foo")),
              result-data-e: ok(("foo", 42, 42, 42, 42, true, 'a', some(42), ok(42), [true], case-hello(42.0), {field-one: true, field-two: "foo"})),
              result-data-f: ok(some("foo")),
              result-data-g: err("foo"),
              result-data-h: err(42),
              result-data-i: err(enum-a),
              result-data-j: err(case-str("foo")),
              result-data-k: err(("foo", 42, 42, 42, 42, true, 'a', some(42), ok(42), [true], case-hello(42.0), {field-one: true, field-two: "foo"})),
              result-data-l: err(some("foo")),
              result-data-m: ok({ featurex, featurey, featurez }),
              result-data-n: err({ featurex, featurey, featurez })
              tuple-data: ("foo", 42, 42, 42, 42, true, 'a', some(42), ok(42), [true], case-hello(42.0), {field-one: true, field-two: "foo"}),
              character-data : 'x',
              f64-data : 3.14,
              f32-data : 3.14,
              u64-data : 42,
              s64-data : 42,
              u32-data : 42,
              s32-data : 42,
              u16-data : 42,
              s16-data : 42,
              u8-data : 42,
              s8-data : 42,
              boolean-data : true
           }
          }"#,
    )
}
