pub use byte_code::*;
use golem_wasm_ast::analysis::{AnalysedExport, AnalysedType, TypeStr};
pub use ir::*;
pub use type_with_unit::*;

use crate::type_registry::FunctionTypeRegistry;
use crate::{Expr, RibInputTypeInfo};
use golem_api_grpc::proto::golem::rib::CompilerOutput as ProtoCompilerOutput;

mod byte_code;
mod desugar;
mod ir;
mod type_with_unit;

pub fn compile(
    expr: &Expr,
    export_metadata: &Vec<AnalysedExport>,
) -> Result<CompilerOutput, String> {
    let type_registry = FunctionTypeRegistry::from_export_metadata(export_metadata);
    let mut expr_cloned = expr.clone();
    expr_cloned
        .infer_types(&type_registry)
        .map_err(|e| e.join("\n"))?;

    // Inferring input is not done properly, however, worse case is run-time error asking user to pass these info
    let rib_input =
        RibInputTypeInfo::from_expr(&mut expr_cloned).unwrap_or(RibInputTypeInfo::empty());

    let rib_byte_code = RibByteCode::from_expr(expr_cloned)?;

    Ok(CompilerOutput {
        byte_code: rib_byte_code,
        rib_input,
    })
}

// A pure expression doesn't require any metadata as it won't call any worker functions
// And this results in possibility of unknown types in the input requirement, and the compiler
// choose to fall back to an AnalysedType::Str
// example: worker-name: "request.path.user.id". It implies input has to be a request of type
// record having field path, which itself should a record having field user, which is a record
// having field id, which is really unknown. At this point compiler decides it has to be a string
// This is mainly used for 1 liner expressions like worker-name, idempotency-key etc, which implies
// there is no need for fields like `id` to be a record, or other complex types.
// If not Str, it is configurable with an optional alternative_type but it should be ideally literal
pub fn compile_pure(
    expr: &Expr,
    export_metadata: &Vec<AnalysedExport>,
    alternative_type: Option<AnalysedType>,
) -> Result<CompilerOutput, String> {
    let type_registry = FunctionTypeRegistry::from_export_metadata(export_metadata);

    let mut expr_cloned = expr.clone();

    expr_cloned
        .infer_types(&type_registry)
        .map_err(|e| e.join("\n"))?;

    let rib_input = RibInputTypeInfo::from_pure_expr(
        &mut expr_cloned,
        alternative_type.unwrap_or(AnalysedType::Str(TypeStr)),
    );
    let rib_byte_code = RibByteCode::from_expr(expr_cloned)?;

    Ok(CompilerOutput {
        byte_code: rib_byte_code,
        rib_input,
    })
}

#[derive(Debug, Clone)]
pub struct CompilerOutput {
    pub byte_code: RibByteCode,
    pub rib_input: RibInputTypeInfo,
}

impl TryFrom<ProtoCompilerOutput> for CompilerOutput {
    type Error = String;

    fn try_from(value: ProtoCompilerOutput) -> Result<Self, Self::Error> {
        let proto_rib_input = value.rib_input.ok_or("Missing rib_input")?;
        let proto_byte_code = value.byte_code.ok_or("Missing byte_code")?;
        let rib_input = RibInputTypeInfo::try_from(proto_rib_input)?;
        let byte_code = RibByteCode::try_from(proto_byte_code)?;

        Ok(CompilerOutput {
            byte_code,
            rib_input,
        })
    }
}

impl From<CompilerOutput> for ProtoCompilerOutput {
    fn from(value: CompilerOutput) -> Self {
        ProtoCompilerOutput {
            byte_code: Some(golem_api_grpc::proto::golem::rib::RibByteCode::from(
                value.byte_code,
            )),
            rib_input: Some(golem_api_grpc::proto::golem::rib::RibInputType::from(
                value.rib_input,
            )),
        }
    }
}
