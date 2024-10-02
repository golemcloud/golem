use golem_wasm_ast::analysis::AnalysedExport;
use rib::{CompilerOutput, Expr};

// A wrapper service over original Rib Compiler concerning
// the details of the worker bridge.
pub trait WorkerServiceRibCompiler {
    fn compile(rib: &Expr, export_metadata: &[AnalysedExport]) -> Result<CompilerOutput, String>;
}

pub struct DefaultRibCompiler;

impl WorkerServiceRibCompiler for DefaultRibCompiler {
    fn compile(rib: &Expr, export_metadata: &[AnalysedExport]) -> Result<CompilerOutput, String> {
        rib::compile_with_limited_globals(
            rib,
            &export_metadata.to_vec(),
            Some(vec!["request".to_string()]),
        )
    }
}
