use crate::interpreter::comprehensive_test::function_metadata;
#[cfg(test)]
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
