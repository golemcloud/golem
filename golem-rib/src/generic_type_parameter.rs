use std::fmt::Display;

#[derive(Debug, Hash, Clone, PartialEq, Eq, Ord, PartialOrd)]
pub struct GenericTypeParameter {
    pub value: String,
}

impl Display for GenericTypeParameter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.value)
    }
}
