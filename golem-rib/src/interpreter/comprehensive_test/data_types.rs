use crate::interpreter::comprehensive_test::test_utils;
#[cfg(test)]
use golem_wasm_ast::analysis::*;

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
        inner: Box::new(record_type()),
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
        inner: Box::new(record_type()),
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
    test_utils::analysed_type_record(vec![
        (
            "string_headers",
            test_utils::analysed_type_record(vec![(
                "authorization_string",
                AnalysedType::Str(TypeStr),
            )]),
        ),
        (
            "data_body",
            test_utils::analysed_type_record(vec![
                ("str", AnalysedType::Str(TypeStr)),
                ("list_of_str", list_of_str_type()),
                ("list_of_option", list_of_option_type()),
                ("list_of_list", list_of_list_type()),
                ("list_of_variant", list_of_variant_type()),
                ("list_of_enum", list_of_enum_type()),
                ("list_of_tuple", list_of_tuple()),
                (
                    "list_of_record",
                    AnalysedType::List(TypeList {
                        inner: Box::new(test_utils::analysed_type_record(vec![
                            ("field_string_one", AnalysedType::Str(TypeStr)),
                            ("field_string_two", AnalysedType::Str(TypeStr)),
                        ])),
                    }),
                ),
                ("option_of_str", option_of_str_type()),
                ("option_of_option", option_of_option_type()),
                ("option_of_variant", option_of_variant_type()),
                ("option_of_enum", option_of_enum_type()),
                ("option_of_tuple", option_of_tuple()),
                (
                    "option_of_record",
                    AnalysedType::Option(TypeOption {
                        inner: Box::new(test_utils::analysed_type_record(vec![
                            ("field_string_one", AnalysedType::Str(TypeStr)),
                            ("field_string_two", AnalysedType::Str(TypeStr)),
                        ])),
                    }),
                ),
                ("option_of_list", option_of_list()),
                (
                    "nested_record",
                    test_utils::analysed_type_record(vec![
                        ("field_string_one", AnalysedType::Str(TypeStr)),
                        ("field_string_two", AnalysedType::Str(TypeStr)),
                    ]),
                ),
                ("variant_data_1", variant_type()),
                ("variant_data_2", variant_type()),
                ("variant_data_3", variant_type()),
                ("variant_data_4", variant_type()),
                ("variant_data_5", variant_type()),
                ("variant_data_6", variant_type()),
                ("enum_data_1", enum_type()),
                ("enum_data_2", enum_type()),
                ("enum_data_3", enum_type()),
                ("flags_data_1", flag_type()),
                ("flags_data_2", flag_type()),
                ("flags_data_3", flag_type()),
                ("result_data_1", result_of_str_type()),
                ("result_data_2", result_of_number_type()),
                ("result_data_3", result_of_enum_type()),
                ("result_data_4", result_of_variant_type()),
                ("result_data_5", result_of_tuple_type()),
                ("result_data_6", result_of_option_type()),
                ("result_data_7", result_of_str_type()),
                ("result_data_8", result_of_number_type()),
                ("result_data_9", result_of_enum_type()),
                ("result_data_10", result_of_variant_type()),
                ("result_data_11", result_of_tuple_type()),
                ("result_data_12", result_of_option_type()),
                (
                    "result_data_13",
                    test_utils::analysed_type_record(vec![
                        ("field_string_one", AnalysedType::Str(TypeStr)),
                        ("field_string_two", AnalysedType::Str(TypeStr)),
                    ]),
                ),
                (
                    "result_data_14",
                    test_utils::analysed_type_record(vec![
                        ("field_string_one", AnalysedType::Str(TypeStr)),
                        ("field_string_two", AnalysedType::Str(TypeStr)),
                    ]),
                ),
                ("result_data_15", result_of_flag_type()),
                ("result_data_16", result_of_flag_type()),
                (
                    "nested_record",
                    test_utils::analysed_type_record(vec![
                        ("field_string_one", AnalysedType::Str(TypeStr)),
                        ("field_string_two", AnalysedType::Str(TypeStr)),
                    ]),
                ),
                (
                    "tuple_data",
                    AnalysedType::Tuple(TypeTuple {
                        items: vec![
                            AnalysedType::Str(TypeStr),
                            AnalysedType::U32(TypeU32),
                            AnalysedType::Bool(TypeBool),
                        ],
                    }),
                ),
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
            AnalysedType::Str(TypeStr),
            AnalysedType::U64(TypeU64),
            AnalysedType::S32(TypeS32),
            AnalysedType::F32(TypeF32),
            AnalysedType::F64(TypeF64),
            AnalysedType::Bool(TypeBool),
            AnalysedType::Chr(TypeChr),
            AnalysedType::Option(TypeOption {
                inner: Box::new(AnalysedType::S16(TypeS16)),
            }),
            AnalysedType::Result(TypeResult {
                ok: Some(Box::new(AnalysedType::U8(TypeU8))),
                err: Some(Box::new(AnalysedType::S8(TypeS8))),
            }),
            AnalysedType::List(TypeList {
                inner: Box::new(AnalysedType::Bool(TypeBool)),
            }),
            AnalysedType::Variant(TypeVariant {
                cases: vec![
                    NameOptionTypePair {
                        name: "case-hello".to_string(),
                        typ: Some(AnalysedType::F64(TypeF64)),
                    },
                    NameOptionTypePair {
                        name: "case-none".to_string(),
                        typ: None,
                    },
                ],
            }),
            AnalysedType::Record(TypeRecord {
                // Option<Record>
                fields: vec![
                    NameTypePair {
                        name: "field-one".to_string(),
                        typ: AnalysedType::Bool(TypeBool),
                    },
                    NameTypePair {
                        name: "field-two".to_string(),
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
        cases: vec![
            "enum-a".to_string(),
            "enum-b".to_string(),
            "enum-c".to_string(),
        ],
    })
}

// Str
pub(crate) fn str_type() -> AnalysedType {
    AnalysedType::Str(TypeStr)
}

// Number
pub(crate) fn number_type() -> AnalysedType {
    AnalysedType::U64(TypeU64)
}

// Flag
pub(crate) fn flag_type() -> AnalysedType {
    AnalysedType::Flags(TypeFlags {
        names: vec![
            "FeatureX".to_string(),
            "FeatureY".to_string(),
            "FeatureZ".to_string(),
        ],
    })
}

// Variant
pub(crate) fn variant_type() -> AnalysedType {
    AnalysedType::Variant(TypeVariant {
        cases: vec![
            NameOptionTypePair {
                name: "case-none".to_string(),
                typ: None,
            },
            NameOptionTypePair {
                name: "case-str".to_string(),
                typ: Some(AnalysedType::Str(TypeStr)), // Variant case for String
            },
            NameOptionTypePair {
                name: "case-u64".to_string(),
                typ: Some(AnalysedType::U64(TypeU64)), // Variant case for u64
            },
            NameOptionTypePair {
                name: "case-s32".to_string(),
                typ: Some(AnalysedType::S32(TypeS32)), // Variant case for i32
            },
            NameOptionTypePair {
                name: "case-f32".to_string(),
                typ: Some(AnalysedType::F32(TypeF32)), // Variant case for f32
            },
            NameOptionTypePair {
                name: "case-f64".to_string(),
                typ: Some(AnalysedType::F64(TypeF64)), // Variant case for f64
            },
            NameOptionTypePair {
                name: "case-bool".to_string(),
                typ: Some(AnalysedType::Bool(TypeBool)), // Variant case for bool
            },
            NameOptionTypePair {
                name: "case-chr".to_string(),
                typ: Some(AnalysedType::Chr(TypeChr)), // Variant case for char
            },
            NameOptionTypePair {
                name: "case-list".to_string(),
                typ: Some(AnalysedType::List(TypeList {
                    // Variant case for List
                    inner: Box::new(AnalysedType::S16(TypeS16)),
                })),
            },
            NameOptionTypePair {
                name: "case-option".to_string(),
                typ: Some(AnalysedType::Option(TypeOption {
                    // Variant case for Option
                    inner: Box::new(AnalysedType::U16(TypeU16)),
                })),
            },
            NameOptionTypePair {
                name: "case-result".to_string(),
                typ: Some(AnalysedType::Result(TypeResult {
                    // Variant case for Result
                    ok: Some(Box::new(AnalysedType::U8(TypeU8))),
                    err: Some(Box::new(AnalysedType::S8(TypeS8))),
                })),
            },
            NameOptionTypePair {
                name: "case-record".to_string(),
                typ: Some(AnalysedType::Record(TypeRecord {
                    // Variant case for Record
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
                name: "case-tuple".to_string(),
                typ: Some(AnalysedType::Tuple(TypeTuple {
                    // Variant case for Tuple
                    items: vec![AnalysedType::F32(TypeF32), AnalysedType::U32(TypeU32)],
                })),
            },
        ],
    })
}
