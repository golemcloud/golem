pub(crate) use compiled_golem_worker_binding::*;
use golem_wasm_ast::analysis::AnalysedExport;
pub(crate) use golem_worker_binding::*;
pub(crate) use request_details::*;
use rib::{CompilerOutput, Expr};
pub(crate) use rib_input_value_resolver::*;
pub(crate) use worker_binding_resolver::*;

mod compiled_golem_worker_binding;
mod golem_worker_binding;
mod request_details;
mod rib_input_value_resolver;
mod worker_binding_resolver;

pub fn compile_rib(
    worker_name: &Expr,
    export_metadata: &Vec<AnalysedExport>,
) -> Result<CompilerOutput, String> {
    rib::compile_with_limited_globals(
        worker_name,
        export_metadata,
        Some(vec!["request".to_string()]),
    )
}
