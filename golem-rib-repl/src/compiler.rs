use crate::repl_state::ReplState;
use rib::{
    Expr, FunctionDictionary, FunctionTypeRegistry, InferredExpr, InferredType, RibByteCode,
    RibError, VariableId,
};
use std::collections::{HashMap, VecDeque};

pub fn compile_rib_script(
    rib_script: &str,
    repl_state: &mut ReplState,
) -> Result<CompilerOutput, RibError> {
    let expr =
        Expr::from_text(rib_script).map_err(|e| RibError::InvalidRibScript(e.to_string()))?;

    let function_registry =
        FunctionTypeRegistry::from_export_metadata(&repl_state.dependency().metadata);

    let inferred_expr = InferredExpr::from_expr(expr, &function_registry, &vec![])
        .map_err(|e| RibError::InvalidRibScript(e.to_string()))?;

    let instance_variables = fetch_instance_variables(&inferred_expr);

    let identifiers = get_identifiers(&inferred_expr);

    let new_byte_code = RibByteCode::from_expr(&inferred_expr)
        .map_err(|e| RibError::InternalError(e.to_string()))?;

    let byte_code = new_byte_code.diff(repl_state.byte_code());

    repl_state.update_byte_code(new_byte_code);

    Ok(CompilerOutput {
        rib_byte_code: byte_code,
        inferred_expr,
        instance_variables,
        identifiers,
    })
}

pub struct CompilerOutput {
    pub rib_byte_code: RibByteCode,
    pub inferred_expr: InferredExpr,
    pub instance_variables: InstanceVariables,
    pub identifiers: Vec<VariableId>,
}

#[derive(Default, Clone)]
pub struct InstanceVariables {
    pub instance_variables: HashMap<String, FunctionDictionary>,
}

impl InstanceVariables {
    pub fn variable_names(&self) -> Vec<String> {
        self.instance_variables
            .keys()
            .map(|k| k.to_string())
            .collect()
    }
}

pub fn get_identifiers(inferred_expr: &InferredExpr) -> Vec<VariableId> {
    let expr = inferred_expr.get_expr();
    let mut queue = VecDeque::new();

    queue.push_back(expr);

    let mut identifiers = Vec::new();

    while let Some(expr) = queue.pop_back() {
        match expr {
            Expr::Let {
                variable_id, expr, ..
            } => {
                if !identifiers.contains(variable_id) {
                    identifiers.push(variable_id.clone());
                }

                queue.push_back(expr);
            }
            Expr::Identifier { variable_id, .. } => {
                if !identifiers.contains(variable_id) {
                    identifiers.push(variable_id.clone());
                }
            }
            _ => expr.visit_children_bottom_up(&mut queue),
        }
    }

    identifiers
}

pub fn fetch_instance_variables(inferred_expr: &InferredExpr) -> InstanceVariables {
    let expr = inferred_expr.get_expr();
    let mut queue = VecDeque::new();

    queue.push_back(expr);

    let mut instance_variables = HashMap::new();

    while let Some(expr) = queue.pop_front() {
        match expr {
            Expr::Let {
                variable_id, expr, ..
            } => {
                if let InferredType::Instance { instance_type } = expr.inferred_type() {
                    instance_variables.insert(variable_id.name(), instance_type.function_dict());
                }

                queue.push_front(expr)
            }

            _ => expr.visit_children_bottom_up(&mut queue),
        }
    }

    InstanceVariables { instance_variables }
}
