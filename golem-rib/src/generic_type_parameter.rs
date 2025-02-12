use std::fmt::Display;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenericTypeParameter {
    pub value: String,
}

impl Display for GenericTypeParameter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.value)
    }
}