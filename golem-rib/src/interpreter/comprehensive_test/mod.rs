
#[cfg(test)]
mod complex_test {

}


mod data {
    use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
    use crate::interpret;
    use crate::interpreter::comprehensive_test::{data_types, internal};

    pub(crate) fn result_of_str() -> TypeAnnotatedValue {
        internal::get_type_annotated_value(&data_types::result_of_str_type(), "foo")
    }

    pub(crate) fn result_of_number() -> TypeAnnotatedValue {
        internal::get_type_annotated_value(&data_types::result_of_number_type(), "42")
    }

    pub(crate) fn result_of_option() -> TypeAnnotatedValue {
        internal::get_type_annotated_value(&data_types::result_of_option_type(), "Some(\"foo\")")
    }

    pub(crate) fn result_of_variant() -> TypeAnnotatedValue {
        internal::get_type_annotated_value(&data_types::result_of_variant_type(), "CaseStr(\"foo\")")
    }

    pub(crate) fn result_of_enum() -> TypeAnnotatedValue {
        internal::get_type_annotated_value(&data_types::result_of_enum_type(), "EnumA")
    }

    // TBD
    pub(crate) fn result_of_tuple() -> TypeAnnotatedValue {
        internal::get_type_annotated_value(&data_types::result_of_tuple_type(), "(\"foo\", 42)")
    }

    pub(crate) fn result_of_flag() -> TypeAnnotatedValue {
        internal::get_type_annotated_value(&data_types::result_of_flag_type(), "FeatureX")
    }

    pub(crate) fn result_of_record() -> TypeAnnotatedValue {
        todo!()
        ///  internal::get_type_annotated_value(&data_types::result_of_record_type(), r#"{"string_headers": {"authorization_string": "foo"}, "data_body": {"str": "foo", "list_of_str": ["foo"], "list_of_option": ["foo"], "list_of_list": [["foo"]], "list_of_variant": ["CaseStr(\"foo\")"], "list_of_enum": ["EnumA"], "list_of_tuple": [("foo", 42)], "list_of_record": [{"field_string_one": "foo", "field_string_two": "foo"}], "option_of_str": "foo", "option_of_option": "foo", "option_of_variant": "CaseStr(\"foo\")", "option_of_enum": "EnumA", "option_of_tuple": ("foo", 42), "option_of_record": {"field_string_one": "foo", "field_string_two": "foo"}, "option_of_list": ["foo"], "nested_record": {"field_string_one": "foo", "field_string_two": "foo"}, "variant_data_1": "CaseStr(\"foo\")", "variant_data_2": "CaseStr(\"foo\")", "variant_data_3": "CaseStr(\"foo\")", "variant_data_4": "CaseStr(\"foo\")", "variant_data_5": "CaseStr(\"foo\")", "variant_data_6": "CaseStr(\"foo\")", "enum_data_1": "EnumA", "enum_data_2": "EnumA", "enum_data_3": "EnumA", "flags_data_1": "FeatureX", "flags_data_2": "FeatureX", "flags_data_3": "FeatureX", "result_data_1": {"ok": "foo", "err": "foo"}, "result_data_2": {"ok": 42, "err": 42}, "result_data_3": {"ok": "EnumA", "err": "EnumA"}, "result_data_4": {"ok": "CaseStr(\"foo\")", "err": "CaseStr(\"foo\")"}, "result_data_5": {"ok": ("foo", 42), "err": ("foo", 42)}, "result_data_6": {"ok": "Some(\"foo\")", "err": "Some(\"foo\")
    }

    pub(crate) fn result_of_list() -> TypeAnnotatedValue {
        internal::get_type_annotated_value(&data_types::result_of_list_type(), "[\"foo\"]")
    }

    pub(crate) fn list_of_number() -> TypeAnnotatedValue {
        internal::get_type_annotated_value(&data_types::list_of_number_type_type(), "[42]")
    }

    pub(crate) fn list_of_str() -> TypeAnnotatedValue {
        internal::get_type_annotated_value(&data_types::list_of_str_type(), "[\"foo\"]")
    }

    pub(crate) fn list_of_option() -> TypeAnnotatedValue {
        internal::get_type_annotated_value(&data_types::list_of_option_type(), "[Some(\"foo\")]")
    }

    pub(crate) fn list_of_list() -> TypeAnnotatedValue {
        internal::get_type_annotated_value(&data_types::list_of_list_type(), "[[\"foo\"]]")
    }

    pub(crate) fn list_of_variant() -> TypeAnnotatedValue {
        internal::get_type_annotated_value(&data_types::list_of_variant_type(), "[CaseStr(\"foo\")]")
    }

    pub(crate) fn list_of_enum() -> TypeAnnotatedValue {
        internal::get_type_annotated_value(&data_types::list_of_enum_type(), "[EnumA]")
    }

    pub(crate) fn list_of_tuple() -> TypeAnnotatedValue {
        let tuple_str = internal::convert_type_annotated_value_to_str(&tuple());
        let wave_str = format!("[{}, {}]", &tuple_str, &tuple_str);
        internal::get_type_annotated_value(&data_types::list_of_tuple(), wave_str.as_str())
    }

    pub(crate) fn list_of_record() -> TypeAnnotatedValue {
        todo!()
        /// internal::get_type_annotated_value(&data_types::list_of_record_type(), r#"[{"field_string_one": "foo", "field_string_two": "foo"}]"#)
    }

    pub(crate) fn option_of_number() -> TypeAnnotatedValue {
        internal::get_type_annotated_value(&data_types::option_of_number_type(), "Some(42)")
    }

    pub(crate) fn option_of_str() -> TypeAnnotatedValue {
        internal::get_type_annotated_value(&data_types::option_of_str_type(), "Some(\"foo\")")
    }

    pub(crate) fn option_of_option() -> TypeAnnotatedValue {
        internal::get_type_annotated_value(&data_types::option_of_option_type(), "Some(Some(\"foo\"))")
    }

    pub(crate) fn option_of_variant() -> TypeAnnotatedValue {
        internal::get_type_annotated_value(&data_types::option_of_variant_type(), "Some(CaseStr(\"foo\"))")
    }

    pub(crate) fn option_of_enum() -> TypeAnnotatedValue {
        internal::get_type_annotated_value(&data_types::option_of_enum_type(), "Some(EnumA)")
    }

    pub(crate) fn option_of_tuple() -> TypeAnnotatedValue {
        let tuple_str = internal::convert_type_annotated_value_to_str(&tuple());
        let wave_str = format!("Some({})", tuple_str);
        internal::get_type_annotated_value(&data_types::option_of_tuple(), wave_str.as_str())
    }

    pub(crate) fn option_of_record() -> TypeAnnotatedValue {
        todo!()
        /// internal::get_type_annotated_value(&data_types::option_of_record_type(), r#"Some({"field_string_one": "foo", "field_string_two": "foo"})"#)
    }

    pub(crate) fn option_of_list() -> TypeAnnotatedValue {
        internal::get_type_annotated_value(&data_types::option_of_list(), "Some([\"foo\"])")
    }

    pub(crate) fn tuple() -> TypeAnnotatedValue {
        internal::get_type_annotated_value(&data_types::tuple_type(), "(\"foo\", 42, 42, 42, 42, true, 'a', some(42), ok(42), [true], CaseF64(42.0), {\"field_one\": true, \"field_two\": \"foo\"})")
    }

    pub(crate) fn enum_data() -> TypeAnnotatedValue {
        internal::get_type_annotated_value(&data_types::enum_type(), "EnumA")
    }

    pub(crate) fn str_data() -> TypeAnnotatedValue {
        internal::get_type_annotated_value(&data_types::str_type(), "foo")
    }

    pub(crate) fn flag_data() -> TypeAnnotatedValue {
        internal::get_type_annotated_value(&data_types::flag_type(), "FeatureX")
    }
}

mod data_types {
    use golem_wasm_ast::analysis::*;
    use crate::interpreter::comprehensive_test::internal;


    // Result
    pub(crate) fn result_of_str_type() -> AnalysedType {
        AnalysedType::Result(TypeResult {
            ok: Some(Box::new(AnalysedType::Str(TypeStr))),
            err: Some(Box::new(AnalysedType::Str(TypeStr))),
        })
    }

    pub(crate) fn result_of_number_type() -> AnalysedType {
        AnalysedType::Result(TypeResult {
            ok: Some(Box::new(AnalysedType::U64(TypeU64))),
            err: Some(Box::new(AnalysedType::U64(TypeU64))),
        })
    }

    pub(crate) fn result_of_option_type() -> AnalysedType {
        AnalysedType::Result(TypeResult {
            ok: Some(Box::new(option_of_str_type())),
            err: Some(Box::new(option_of_str_type())),
        })
    }

    pub(crate) fn result_of_variant_type() -> AnalysedType {
        AnalysedType::Result(TypeResult {
            ok: Some(Box::new(variant_type())),
            err: Some(Box::new(variant_type())),
        })
    }

    pub(crate) fn result_of_enum_type() -> AnalysedType {
        AnalysedType::Result(TypeResult {
            ok: Some(Box::new(enum_type())),
            err: Some(Box::new(enum_type())),
        })
    }


    pub(crate) fn result_of_tuple_type() -> AnalysedType {
        AnalysedType::Result(TypeResult {
            ok: Some(Box::new(tuple_type())),
            err: Some(Box::new(tuple_type())),
        })
    }

    pub(crate) fn result_of_flag_type() -> AnalysedType {
        AnalysedType::Result(TypeResult {
            ok: Some(Box::new(flag_type())),
            err: Some(Box::new(flag_type())),
        })
    }

    pub(crate) fn result_of_record_type() -> AnalysedType {
        AnalysedType::Result(TypeResult {
            ok: Some(Box::new(record_type())),
            err: Some(Box::new(record_type())),
        })
    }

    pub(crate) fn result_of_list_type() -> AnalysedType {
        AnalysedType::Result(TypeResult {
            ok: Some(Box::new(list_of_str_type())),
            err: Some(Box::new(list_of_str_type())),
        })
    }


    // List
    pub(crate) fn list_of_number_type_type() -> AnalysedType {
        AnalysedType::List(TypeList {
            inner: Box::new(AnalysedType::U64(TypeU64)),
        })
    }

    pub(crate) fn list_of_str_type() -> AnalysedType {
        AnalysedType::List(TypeList {
            inner: Box::new(AnalysedType::Str(TypeStr)),
        })
    }

    pub(crate) fn list_of_option_type() -> AnalysedType {
        AnalysedType::List(TypeList {
            inner: Box::new(AnalysedType::Option(TypeOption {
                inner: Box::new(AnalysedType::Str(TypeStr)),
            })),
        })
    }

    pub(crate) fn list_of_list_type() -> AnalysedType {
        AnalysedType::List(TypeList {
            inner: Box::new(AnalysedType::List(TypeList {
                inner: Box::new(AnalysedType::Str(TypeStr)),
            })),
        })
    }

    pub(crate) fn list_of_variant_type() -> AnalysedType {
        AnalysedType::List(TypeList {
            inner: Box::new(variant_type()),
        })
    }

    pub(crate) fn list_of_enum_type() -> AnalysedType {
        AnalysedType::List(TypeList {
            inner: Box::new(enum_type()),
        })
    }

    pub(crate) fn list_of_tuple() -> AnalysedType {
        AnalysedType::List(TypeList {
            inner: Box::new(tuple_type()),
        })
    }

    pub(crate) fn list_of_record_type() -> AnalysedType {
        AnalysedType::List(TypeList {
            inner: Box::new(record_type())
        })
    }

    pub(crate) fn option_of_number_type() -> AnalysedType {
        AnalysedType::Option(TypeOption {
            inner: Box::new(AnalysedType::U64(TypeU64)),
        })
    }

    // Option
    pub(crate) fn option_of_str_type() -> AnalysedType {
        AnalysedType::Option(TypeOption {
            inner: Box::new(AnalysedType::Str(TypeStr)),
        })
    }

    pub(crate) fn option_of_option_type() -> AnalysedType {
        AnalysedType::Option(TypeOption {
            inner: Box::new(AnalysedType::Option(TypeOption {
                inner: Box::new(AnalysedType::Str(TypeStr)),
            })),
        })
    }

    pub(crate) fn option_of_variant_type() -> AnalysedType {
        AnalysedType::Option(TypeOption {
            inner: Box::new(variant_type()),
        })
    }

    pub(crate) fn option_of_enum_type() -> AnalysedType {
        AnalysedType::Option(TypeOption {
            inner: Box::new(enum_type()),
        })
    }

    pub(crate) fn option_of_tuple() -> AnalysedType {
        AnalysedType::Option(TypeOption {
            inner: Box::new(tuple_type()),
        })
    }


    pub(crate) fn option_of_record_type() -> AnalysedType {
        AnalysedType::Option(TypeOption {
            inner: Box::new(record_type())
        })
    }

    pub(crate) fn option_of_list() -> AnalysedType {
        AnalysedType::Option(TypeOption {
            inner: Box::new(AnalysedType::List(TypeList {
                inner: Box::new(AnalysedType::Str(TypeStr)),
            })),
        })
    }

    // Record
    pub(crate) fn record_type() -> AnalysedType {
        internal::analysed_type_record(vec![
            (
                "string_headers",
                internal::analysed_type_record(vec![(
                    "authorization_string",
                    AnalysedType::Str(TypeStr),
                )]),
            ),
            (
                "data_body",
                internal::analysed_type_record(vec![
                    ("str", AnalysedType::Str(TypeStr)),
                    (
                        "list_of_str",
                        list_of_str_type(),
                    ),
                    (
                        "list_of_option",
                        list_of_option_type(),
                    ),
                    (
                        "list_of_list",
                        list_of_list_type(),
                    ),
                    (
                        "list_of_variant",
                        list_of_variant_type(),
                    ),
                    (
                        "list_of_enum",
                        list_of_enum_type(),
                    ),
                    (
                        "list_of_tuple",
                        list_of_tuple(),
                    ),
                    (
                        "list_of_record",
                        AnalysedType::List(TypeList {
                            inner: Box::new(internal::analysed_type_record(vec![
                                ("field_string_one", AnalysedType::Str(TypeStr)),
                                ("field_string_two", AnalysedType::Str(TypeStr)),
                            ])),
                        }),
                    ),
                    (
                        "option_of_str",
                        option_of_str_type(),
                    ),
                    (
                        "option_of_option",
                        option_of_option_type(),
                    ),
                    (
                        "option_of_variant",
                        option_of_variant_type(),
                    ),
                    (
                        "option_of_enum",
                        option_of_enum_type(),
                    ),
                    (
                        "option_of_tuple",
                        option_of_tuple(),
                    ),
                    (
                        "option_of_record",
                        AnalysedType::Option(TypeOption {
                            inner: Box::new(internal::analysed_type_record(vec![
                                ("field_string_one", AnalysedType::Str(TypeStr)),
                                ("field_string_two", AnalysedType::Str(TypeStr)),
                            ])),
                        }),
                    ),
                    (
                        "option_of_list",
                        option_of_list(),
                    ),
                    (
                        "nested_record",
                        internal::analysed_type_record(vec![
                            ("field_string_one", AnalysedType::Str(TypeStr)),
                            ("field_string_two", AnalysedType::Str(TypeStr)),
                        ]),
                    ),
                    (
                        "variant_data_1",
                        variant_type(),
                    ),
                    (
                        "variant_data_2",
                        variant_type(),
                    ),
                    (
                        "variant_data_3",
                        variant_type(),
                    ),
                    (
                        "variant_data_4",
                        variant_type(),
                    ),
                    (
                        "variant_data_5",
                        variant_type(),
                    ),
                    (
                        "variant_data_6",
                        variant_type(),
                    ),
                    (
                        "enum_data_1",
                        enum_type(),
                    ),
                    (
                        "enum_data_2",
                        enum_type(),
                    ),
                    (
                        "enum_data_3",
                        enum_type(),
                    ),
                    (
                        "flags_data_1",
                        flag_type(),
                    ),
                    (
                        "flags_data_2",
                        flag_type(),
                    ),
                    (
                        "flags_data_3",
                        flag_type(),
                    ),
                    (
                        "result_data_1",
                        result_of_str_type(),
                    ),
                    (
                        "result_data_2",
                        result_of_number_type(),
                    ),
                    (
                        "result_data_3",
                        result_of_enum_type(),
                    ),
                    (
                        "result_data_4",
                        result_of_variant_type(),
                    ),
                    (
                        "result_data_5",
                        result_of_tuple_type(),
                    ),
                    (
                        "result_data_6",
                        result_of_option_type(),
                    ),
                    (
                        "result_data_7",
                        internal::analysed_type_record(vec![
                            ("field_string_one", AnalysedType::Str(TypeStr)),
                            ("field_string_two", AnalysedType::Str(TypeStr)),
                        ]),
                    ),
                    (
                        "result_data_8",
                        result_of_flag_type()
                    ),
                    (
                        "nested_record",
                        internal::analysed_type_record(vec![
                            ("field_string_one", AnalysedType::Str(TypeStr)),
                            ("field_string_two", AnalysedType::Str(TypeStr)),
                        ]),
                    ),

                    ("tuple_data", AnalysedType::Tuple(TypeTuple {
                        items: vec![
                            AnalysedType::Str(TypeStr),
                            AnalysedType::U32(TypeU32),
                            AnalysedType::Bool(TypeBool),
                        ],
                    })),
                    ("character_data", AnalysedType::Chr(TypeChr)),
                    ("f64_data", AnalysedType::F64(TypeF64)),
                    ("f32_data", AnalysedType::F32(TypeF32)),
                    ("u64_data", AnalysedType::U64(TypeU64)),
                    ("s64_data", AnalysedType::S64(TypeS64)),
                    ("u32_data", AnalysedType::U32(TypeU32)),
                    ("s32_data", AnalysedType::S32(TypeS32)),
                    ("u16_data", AnalysedType::U16(TypeU16)),
                    ("s16_data", AnalysedType::S16(TypeS16)),
                    ("u8_data", AnalysedType::U8(TypeU8)),
                    ("s8_data", AnalysedType::S8(TypeS8)),
                    ("boolean_data", AnalysedType::Bool(TypeBool)),
                ]),
            ),
        ])
    }

    // Tuple
    pub(crate) fn tuple_type() -> AnalysedType {
        AnalysedType::Tuple(TypeTuple {
            items: vec![
                AnalysedType::Str(TypeStr),                    // Option<String>
                AnalysedType::U64(TypeU64),                    // Option<u64>
                AnalysedType::S32(TypeS32),                    // Option<i32>
                AnalysedType::F32(TypeF32),                    // Option<f32>
                AnalysedType::F64(TypeF64),                    // Option<f64>
                AnalysedType::Bool(TypeBool),                  // Option<bool>
                AnalysedType::Chr(TypeChr),                    // Option<char>
                AnalysedType::Option(TypeOption {              // Option<Option>
                    inner: Box::new(AnalysedType::S16(TypeS16)),
                }),
                AnalysedType::Result(TypeResult {              // Option<Result>
                    ok: Some(Box::new(AnalysedType::U8(TypeU8))),
                    err: Some(Box::new(AnalysedType::S8(TypeS8))),
                }),
                AnalysedType::List(TypeList {                  // Option<List>
                    inner: Box::new(AnalysedType::Bool(TypeBool)),
                }),
                AnalysedType::Variant(TypeVariant {            // Option<Variant>
                    cases: vec![
                        NameOptionTypePair {
                            name: "CaseF64".to_string(),
                            typ: Some(AnalysedType::F64(TypeF64)),
                        },
                        NameOptionTypePair {
                            name: "CaseNone".to_string(),
                            typ: None,
                        },
                    ],
                }),
                AnalysedType::Record(TypeRecord {              // Option<Record>
                    fields: vec![
                        NameTypePair {
                            name: "field_one".to_string(),
                            typ: AnalysedType::Bool(TypeBool),
                        },
                        NameTypePair {
                            name: "field_two".to_string(),
                            typ: AnalysedType::Str(TypeStr),
                        },
                    ],
                }),
            ],
        })
    }

    // Enum
    pub(crate) fn enum_type() -> AnalysedType {
        AnalysedType::Enum(TypeEnum {
            cases: vec!["EnumA".to_string(), "EnumB".to_string(), "EnumC".to_string()],
        })
    }


    // Str
    pub(crate) fn str_type() -> AnalysedType {
        AnalysedType::Str(TypeStr)
    }

    // Flag
    pub(crate) fn flag_type() -> AnalysedType {
        AnalysedType::Flags(TypeFlags {
            names: vec!["FeatureX".to_string(), "FeatureY".to_string(), "FeatureZ".to_string()],
        })
    }

    // Variant
    pub(crate) fn variant_type() -> AnalysedType {
        AnalysedType::Variant(TypeVariant {
            cases: vec![
                NameOptionTypePair {
                    name: "CaseNone".to_string(),
                    typ: None,
                },
                NameOptionTypePair {
                    name: "CaseStr".to_string(),
                    typ: Some(AnalysedType::Str(TypeStr)),      // Variant case for String
                },
                NameOptionTypePair {
                    name: "CaseU64".to_string(),
                    typ: Some(AnalysedType::U64(TypeU64)),      // Variant case for u64
                },
                NameOptionTypePair {
                    name: "CaseS32".to_string(),
                    typ: Some(AnalysedType::S32(TypeS32)),      // Variant case for i32
                },
                NameOptionTypePair {
                    name: "CaseF32".to_string(),
                    typ: Some(AnalysedType::F32(TypeF32)),      // Variant case for f32
                },
                NameOptionTypePair {
                    name: "CaseF64".to_string(),
                    typ: Some(AnalysedType::F64(TypeF64)),      // Variant case for f64
                },
                NameOptionTypePair {
                    name: "CaseBool".to_string(),
                    typ: Some(AnalysedType::Bool(TypeBool)),    // Variant case for bool
                },
                NameOptionTypePair {
                    name: "CaseChr".to_string(),
                    typ: Some(AnalysedType::Chr(TypeChr)),      // Variant case for char
                },
                NameOptionTypePair {
                    name: "CaseList".to_string(),
                    typ: Some(AnalysedType::List(TypeList {     // Variant case for List
                        inner: Box::new(AnalysedType::S16(TypeS16)),
                    })),
                },
                NameOptionTypePair {
                    name: "CaseOption".to_string(),
                    typ: Some(AnalysedType::Option(TypeOption { // Variant case for Option
                        inner: Box::new(AnalysedType::U16(TypeU16)),
                    })),
                },
                NameOptionTypePair {
                    name: "CaseResult".to_string(),
                    typ: Some(AnalysedType::Result(TypeResult { // Variant case for Result
                        ok: Some(Box::new(AnalysedType::U8(TypeU8))),
                        err: Some(Box::new(AnalysedType::S8(TypeS8))),
                    })),
                },
                NameOptionTypePair {
                    name: "CaseRecord".to_string(),
                    typ: Some(AnalysedType::Record(TypeRecord { // Variant case for Record
                        fields: vec![
                            NameTypePair {
                                name: "field1".to_string(),
                                typ: AnalysedType::Str(TypeStr),
                            },
                            NameTypePair {
                                name: "field2".to_string(),
                                typ: AnalysedType::Bool(TypeBool),
                            },
                        ],
                    })),
                },
                NameOptionTypePair {
                    name: "CaseTuple".to_string(),
                    typ: Some(AnalysedType::Tuple(TypeTuple {   // Variant case for Tuple
                        items: vec![
                            AnalysedType::F32(TypeF32),
                            AnalysedType::U32(TypeU32),
                        ],
                    })),
                },
            ],
        })
    }

}

mod internal {
    use golem_wasm_ast::analysis::{AnalysedType, NameTypePair, TypeRecord};
    use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;

    pub(crate) fn analysed_type_record(fields: Vec<(&str, AnalysedType)>) -> AnalysedType {
        AnalysedType::Record(TypeRecord {
            fields: fields
                .into_iter()
                .map(|(name, typ)| NameTypePair {
                    name: name.to_string(),
                    typ,
                })
                .collect(),
        })
    }

    pub(crate) fn get_type_annotated_value(
        analysed_type: &AnalysedType,
        wasm_wave_str: &str,
    ) -> TypeAnnotatedValue {
        golem_wasm_rpc::type_annotated_value_from_str(analysed_type, wasm_wave_str).unwrap()
    }

    pub(crate) fn convert_type_annotated_value_to_str(
        type_annotated_value: &TypeAnnotatedValue,
    ) -> String {
        golem_wasm_rpc::type_annotated_value_to_string(type_annotated_value).unwrap()
    }
}