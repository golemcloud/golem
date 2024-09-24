use itertools::Itertools;

pub struct ValidationContext {
    pub name: &'static str,
    pub value: String,
}

pub enum ValidationResult {
    Ok,
    Warns(Vec<String>),
    WarnsAndErrors(Vec<String>),
}

impl ValidationResult {
    pub fn merge(self, other: Self) -> Self {
        match self {
            ValidationResult::Ok => self,
            ValidationResult::Warns(warns) => match other {
                ValidationResult::Ok => ValidationResult::Ok,
                ValidationResult::Warns(other_warns) => {
                    ValidationResult::Warns(concat_vec(warns, other_warns))
                }
                ValidationResult::WarnsAndErrors(other_warns_and_errors) => {
                    ValidationResult::WarnsAndErrors(concat_vec(warns, other_warns_and_errors))
                }
            },
            ValidationResult::WarnsAndErrors(warns_and_errors) => match other {
                ValidationResult::Ok => ValidationResult::WarnsAndErrors(warns_and_errors),
                ValidationResult::Warns(other_warns) => {
                    ValidationResult::WarnsAndErrors(concat_vec(warns_and_errors, other_warns))
                }
                ValidationResult::WarnsAndErrors(other_warns_and_errors) => {
                    ValidationResult::WarnsAndErrors(concat_vec(
                        warns_and_errors,
                        other_warns_and_errors,
                    ))
                }
            },
        }
    }
}

pub struct ValidationBuilder {
    context: Vec<ValidationContext>,
    warns: Vec<String>,
    errors: Vec<String>,
}

impl ValidationBuilder {
    pub fn new() -> Self {
        Self {
            context: vec![],
            warns: vec![],
            errors: vec![],
        }
    }

    pub fn push_context(&mut self, name: &'static str, value: String) {
        self.context.push(ValidationContext { name, value })
    }

    pub fn pop_context(&mut self) {
        _ = self.context.pop();
    }

    pub fn add_error(&mut self, error: String) {
        self.errors
            .push(format!("Error: {}{}", error, self.context()));
    }

    pub fn add_warn(&mut self, error: String) {
        self.warns
            .push(format!("Warning: {}{}", error, self.context(),));
    }

    pub fn has_any_errors(&self) -> bool {
        !self.errors.is_empty()
    }

    pub fn build(self) -> ValidationResult {
        if self.errors.is_empty() {
            if self.warns.is_empty() {
                ValidationResult::Ok
            } else {
                ValidationResult::Warns(self.warns)
            }
        } else {
            ValidationResult::WarnsAndErrors(concat_vec(self.warns, self.errors))
        }
    }

    fn context(&self) -> String {
        if self.context.is_empty() {
            "".to_string()
        } else {
            format!(
                " [{}]",
                self.context
                    .iter()
                    .map(|c| format!("{}: {}", c.name, c.value))
                    .join(", ")
            )
        }
    }
}

impl Default for ValidationBuilder {
    fn default() -> Self {
        Self::new()
    }
}

fn concat_vec(a: Vec<String>, b: Vec<String>) -> Vec<String> {
    let mut merged = Vec::<String>::new();
    merged.extend(a);
    merged.extend(b);
    merged
}
