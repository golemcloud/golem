
    use golem_wasm_ast::analysis::AnalysedExport;
    use crate::interpreter::comprehensive_test::{data_types, test_utils};

    pub(crate) fn function_unit_response() -> Vec<AnalysedExport> {
        test_utils::get_function_component_metadata(
            "function-unit-response",
            vec![data_types::str_type()],
            None,
        )
    }

    pub(crate) fn function_no_arg() -> Vec<AnalysedExport> {
        test_utils::get_function_component_metadata("function-no-arg", vec![], Some(data_types::str_type()))
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