use crate::InferredType;

pub type UnificationResult = Result<Unified, String>;
pub struct Unified(pub InferredType);

impl Unified {
    pub fn inferred_type(&self) -> InferredType {
        self.0.clone()
    }
}
