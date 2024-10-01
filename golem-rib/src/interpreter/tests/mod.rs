#[cfg(test)]
mod comprehensive_test {
    use crate::{compiler, Expr};
    use golem_wasm_ast::analysis::{AnalysedType, NameTypePair, TypeRecord, TypeStr, TypeU64};
    use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;

    #[tokio::test]
    async fn test_interpreter_complex_rib() {
        let expr = r#"

              let str1: str = request.body.name;
              let str2: str = request.headers.name;
              let str3: str = request.path.name;

              let unused = function-unit-response(str1);

              let str_output = function-no-arg();

              let unused = function-no-arg-unit();

              let str_response = function-str-response(str_output);

              let number_response = function-number-response(str1);

              let option_str_response = function-option-str-response(str2);

              let option_number_response = function-option-number-response(str1);

              let option_option_response = function-option-option-response(str1);

              let option_variant_response = function-option-variant-response(str1);

              let option_enum_response = function-option-enum-response(str1);

              let option_tuple_response = function-option-tuple-response(str1);

              let option_record_response = function-option-record-response(str1);

              let option_list_response = function-option-list-response(str1);

              let list_number_response = function-list-number-response(str1);

              let list_str_response = function-list-str-response(str1);

              let list_option_response = function-list-option-response(str1);

              let list_list_response = function-list-list-response(str1);

              let list_variant_response = function-list-variant-response(str1);

              let list_enum_response = function-list-enum-response(str1);

              let list_tuple_response = function-list-tuple-response(str1);

              let list_record_response = function-list-record-response(str1);

              let result_of_str_response = function-result-str-response(str1);

              let result_of_number_response = function-result-number-response(str1);

              let result_of_variant_response = function-result-variant-response(str1);

              let result_of_enum_response = function-result-enum-response(str1);

              let result_of_tuple_response = function-result-tuple-response(str1);

              let result_of_flag_response = function-result-flag-response(str1);

              let result_of_record_response = function-result-record-response(str1);

              let result_of_list_response = function-result-list-response(str1);

              let tuple_response = function-tuple-response(str1);

              let enum_response = function-enum-response(str1);

              let flag_response = function-flag-response(str1);

              let variant_response = function-variant-response(str1);

              let str_response_processed = str_response == "foo";

              let number_response_processed = if number_response == 42u64 then "foo" else "bar";

              let option_str_response_processed = match option_str_response {
                some(text) => text,
                none => "not found"
              };

              let option_number_response_processed = match option_number_response {
                some(number) => number,
                none => 0
              };

              let option_option_response_processed = match option_option_response {
                 some(some(x)) => x,
                 none => "not found"
              };

              let option_variant_response_processed = match option_variant_response {
                 some(case-str(_)) => "found",
                 _ => "not found"
              };

              let option_enum_response_processed = match option_enum_response {
                 some(enum-a) => "a",
                 some(enum-b) => "b",
                 _ => "not found"
              };

              let option_tuple_response_processed = match option_tuple_response {
                    some((text, _, _, _, _, _, _, _, _, _, _, _)) => text,
                     _ => "not found"
                };

              let option_record_response_processed = match option_record_response {
                  some({data-body: {list-of-str : _}}) => "found list",
                   _ => "not found"
              };

              let option_list_response_processed = match option_list_response {
                    some([_]) => "found list",
                     _ => "not found"
                };

              let list_number_response_processed = match list_number_response {
                    [number] => if number > 10u64 then "greater" else "lesser",
                     _ => "not found"
                };

              let list_str_response_processed = match list_str_response {
                [text] => text,
                _ => "not found"
              };


              let list_option_response_processed = match list_option_response {
                [some(text)] => text,
                _ => "not found"
              };


              let list_list_response_processed = match list_list_response {
                 [[text]] => text,
                  _ => "not found"
              };


              let list_variant_response_processed = match list_variant_response {
                 [case-str(text)] => text,
                  _ => "not found"
              };

              let list_enum_response_processed = match list_enum_response {
                [enum-a] => "a",
                [enum-b] => "b",
                _ => "not found"
              };

              let list_tuple_response_processed = match list_tuple_response {
                [(text, _, _, _, _, _, _, _, _, _, _, _)] => text,
                _ => "not found"
              };

              let list_record_response_processed = match list_record_response {
                [{data-body: {list-of-str : [text]}}] => text,
                _ => "not found"
              };

              let result_of_str_response_processed = match result_of_str_response {
                ok(text) => text,
                err(msg) => "not found"
              };

              let result_of_number_response_processed = match result_of_number_response {
                ok(number) => number,
                err(msg) => 0
              };

              let result_of_variant_response_processed = match result_of_variant_response {
                ok(case-str(_)) => "found",
                err(msg) => "not found"
              };

              let result_of_enum_response_processed = match result_of_enum_response {
                ok(enum-a) => "a",
                ok(enum-b) => "b",
                ok(enum-c) => "c",
                err(msg) => "not found"
              };

              let result_of_tuple_response_processed = match result_of_tuple_response {
                ok((text, _, _, _, _, _, _, _, _, _, _, _)) => text,
                err(msg) => "not found"
              };

              let result_of_flag_response_processed = match result_of_flag_response {
                ok({featurex, featurey, featurez}) => "found all flags",
                ok({featurex}) => "found x",
                ok({featurey}) => "found x",
                ok({featurex, featurey}) => "found x and y",
                _ => "not found"
               };

              let result_of_record_response_processed = match result_of_record_response {
                 ok({data-body: {list-of-str : _}}) => "found list",
                 err(msg) => "not found"
               };

               let tuple_response_processed = match tuple_response {
                 (_, _, _, _, _, _, _, _, _, _, case-hello(a), _) => "${a}"
               };

               let enum_response_processed = match enum_response {
                 enum-a => "a",
                 enum-b => "b",
                 enum-c => "c",
                 _ => "not found"
               };

               let variant_response_processed = match variant_response {
                 case-str(text) => text,
                 _ => "not found"
               };

              {
                 a : option_str_response_processed,
                 b: option_number_response_processed,
                 c: option_option_response_processed,
                 d: option_variant_response_processed,
                 e: option_enum_response_processed,
                 f: option_tuple_response_processed,
                 g: option_record_response_processed,
                 h: option_list_response_processed,
                 i: list_number_response_processed,
                 j: list_str_response_processed,
                 k: list_option_response_processed,
                 l: list_list_response_processed,
                 m: list_variant_response_processed,
                 n: list_enum_response_processed,
                 o: list_tuple_response_processed,
                 p: list_record_response_processed,
                 q: result_of_str_response_processed,
                 r: result_of_number_response_processed,
                 s: result_of_variant_response_processed,
                 t: result_of_enum_response_processed,
                 u: result_of_tuple_response_processed,
                 v: result_of_flag_response_processed,
                 w: result_of_record_response_processed,
                 x: tuple_response_processed,
                 y: enum_response_processed,
                 z: variant_response_processed
              }
        "#;

        let expr = Expr::from_text(expr).unwrap();

        let compiled_expr = compiler::compile(&expr, &component_metadata::component_metadata())
            .unwrap()
            .byte_code;

        let mut rib_executor = mock_interpreter::interpreter();
        let result = rib_executor.run(compiled_expr).await.unwrap();

        assert_eq!(result.get_val().unwrap(), expected_type_annotated_value());
    }

    fn expected_type_annotated_value() -> TypeAnnotatedValue {
        let wasm_wave_str = "{a: \"foo\", b: 42, c: \"foo\", d: \"found\", e: \"a\", f: \"foo\", g: \"found list\", h: \"found list\", i: \"greater\", j: \"foo\", k: \"foo\", l: \"foo\", m: \"foo\", n: \"a\", o: \"foo\", p: \"foo\", q: \"foo\", r: 42, s: \"found\", t: \"a\", u: \"foo\", v: \"found x\", w: \"found list\", x: \"42\", y: \"a\", z: \"foo\"}";

        test_utils::get_type_annotated_value(&expected_analysed_type(), wasm_wave_str)
    }

    fn expected_analysed_type() -> AnalysedType {
        AnalysedType::Record(TypeRecord {
            fields: vec![
                NameTypePair {
                    name: "a".to_string(),
                    typ: AnalysedType::Str(TypeStr),
                },
                NameTypePair {
                    name: "b".to_string(),
                    typ: AnalysedType::U64(TypeU64),
                },
                NameTypePair {
                    name: "c".to_string(),
                    typ: AnalysedType::Str(TypeStr),
                },
                NameTypePair {
                    name: "d".to_string(),
                    typ: AnalysedType::Str(TypeStr),
                },
                NameTypePair {
                    name: "e".to_string(),
                    typ: AnalysedType::Str(TypeStr),
                },
                NameTypePair {
                    name: "f".to_string(),
                    typ: AnalysedType::Str(TypeStr),
                },
                NameTypePair {
                    name: "g".to_string(),
                    typ: AnalysedType::Str(TypeStr),
                },
                NameTypePair {
                    name: "h".to_string(),
                    typ: AnalysedType::Str(TypeStr),
                },
                NameTypePair {
                    name: "i".to_string(),
                    typ: AnalysedType::Str(TypeStr),
                },
                NameTypePair {
                    name: "j".to_string(),
                    typ: AnalysedType::Str(TypeStr),
                },
                NameTypePair {
                    name: "k".to_string(),
                    typ: AnalysedType::Str(TypeStr),
                },
                NameTypePair {
                    name: "l".to_string(),
                    typ: AnalysedType::Str(TypeStr),
                },
                NameTypePair {
                    name: "m".to_string(),
                    typ: AnalysedType::Str(TypeStr),
                },
                NameTypePair {
                    name: "n".to_string(),
                    typ: AnalysedType::Str(TypeStr),
                },
                NameTypePair {
                    name: "o".to_string(),
                    typ: AnalysedType::Str(TypeStr),
                },
                NameTypePair {
                    name: "p".to_string(),
                    typ: AnalysedType::Str(TypeStr),
                },
                NameTypePair {
                    name: "q".to_string(),
                    typ: AnalysedType::Str(TypeStr),
                },
                NameTypePair {
                    name: "r".to_string(),
                    typ: AnalysedType::U64(TypeU64),
                },
                NameTypePair {
                    name: "s".to_string(),
                    typ: AnalysedType::Str(TypeStr),
                },
                NameTypePair {
                    name: "t".to_string(),
                    typ: AnalysedType::Str(TypeStr),
                },
                NameTypePair {
                    name: "u".to_string(),
                    typ: AnalysedType::Str(TypeStr),
                },
                NameTypePair {
                    name: "v".to_string(),
                    typ: AnalysedType::Str(TypeStr),
                },
                NameTypePair {
                    name: "w".to_string(),
                    typ: AnalysedType::Str(TypeStr),
                },
                NameTypePair {
                    name: "x".to_string(),
                    typ: AnalysedType::Str(TypeStr),
                },
                NameTypePair {
                    name: "y".to_string(),
                    typ: AnalysedType::Str(TypeStr),
                },
                NameTypePair {
                    name: "z".to_string(),
                    typ: AnalysedType::Str(TypeStr),
                },
            ],
        })
    }

    mod component_metadata {
        use crate::interpreter::tests::comprehensive_test::function_metadata;
        use golem_wasm_ast::analysis::AnalysedExport;

        pub(crate) fn component_metadata() -> Vec<AnalysedExport> {
            let mut exports = vec![];
            exports.extend(function_metadata::function_unit_response());
            exports.extend(function_metadata::function_no_arg());
            exports.extend(function_metadata::function_no_arg_unit());
            exports.extend(function_metadata::function_str_response());
            exports.extend(function_metadata::function_number_response());
            exports.extend(function_metadata::function_option_of_str_response());
            exports.extend(function_metadata::function_option_of_number_response());
            exports.extend(function_metadata::function_option_of_option_response());
            exports.extend(function_metadata::function_option_of_variant_response());
            exports.extend(function_metadata::function_option_of_enum_response());
            exports.extend(function_metadata::function_option_of_tuple_response());
            exports.extend(function_metadata::function_option_of_record_response());
            exports.extend(function_metadata::function_option_of_list_response());
            exports.extend(function_metadata::function_list_of_number_response());
            exports.extend(function_metadata::function_list_of_str_response());
            exports.extend(function_metadata::function_list_of_option_response());
            exports.extend(function_metadata::function_list_of_list_response());
            exports.extend(function_metadata::function_list_of_variant_response());
            exports.extend(function_metadata::function_list_of_enum_response());
            exports.extend(function_metadata::function_list_of_tuple_response());
            exports.extend(function_metadata::function_list_of_record_response());
            exports.extend(function_metadata::function_result_of_str_response());
            exports.extend(function_metadata::function_result_of_number_response());
            exports.extend(function_metadata::function_result_of_option_response());
            exports.extend(function_metadata::function_result_of_variant_response());
            exports.extend(function_metadata::function_result_of_enum_response());
            exports.extend(function_metadata::function_result_of_tuple_response());
            exports.extend(function_metadata::function_result_of_flag_response());
            exports.extend(function_metadata::function_result_of_record_response());
            exports.extend(function_metadata::function_result_of_list_response());
            exports.extend(function_metadata::function_tuple_response());
            exports.extend(function_metadata::function_enum_response());
            exports.extend(function_metadata::function_flag_response());
            exports.extend(function_metadata::function_variant_response());
            exports.extend(function_metadata::function_record_response());
            exports.extend(function_metadata::function_all_inputs());

            exports
        }
    }

    mod function_metadata {
        use crate::interpreter::tests::comprehensive_test::{data_types, test_utils};
        use golem_wasm_ast::analysis::AnalysedExport;

        pub(crate) fn function_unit_response() -> Vec<AnalysedExport> {
            test_utils::get_function_component_metadata(
                "function-unit-response",
                vec![data_types::str_type()],
                None,
            )
        }

        pub(crate) fn function_no_arg() -> Vec<AnalysedExport> {
            test_utils::get_function_component_metadata(
                "function-no-arg",
                vec![],
                Some(data_types::str_type()),
            )
        }

        pub(crate) fn function_no_arg_unit() -> Vec<AnalysedExport> {
            test_utils::get_function_component_metadata("function-no-arg-unit", vec![], None)
        }

        pub(crate) fn function_str_response() -> Vec<AnalysedExport> {
            test_utils::get_function_component_metadata(
                "function-str-response",
                vec![data_types::str_type()],
                Some(data_types::str_type()),
            )
        }

        pub(crate) fn function_number_response() -> Vec<AnalysedExport> {
            test_utils::get_function_component_metadata(
                "function-number-response",
                vec![data_types::str_type()],
                Some(data_types::number_type()),
            )
        }

        pub(crate) fn function_option_of_str_response() -> Vec<AnalysedExport> {
            test_utils::get_function_component_metadata(
                "function-option-str-response",
                vec![data_types::str_type()],
                Some(data_types::option_of_str_type()),
            )
        }

        pub(crate) fn function_option_of_number_response() -> Vec<AnalysedExport> {
            test_utils::get_function_component_metadata(
                "function-option-number-response",
                vec![data_types::str_type()],
                Some(data_types::option_of_number_type()),
            )
        }

        pub(crate) fn function_option_of_option_response() -> Vec<AnalysedExport> {
            test_utils::get_function_component_metadata(
                "function-option-option-response",
                vec![data_types::str_type()],
                Some(data_types::option_of_option_type()),
            )
        }

        pub(crate) fn function_option_of_variant_response() -> Vec<AnalysedExport> {
            test_utils::get_function_component_metadata(
                "function-option-variant-response",
                vec![data_types::str_type()],
                Some(data_types::option_of_variant_type()),
            )
        }

        pub(crate) fn function_option_of_enum_response() -> Vec<AnalysedExport> {
            test_utils::get_function_component_metadata(
                "function-option-enum-response",
                vec![data_types::str_type()],
                Some(data_types::option_of_enum_type()),
            )
        }

        pub(crate) fn function_option_of_tuple_response() -> Vec<AnalysedExport> {
            test_utils::get_function_component_metadata(
                "function-option-tuple-response",
                vec![data_types::str_type()],
                Some(data_types::option_of_tuple()),
            )
        }

        pub(crate) fn function_option_of_record_response() -> Vec<AnalysedExport> {
            test_utils::get_function_component_metadata(
                "function-option-record-response",
                vec![data_types::str_type()],
                Some(data_types::option_of_record_type()),
            )
        }

        pub(crate) fn function_option_of_list_response() -> Vec<AnalysedExport> {
            test_utils::get_function_component_metadata(
                "function-option-list-response",
                vec![data_types::str_type()],
                Some(data_types::option_of_list()),
            )
        }

        pub(crate) fn function_list_of_number_response() -> Vec<AnalysedExport> {
            test_utils::get_function_component_metadata(
                "function-list-number-response",
                vec![data_types::str_type()],
                Some(data_types::list_of_number_type_type()),
            )
        }

        pub(crate) fn function_list_of_str_response() -> Vec<AnalysedExport> {
            test_utils::get_function_component_metadata(
                "function-list-str-response",
                vec![data_types::str_type()],
                Some(data_types::list_of_str_type()),
            )
        }

        pub(crate) fn function_list_of_option_response() -> Vec<AnalysedExport> {
            test_utils::get_function_component_metadata(
                "function-list-option-response",
                vec![data_types::str_type()],
                Some(data_types::list_of_option_type()),
            )
        }

        pub(crate) fn function_list_of_list_response() -> Vec<AnalysedExport> {
            test_utils::get_function_component_metadata(
                "function-list-list-response",
                vec![data_types::str_type()],
                Some(data_types::list_of_list_type()),
            )
        }

        pub(crate) fn function_list_of_variant_response() -> Vec<AnalysedExport> {
            test_utils::get_function_component_metadata(
                "function-list-variant-response",
                vec![data_types::str_type()],
                Some(data_types::list_of_variant_type()),
            )
        }

        pub(crate) fn function_list_of_enum_response() -> Vec<AnalysedExport> {
            test_utils::get_function_component_metadata(
                "function-list-enum-response",
                vec![data_types::str_type()],
                Some(data_types::list_of_enum_type()),
            )
        }

        pub(crate) fn function_list_of_tuple_response() -> Vec<AnalysedExport> {
            test_utils::get_function_component_metadata(
                "function-list-tuple-response",
                vec![data_types::str_type()],
                Some(data_types::list_of_tuple()),
            )
        }

        pub(crate) fn function_list_of_record_response() -> Vec<AnalysedExport> {
            test_utils::get_function_component_metadata(
                "function-list-record-response",
                vec![data_types::str_type()],
                Some(data_types::list_of_record_type()),
            )
        }

        pub(crate) fn function_result_of_str_response() -> Vec<AnalysedExport> {
            test_utils::get_function_component_metadata(
                "function-result-str-response",
                vec![data_types::str_type()],
                Some(data_types::result_of_str_type()),
            )
        }

        pub(crate) fn function_result_of_number_response() -> Vec<AnalysedExport> {
            test_utils::get_function_component_metadata(
                "function-result-number-response",
                vec![data_types::str_type()],
                Some(data_types::result_of_number_type()),
            )
        }

        pub(crate) fn function_result_of_option_response() -> Vec<AnalysedExport> {
            test_utils::get_function_component_metadata(
                "function-result-option-response",
                vec![data_types::str_type()],
                Some(data_types::result_of_option_type()),
            )
        }

        pub(crate) fn function_result_of_variant_response() -> Vec<AnalysedExport> {
            test_utils::get_function_component_metadata(
                "function-result-variant-response",
                vec![data_types::str_type()],
                Some(data_types::result_of_variant_type()),
            )
        }

        pub(crate) fn function_result_of_enum_response() -> Vec<AnalysedExport> {
            test_utils::get_function_component_metadata(
                "function-result-enum-response",
                vec![data_types::str_type()],
                Some(data_types::result_of_enum_type()),
            )
        }

        pub(crate) fn function_result_of_tuple_response() -> Vec<AnalysedExport> {
            test_utils::get_function_component_metadata(
                "function-result-tuple-response",
                vec![data_types::str_type()],
                Some(data_types::result_of_tuple_type()),
            )
        }

        pub(crate) fn function_result_of_flag_response() -> Vec<AnalysedExport> {
            test_utils::get_function_component_metadata(
                "function-result-flag-response",
                vec![data_types::str_type()],
                Some(data_types::result_of_flag_type()),
            )
        }

        pub(crate) fn function_result_of_record_response() -> Vec<AnalysedExport> {
            test_utils::get_function_component_metadata(
                "function-result-record-response",
                vec![data_types::str_type()],
                Some(data_types::result_of_record_type()),
            )
        }

        pub(crate) fn function_result_of_list_response() -> Vec<AnalysedExport> {
            test_utils::get_function_component_metadata(
                "function-result-list-response",
                vec![data_types::str_type()],
                Some(data_types::result_of_list_type()),
            )
        }

        pub(crate) fn function_tuple_response() -> Vec<AnalysedExport> {
            test_utils::get_function_component_metadata(
                "function-tuple-response",
                vec![data_types::str_type()],
                Some(data_types::tuple_type()),
            )
        }

        pub(crate) fn function_enum_response() -> Vec<AnalysedExport> {
            test_utils::get_function_component_metadata(
                "function-enum-response",
                vec![data_types::str_type()],
                Some(data_types::enum_type()),
            )
        }

        pub(crate) fn function_flag_response() -> Vec<AnalysedExport> {
            test_utils::get_function_component_metadata(
                "function-flag-response",
                vec![data_types::str_type()],
                Some(data_types::flag_type()),
            )
        }

        pub(crate) fn function_variant_response() -> Vec<AnalysedExport> {
            test_utils::get_function_component_metadata(
                "function-variant-response",
                vec![data_types::str_type()],
                Some(data_types::variant_type()),
            )
        }

        pub(crate) fn function_record_response() -> Vec<AnalysedExport> {
            test_utils::get_function_component_metadata(
                "function-record-response",
                vec![data_types::str_type()],
                Some(data_types::record_type()),
            )
        }

        pub(crate) fn function_all_inputs() -> Vec<AnalysedExport> {
            test_utils::get_function_component_metadata(
                "function-all-inputs",
                vec![
                    data_types::str_type(),
                    data_types::number_type(),
                    data_types::option_of_str_type(),
                    data_types::option_of_number_type(),
                    data_types::option_of_option_type(),
                    data_types::option_of_variant_type(),
                    data_types::option_of_enum_type(),
                    data_types::option_of_tuple(),
                    data_types::option_of_record_type(),
                    data_types::option_of_list(),
                    data_types::list_of_number_type_type(),
                    data_types::list_of_str_type(),
                    data_types::list_of_option_type(),
                    data_types::list_of_list_type(),
                    data_types::list_of_variant_type(),
                    data_types::list_of_enum_type(),
                    data_types::list_of_tuple(),
                    data_types::list_of_record_type(),
                    data_types::result_of_str_type(),
                    data_types::result_of_number_type(),
                    data_types::result_of_option_type(),
                    data_types::result_of_variant_type(),
                    data_types::result_of_enum_type(),
                    data_types::result_of_tuple_type(),
                    data_types::result_of_flag_type(),
                    data_types::result_of_record_type(),
                    data_types::result_of_list_type(),
                    data_types::tuple_type(),
                    data_types::enum_type(),
                    data_types::flag_type(),
                    data_types::variant_type(),
                    data_types::record_type(),
                ],
                Some(data_types::str_type()),
            )
        }
    }

    mod data_types {
        use crate::interpreter::tests::comprehensive_test::test_utils;
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
                    "string-headers",
                    test_utils::analysed_type_record(vec![(
                        "authorization-string",
                        AnalysedType::Str(TypeStr),
                    )]),
                ),
                (
                    "data-body",
                    test_utils::analysed_type_record(vec![
                        ("str", AnalysedType::Str(TypeStr)),
                        ("list-of-str", list_of_str_type()),
                        ("list-of-option", list_of_option_type()),
                        ("list-of-list", list_of_list_type()),
                        ("list-of-variant", list_of_variant_type()),
                        ("list-of-enum", list_of_enum_type()),
                        ("list-of-tuple", list_of_tuple()),
                        (
                            "list-of-record",
                            AnalysedType::List(TypeList {
                                inner: Box::new(test_utils::analysed_type_record(vec![
                                    ("field-string-one", AnalysedType::Str(TypeStr)),
                                    ("field-string-two", AnalysedType::Str(TypeStr)),
                                ])),
                            }),
                        ),
                        ("option-of-str", option_of_str_type()),
                        ("option-of-option", option_of_option_type()),
                        ("option-of-variant", option_of_variant_type()),
                        ("option-of-enum", option_of_enum_type()),
                        ("option-of-tuple", option_of_tuple()),
                        (
                            "option-of-record",
                            AnalysedType::Option(TypeOption {
                                inner: Box::new(test_utils::analysed_type_record(vec![
                                    ("field-string-one", AnalysedType::Str(TypeStr)),
                                    ("field-string-two", AnalysedType::Str(TypeStr)),
                                ])),
                            }),
                        ),
                        ("option-of-list", option_of_list()),
                        (
                            "nested-record",
                            test_utils::analysed_type_record(vec![
                                ("field-string-one", AnalysedType::Str(TypeStr)),
                                ("field-string-two", AnalysedType::Str(TypeStr)),
                            ]),
                        ),
                        ("variant-data-a", variant_type()),
                        ("variant-data-b", variant_type()),
                        ("variant-data-c", variant_type()),
                        ("variant-data-d", variant_type()),
                        ("variant-data-e", variant_type()),
                        ("variant-data-f", variant_type()),
                        ("enum-data-a", enum_type()),
                        ("enum-data-b", enum_type()),
                        ("enum-data-c", enum_type()),
                        ("flags-data-a", flag_type()),
                        ("flags-data-b", flag_type()),
                        ("flags-data-c", flag_type()),
                        ("result-data-a", result_of_str_type()),
                        ("result-data-b", result_of_number_type()),
                        ("result-data-c", result_of_enum_type()),
                        ("result-data-d", result_of_variant_type()),
                        ("result-data-e", result_of_tuple_type()),
                        ("result-data-f", result_of_option_type()),
                        ("result-data-g", result_of_str_type()),
                        ("result-data-h", result_of_number_type()),
                        ("result-data-i", result_of_enum_type()),
                        ("result-data-j", result_of_variant_type()),
                        ("result-data-k", result_of_tuple_type()),
                        ("result-data-l", result_of_option_type()),
                        ("result-data-m", result_of_flag_type()),
                        ("result-data-n", result_of_flag_type()),
                        ("tuple-data", tuple_type()),
                        ("character-data", AnalysedType::Chr(TypeChr)),
                        ("f64-data", AnalysedType::F64(TypeF64)),
                        ("f32-data", AnalysedType::F32(TypeF32)),
                        ("u64-data", AnalysedType::U64(TypeU64)),
                        ("s64-data", AnalysedType::S64(TypeS64)),
                        ("u32-data", AnalysedType::U32(TypeU32)),
                        ("s32-data", AnalysedType::S32(TypeS32)),
                        ("u16-data", AnalysedType::U16(TypeU16)),
                        ("s16-data", AnalysedType::S16(TypeS16)),
                        ("u8-data", AnalysedType::U8(TypeU8)),
                        ("s8-data", AnalysedType::S8(TypeS8)),
                        ("boolean-data", AnalysedType::Bool(TypeBool)),
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
                    "featurex".to_string(),
                    "featurey".to_string(),
                    "featurez".to_string(),
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
    }

    mod mock_data {
        use crate::interpreter::tests::comprehensive_test::{data_types, test_utils};
        use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;

        pub(crate) fn result_of_str() -> TypeAnnotatedValue {
            test_utils::get_type_annotated_value(&data_types::result_of_str_type(), "ok(\"foo\")")
        }

        pub(crate) fn result_of_number() -> TypeAnnotatedValue {
            test_utils::get_type_annotated_value(&data_types::result_of_number_type(), "ok(42)")
        }

        pub(crate) fn result_of_option() -> TypeAnnotatedValue {
            test_utils::get_type_annotated_value(
                &data_types::result_of_option_type(),
                "ok(some(\"foo\"))",
            )
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
            test_utils::get_type_annotated_value(
                &data_types::result_of_tuple_type(),
                wave_str.as_str(),
            )
        }

        pub(crate) fn result_of_flag() -> TypeAnnotatedValue {
            test_utils::get_type_annotated_value(
                &data_types::result_of_flag_type(),
                "ok({featurex})",
            )
        }

        pub(crate) fn result_of_record() -> TypeAnnotatedValue {
            let record_str = test_utils::convert_type_annotated_value_to_str(&record());
            let wave_str = format!("ok({})", &record_str);
            test_utils::get_type_annotated_value(
                &data_types::result_of_record_type(),
                wave_str.as_str(),
            )
        }

        pub(crate) fn result_of_list() -> TypeAnnotatedValue {
            test_utils::get_type_annotated_value(
                &data_types::result_of_list_type(),
                "ok([\"foo\"])",
            )
        }

        pub(crate) fn list_of_number() -> TypeAnnotatedValue {
            test_utils::get_type_annotated_value(&data_types::list_of_number_type_type(), "[42]")
        }

        pub(crate) fn list_of_str() -> TypeAnnotatedValue {
            test_utils::get_type_annotated_value(&data_types::list_of_str_type(), "[\"foo\"]")
        }

        pub(crate) fn list_of_option() -> TypeAnnotatedValue {
            test_utils::get_type_annotated_value(
                &data_types::list_of_option_type(),
                "[some(\"foo\")]",
            )
        }

        pub(crate) fn list_of_list() -> TypeAnnotatedValue {
            test_utils::get_type_annotated_value(&data_types::list_of_list_type(), "[[\"foo\"]]")
        }

        pub(crate) fn list_of_variant() -> TypeAnnotatedValue {
            test_utils::get_type_annotated_value(
                &data_types::list_of_variant_type(),
                "[case-str(\"foo\")]",
            )
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
            test_utils::get_type_annotated_value(
                &data_types::list_of_record_type(),
                wave_str.as_str(),
            )
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
            test_utils::get_type_annotated_value(
                &data_types::option_of_record_type(),
                wave_str.as_str(),
            )
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
    }

    mod mock_interpreter {
        use crate::interpreter::env::InterpreterEnv;
        use crate::interpreter::stack::InterpreterStack;
        use crate::interpreter::tests::comprehensive_test::{mock_data, test_utils};
        use crate::{Interpreter, RibFunctionInvoke};
        use golem_wasm_ast::analysis::{AnalysedType, TypeStr};
        use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
        use golem_wasm_rpc::protobuf::TypedTuple;
        #[cfg(test)]
        use std::collections::HashMap;
        use std::sync::Arc;

        pub(crate) fn interpreter() -> Interpreter {
            let functions_and_results: Vec<(&str, Option<TypeAnnotatedValue>)> = vec![
                ("function-unit-response", None),
                ("function-no-arg", Some(mock_data::str_data())),
                ("function-no-arg-unit", None),
                ("function-str-response", Some(mock_data::str_data())),
                ("function-number-response", Some(mock_data::number_data())),
                (
                    "function-option-str-response",
                    Some(mock_data::option_of_str()),
                ),
                (
                    "function-option-number-response",
                    Some(mock_data::option_of_number()),
                ),
                (
                    "function-option-option-response",
                    Some(mock_data::option_of_option()),
                ),
                (
                    "function-option-variant-response",
                    Some(mock_data::option_of_variant()),
                ),
                (
                    "function-option-enum-response",
                    Some(mock_data::option_of_enum()),
                ),
                (
                    "function-option-tuple-response",
                    Some(mock_data::option_of_tuple()),
                ),
                (
                    "function-option-record-response",
                    Some(mock_data::option_of_record()),
                ),
                (
                    "function-option-list-response",
                    Some(mock_data::option_of_list()),
                ),
                (
                    "function-list-number-response",
                    Some(mock_data::list_of_number()),
                ),
                ("function-list-str-response", Some(mock_data::list_of_str())),
                (
                    "function-list-option-response",
                    Some(mock_data::list_of_option()),
                ),
                (
                    "function-list-list-response",
                    Some(mock_data::list_of_list()),
                ),
                (
                    "function-list-variant-response",
                    Some(mock_data::list_of_variant()),
                ),
                (
                    "function-list-enum-response",
                    Some(mock_data::list_of_enum()),
                ),
                (
                    "function-list-tuple-response",
                    Some(mock_data::list_of_tuple()),
                ),
                (
                    "function-list-record-response",
                    Some(mock_data::list_of_record()),
                ),
                (
                    "function-result-str-response",
                    Some(mock_data::result_of_str()),
                ),
                (
                    "function-result-number-response",
                    Some(mock_data::result_of_number()),
                ),
                (
                    "function-result-option-response",
                    Some(mock_data::result_of_option()),
                ),
                (
                    "function-result-variant-response",
                    Some(mock_data::result_of_variant()),
                ),
                (
                    "function-result-enum-response",
                    Some(mock_data::result_of_enum()),
                ),
                (
                    "function-result-tuple-response",
                    Some(mock_data::result_of_tuple()),
                ),
                (
                    "function-result-flag-response",
                    Some(mock_data::result_of_flag()),
                ),
                (
                    "function-result-record-response",
                    Some(mock_data::result_of_record()),
                ),
                (
                    "function-result-list-response",
                    Some(mock_data::result_of_list()),
                ),
                ("function-tuple-response", Some(mock_data::tuple())),
                ("function-enum-response", Some(mock_data::enum_data())),
                ("function-flag-response", Some(mock_data::flag())),
                ("function-variant-response", Some(mock_data::variant())),
                ("function-record-response", Some(mock_data::record())),
                ("function-all-inputs", Some(mock_data::str_data())),
            ];

            let functions_and_result: HashMap<FunctionName, Option<TypeAnnotatedValue>> =
                functions_and_results
                    .into_iter()
                    .map(|(name, result)| (FunctionName(name.to_string()), result))
                    .collect();

            let record_input_type = test_utils::analysed_type_record(vec![
                (
                    "headers",
                    test_utils::analysed_type_record(vec![("name", AnalysedType::Str(TypeStr))]),
                ),
                (
                    "body",
                    test_utils::analysed_type_record(vec![("name", AnalysedType::Str(TypeStr))]),
                ),
                (
                    "path",
                    test_utils::analysed_type_record(vec![("name", AnalysedType::Str(TypeStr))]),
                ),
            ]);

            let record_input_value = test_utils::get_type_annotated_value(
                &record_input_type,
                r#" { headers : { name : "foo" }, body : { name : "bar" }, path : { name : "baz" } }"#,
            );

            let mut interpreter_env_input: HashMap<String, TypeAnnotatedValue> = HashMap::new();
            interpreter_env_input.insert("request".to_string(), record_input_value);

            dynamic_test_interpreter(functions_and_result, interpreter_env_input)
        }

        #[derive(Clone, Hash, PartialEq, Eq)]
        struct FunctionName(pub(crate) String);

        fn dynamic_test_interpreter(
            functions_and_result: HashMap<FunctionName, Option<TypeAnnotatedValue>>,
            interpreter_env_input: HashMap<String, TypeAnnotatedValue>,
        ) -> Interpreter {
            Interpreter {
                stack: InterpreterStack::default(),
                env: InterpreterEnv::from(
                    interpreter_env_input,
                    dynamic_worker_invoke(functions_and_result),
                ),
            }
        }

        fn dynamic_worker_invoke(
            functions_and_result: HashMap<FunctionName, Option<TypeAnnotatedValue>>,
        ) -> RibFunctionInvoke {
            let value = functions_and_result.clone();

            Arc::new(move |a, _| {
                Box::pin({
                    let value = value.get(&FunctionName(a)).cloned().flatten();
                    let analysed_type = value.clone().map(|x| AnalysedType::try_from(&x).unwrap());

                    async move {
                        let analysed_type = analysed_type.clone();
                        let value = value.clone();

                        if let Some(value) = value {
                            Ok(TypeAnnotatedValue::Tuple(TypedTuple {
                                typ: vec![golem_wasm_ast::analysis::protobuf::Type::from(
                                    &analysed_type.unwrap(),
                                )],
                                value: vec![golem_wasm_rpc::protobuf::TypeAnnotatedValue {
                                    type_annotated_value: Some(value),
                                }],
                            }))
                        } else {
                            // Representing Unit
                            Ok(TypeAnnotatedValue::Tuple(TypedTuple {
                                typ: vec![],
                                value: vec![],
                            }))
                        }
                    }
                })
            })
        }
    }

    mod test_utils {
        #[cfg(test)]
        use golem_wasm_ast::analysis::*;
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
            let result =
                golem_wasm_rpc::type_annotated_value_from_str(analysed_type, wasm_wave_str);

            match result {
                Ok(value) => value,
                Err(err) => panic!(
                    "Wasm wave syntax error {:?} {} {}",
                    analysed_type, wasm_wave_str, err
                ),
            }
        }

        pub(crate) fn convert_type_annotated_value_to_str(
            type_annotated_value: &TypeAnnotatedValue,
        ) -> String {
            golem_wasm_rpc::type_annotated_value_to_string(type_annotated_value).unwrap()
        }

        pub(crate) fn get_function_component_metadata(
            function_name: &str,
            input_types: Vec<AnalysedType>,
            output: Option<AnalysedType>,
        ) -> Vec<AnalysedExport> {
            let analysed_function_parameters = input_types
                .into_iter()
                .enumerate()
                .map(|(index, typ)| AnalysedFunctionParameter {
                    name: format!("param{}", index),
                    typ,
                })
                .collect();

            let results = if let Some(output) = output {
                vec![AnalysedFunctionResult {
                    name: None,
                    typ: output,
                }]
            } else {
                // Representing Unit
                vec![]
            };

            vec![AnalysedExport::Function(AnalysedFunction {
                name: function_name.to_string(),
                parameters: analysed_function_parameters,
                results,
            })]
        }
    }
}
