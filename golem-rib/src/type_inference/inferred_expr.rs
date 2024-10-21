use crate::{DynamicParsedFunctionName, Expr, FunctionTypeRegistry, ParsedFunctionName};

pub struct InferredExpr(pub Expr);

impl InferredExpr {
    pub fn from_expr(expr: &Expr, function_type_registry: &FunctionTypeRegistry) -> Result<InferredExpr, String> {
        let mut mutable_expr = expr.clone();
        mutable_expr.infer_types(function_type_registry).map_err(|err| {
            err.join("\n")
        })?;
        Ok(InferredExpr(mutable_expr))
    }
}
