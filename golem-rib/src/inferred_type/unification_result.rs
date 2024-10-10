use crate::InferredType;

pub enum UnificationResult {
    Success(InferredType),
    Failed(String)
}

impl UnificationResult {
    pub fn unified(inferred_type: InferredType) -> UnificationResult {
        UnificationResult::Success(inferred_type)
    }
}