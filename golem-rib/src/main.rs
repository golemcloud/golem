use golem_wasm_ast::analysis::{
    AnalysedExport, AnalysedFunction, AnalysedFunctionParameter, AnalysedFunctionResult,
    AnalysedType,
};
use rib::{Expr, FunctionTypeRegistry};

fn main() {
    let expr_str = r#"
              ["thaj"]
            "#;

    let mut expr = Expr::from_text(expr_str).unwrap();

    expr.infer_types(&FunctionTypeRegistry::empty()).unwrap();

    dbg!(expr);
}
