use itertools::Itertools;
use std::fmt::Display;

pub struct ValidationContext {
    pub name: &'static str,
    pub value: String,
}

pub enum ValidatedResult<T> {
    Ok(T),
    OkWithWarns(T, Vec<String>),
    WarnsAndErrors(Vec<String>, Vec<String>),
}

impl<T> ValidatedResult<T> {
    pub fn from_value_and_warns(value: T, warns: Vec<String>) -> Self {
        if warns.is_empty() {
            ValidatedResult::Ok(value)
        } else {
            ValidatedResult::OkWithWarns(value, warns)
        }
    }

    pub fn from_result<E: Display>(result: Result<T, E>) -> Self {
        ValidatedResult::Ok(result).flatten()
    }

    pub fn from_error(error: String) -> Self {
        ValidatedResult::WarnsAndErrors(vec![], vec![error])
    }

    pub fn as_ok_ref(&self) -> Option<&T> {
        match self {
            ValidatedResult::Ok(value) => Some(value),
            ValidatedResult::OkWithWarns(value, _) => Some(value),
            ValidatedResult::WarnsAndErrors(_, _) => None,
        }
    }

    pub fn into_product(self) -> (Option<T>, Vec<String>, Vec<String>) {
        match self {
            ValidatedResult::Ok(result) => (Some(result), vec![], vec![]),
            ValidatedResult::OkWithWarns(result, warns) => (Some(result), warns, vec![]),
            ValidatedResult::WarnsAndErrors(warns, errors) => (None, warns, errors),
        }
    }

    pub fn combine<U, V, C>(self, u: ValidatedResult<U>, combine: C) -> ValidatedResult<V>
    where
        C: Fn(T, U) -> V,
    {
        let (t, mut t_warns, mut t_errors) = self.into_product();
        let (u, u_warns, u_errors) = u.into_product();

        let warns = {
            t_warns.extend(u_warns);
            t_warns
        };

        let errors = {
            t_errors.extend(u_errors);
            t_errors
        };

        match (t, u, warns.is_empty()) {
            (Some(t), Some(u), true) => ValidatedResult::Ok(combine(t, u)),
            (Some(t), Some(u), false) => ValidatedResult::OkWithWarns(combine(t, u), warns),
            _ => ValidatedResult::WarnsAndErrors(warns, errors),
        }
    }

    pub fn map<U, F>(self, f: F) -> ValidatedResult<U>
    where
        F: FnOnce(T) -> U,
    {
        match self {
            ValidatedResult::Ok(value) => ValidatedResult::Ok(f(value)),
            ValidatedResult::OkWithWarns(value, warns) => {
                ValidatedResult::OkWithWarns(f(value), warns)
            }
            ValidatedResult::WarnsAndErrors(warns, errors) => {
                ValidatedResult::WarnsAndErrors(warns, errors)
            }
        }
    }

    pub fn and_then<U, F>(self, f: F) -> ValidatedResult<U>
    where
        F: FnOnce(T) -> ValidatedResult<U>,
    {
        match self {
            ValidatedResult::Ok(value) => f(value),
            ValidatedResult::OkWithWarns(value, warns) => {
                ValidatedResult::from_value_and_warns((), warns).combine(f(value), |_, value| value)
            }
            ValidatedResult::WarnsAndErrors(warns, errors) => {
                ValidatedResult::WarnsAndErrors(warns, errors)
            }
        }
    }
}

impl<T, E> ValidatedResult<Result<T, E>> {
    pub fn transpose(self) -> Result<ValidatedResult<T>, E> {
        match self {
            ValidatedResult::Ok(result) => match result {
                Ok(value) => Ok(ValidatedResult::Ok(value)),
                Err(err) => Err(err),
            },
            ValidatedResult::OkWithWarns(result, warns) => match result {
                Ok(value) => Ok(ValidatedResult::OkWithWarns(value, warns)),
                Err(err) => Err(err),
            },
            ValidatedResult::WarnsAndErrors(warns, errors) => {
                Ok(ValidatedResult::WarnsAndErrors(warns, errors))
            }
        }
    }

    pub fn flatten(self) -> ValidatedResult<T>
    where
        E: Display,
    {
        match self {
            ValidatedResult::Ok(value) => match value {
                Ok(value) => ValidatedResult::Ok(value),
                Err(err) => ValidatedResult::WarnsAndErrors(vec![], vec![format!("{}", err)]),
            },
            ValidatedResult::OkWithWarns(value, warns) => match value {
                Ok(value) => ValidatedResult::OkWithWarns(value, warns),
                Err(err) => ValidatedResult::WarnsAndErrors(warns, vec![format!("{}", err)]),
            },
            ValidatedResult::WarnsAndErrors(warns, errors) => {
                ValidatedResult::WarnsAndErrors(warns, errors)
            }
        }
    }
}

// NOTE: Only implemented for Vec and as non-short-circuiting on errors for now
impl<A> FromIterator<ValidatedResult<A>> for ValidatedResult<Vec<A>> {
    fn from_iter<T: IntoIterator<Item = ValidatedResult<A>>>(iter: T) -> Self {
        let mut validation = ValidationBuilder::new();
        let mut elems = Vec::<A>::new();

        for elem in iter {
            match elem {
                ValidatedResult::Ok(elem) => {
                    elems.push(elem);
                }
                ValidatedResult::OkWithWarns(elem, warns) => {
                    elems.push(elem);
                    warns.into_iter().for_each(|warn| validation.add_warn(warn));
                }
                ValidatedResult::WarnsAndErrors(warns, errors) => {
                    warns.into_iter().for_each(|warn| validation.add_warn(warn));
                    errors
                        .into_iter()
                        .for_each(|error| validation.add_error(error));
                }
            }
        }

        validation.build(elems)
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
        self.errors.push(format!("{}{}", error, self.context(),));
    }

    pub fn add_warn(&mut self, warn: String) {
        self.warns.push(format!("{}{}", warn, self.context(),));
    }

    pub fn add_errors<T, C, F>(&mut self, elems: C, context_and_error: F)
    where
        C: IntoIterator<Item = T>,
        F: Fn(T) -> Option<(Vec<(&'static str, String)>, String)>,
    {
        self.add(elems, context_and_error, Self::add_error);
    }

    pub fn add_warns<T, C, F>(&mut self, elems: C, context_and_error: F)
    where
        C: IntoIterator<Item = T>,
        F: Fn(T) -> Option<(Vec<(&'static str, String)>, String)>,
    {
        self.add(elems, context_and_error, Self::add_warn);
    }

    pub fn add<T, C, CE, A>(&mut self, elems: C, context_and_error: CE, add: A)
    where
        C: IntoIterator<Item = T>,
        CE: Fn(T) -> Option<(Vec<(&'static str, String)>, String)>,
        A: Fn(&mut Self, String),
    {
        for elem in elems {
            if let Some((context, error)) = context_and_error(elem) {
                let context_count = context.len();
                if !context.is_empty() {
                    context
                        .into_iter()
                        .for_each(|(name, value)| self.push_context(name, value))
                }

                add(self, error);

                for _ in 0..context_count {
                    self.pop_context()
                }
            }
        }
    }

    pub fn has_any_errors(&self) -> bool {
        !self.errors.is_empty()
    }

    pub fn build<T>(self, value: T) -> ValidatedResult<T> {
        if self.errors.is_empty() {
            ValidatedResult::from_value_and_warns(value, self.warns)
        } else {
            ValidatedResult::WarnsAndErrors(self.warns, self.errors)
        }
    }

    fn context(&self) -> String {
        if self.context.is_empty() {
            "".to_string()
        } else {
            format!(
                ", {}",
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
