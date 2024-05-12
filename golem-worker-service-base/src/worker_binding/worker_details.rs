use golem_wasm_ast::analysis::AnalysedType;
use golem_wasm_rpc::TypeAnnotatedValue;
use golem_common::model::ComponentId;

pub struct  WorkerDetails {
    worker_name: String,
    component_id: ComponentId,
    function_name: String
}

impl WorkerDetails {
    pub fn to_type_annotated_value(self) -> TypeAnnotatedValue {
        TypeAnnotatedValue::Record {
            typ: vec![
                ("name".to_string(), AnalysedType::Str),
                ("component_id".to_string(), AnalysedType::Str),
            ],
            value: vec![
                ("name".to_string(), TypeAnnotatedValue::Str(self.worker_name)),
                ("component_id".to_string(), TypeAnnotatedValue::Str(self.component_id.0.to_string())),
            ],
        }
    }
}