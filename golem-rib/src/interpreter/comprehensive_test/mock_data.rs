#[cfg(test)]

use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
use crate::interpreter::comprehensive_test::{data_types, test_utils};

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
    test_utils::get_type_annotated_value(&data_types::result_of_variant_type(), "ok(case-str(\"foo\"))")
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
    test_utils::get_type_annotated_value(&data_types::result_of_flag_type(), "ok({FeatureX})")
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
    test_utils::get_type_annotated_value(&data_types::option_of_option_type(), "some(some(\"foo\"))")
}

pub(crate) fn option_of_variant() -> TypeAnnotatedValue {
    test_utils::get_type_annotated_value(&data_types::option_of_variant_type(), "some(case-str(\"foo\"))")
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
    test_utils::get_type_annotated_value(&data_types::list_of_record_type(), wave_str.as_str())
}

pub(crate) fn option_of_list() -> TypeAnnotatedValue {
    test_utils::get_type_annotated_value(&data_types::option_of_list(), "some([\"foo\"])")
}

    pub(crate) fn tuple() -> TypeAnnotatedValue {
        test_utils::get_type_annotated_value(&data_types::tuple_type(), r#"
          ("foo", 42, 42, 42, 42, true, 'a', some(42), ok(42), [true], case-hello(42.0), {field-one: true, field-two: "foo"})"#
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
    test_utils::get_type_annotated_value(&data_types::flag_type(), "{FeatureX}")
}

pub(crate) fn variant() -> TypeAnnotatedValue {
    test_utils::get_type_annotated_value(&data_types::variant_type(), "case-str(\"foo\")")
}

pub(crate) fn record() -> TypeAnnotatedValue {
    test_utils::get_type_annotated_value(&data_types::record_type(), r#"
          {
            string_headers: {authorization_string: "foo"},
            data_body: {
              str: "foo",
              list_of_str: ["foo"],
              list_of_option: ["foo"],
              list_of_list: [["foo"]],
              list_of_variant: ["case-str(\"foo\")"],
              list_of_enum: ["enum-a"],
              list_of_tuple: [("foo", 42)],
              list_of_record: [{"field_string_one": "foo", "field_string_two": "foo"}],
              option_of_str: some("foo"),
              option_of_option: some(some("foo")),
              option_of_variant: some(case-str(\"foo\")),
              option_of_enum: some("enum-a"),
              option_of_tuple: some(("foo", 42)),
              option_of_record: some({"field_string_one": "foo", "field_string_two": "foo"}),
              option_of_list: some(["foo"]),
              nested_record: {"field_string_one": "foo", "field_string_two": "foo"},
              variant_data_1: case-str(\"foo\"),
              variant_data_2: case-str(\"foo\"),
              variant_data_3: case-str(\"foo\"),
              variant_data_4: case-str(\"foo\"),
              variant_data_5: case-str(\"foo\"),
              variant_data_6: case-str(\"foo\"),
              enum_data_1: enum-a,
              enum_data_2: enum-b,
              enum_data_3: EnumC,
              flags_data_1: { FeatureX },
              flags_data_2: { FeatureX, FeatureY },
              flags_data_3: { FeatureX, FeatureY, FeatureZ },
              result_data_1: ok("foo"),
              result_data_2: ok(42),
              result_data_3: ok(enum-a),
              result_data_4: ok(case-str(\"foo\")),
              result_data_5: ok(("foo", 42)),
              result_data_6: ok(Some(\"foo\")),
              result_data_7: err("foo"),
              result_data_8: err(42),
              result_data_9: err(enum-a),
              result_data_10: err(case-str(\"foo\")),
              result_data_11: err(("foo", 42)),
              result_data_12: err(Some(\"foo\")),
              result_data_13: ok({"field_string_one": "foo", "field_string_two": "foo"}),
              result_data_14: err({"field_string_one": "foo", "field_string_two": "foo"}),
              result_data_15: ok({ FeatureX, FeatureY, FeatureZ }),
              result_data_16: err({ FeatureX, FeatureY, FeatureZ }),
              character_data : 'x',
              f64_data : 3.14,
              f32_data : 3.14,
              u64_data : 42,
              s64_data : 42,
              u32_data : 42,
              s32_data : 42,
              u16_data : 42,
              s16_data : 42,
              u8_data : 42,
              s8_data : 42,
              boolean_data : true,
           }"#)
}
