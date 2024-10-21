use golem_wasm_ast::analysis::AnalysedType;
use crate::ParsedFunctionName;

pub struct FunctionCallsInRib {
    pub function_calls: Vec<FunctionCallInRib>
}


// It keeps track of all the identifiers that act as function name,
// its arguments and return types.
// Naturally this will include the resource constructor its arguments
pub struct FunctionCallInRib {
    pub function_name: ParsedFunctionName,
    pub argument_types: Vec<AnalysedType>,
    pub return_types: Vec<AnalysedType>
}

