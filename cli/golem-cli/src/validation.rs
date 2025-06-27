// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::log::LogColorize;
use itertools::Itertools;
use std::fmt::Debug;

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

    pub fn from_result<E: Debug>(result: Result<T, E>) -> Self {
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
        C: FnOnce(T, U) -> V,
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

    pub fn inspect<F>(self, f: F) -> Self
    where
        F: FnOnce(&T),
    {
        if let Some(value) = self.as_ok_ref() {
            f(value);
        }
        self
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
        E: Debug,
    {
        match self {
            ValidatedResult::Ok(value) => match value {
                Ok(value) => ValidatedResult::Ok(value),
                Err(err) => ValidatedResult::WarnsAndErrors(vec![], vec![format!("{:?}", err)]),
            },
            ValidatedResult::OkWithWarns(value, warns) => match value {
                Ok(value) => ValidatedResult::OkWithWarns(value, warns),
                Err(err) => ValidatedResult::WarnsAndErrors(warns, vec![format!("{:?}", err)]),
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
    has_any_error_stack: Vec<bool>,
}

impl ValidationBuilder {
    pub fn new() -> Self {
        Self {
            context: vec![],
            warns: vec![],
            errors: vec![],
            has_any_error_stack: vec![],
        }
    }

    pub fn push_context(&mut self, name: &'static str, value: String) {
        self.context.push(ValidationContext { name, value })
    }

    pub fn pop_context(&mut self) {
        _ = self.context.pop();
    }

    pub fn with_context<F: FnOnce(&mut ValidationBuilder)>(
        &mut self,
        context: Vec<(&'static str, String)>,
        f: F,
    ) -> bool {
        let (_, valid) = self.with_context_returning(context, f);
        valid
    }

    pub fn with_context_returning<F, R>(
        &mut self,
        context: Vec<(&'static str, String)>,
        f: F,
    ) -> (R, bool)
    where
        F: FnOnce(&mut ValidationBuilder) -> R,
    {
        let context_count = context.len();

        self.has_any_error_stack.push(false);

        for (name, value) in context {
            self.push_context(name, value);
        }

        let result = f(self);

        for _ in 0..context_count {
            self.pop_context()
        }

        let has_any_errors = self.has_any_error_stack.pop().unwrap();
        if has_any_errors {
            self.has_any_error_stack
                .iter_mut()
                .for_each(|has_any_errors| *has_any_errors = true)
        }

        (result, !has_any_errors)
    }

    fn format(&mut self, message: String) -> String {
        if self.context.is_empty() {
            message
        } else {
            let padding = self
                .context
                .iter()
                .map(|c| c.name.len())
                .max()
                .unwrap_or_default()
                + 1;

            let context = self
                .context
                .iter()
                .map(|c| {
                    format!(
                        "  {:<padding$} {}",
                        format!("{}:", c.name).log_color_highlight(),
                        c.value
                    )
                })
                .join("\n");
            format!(
                "{}\n{}\n{}\n{}",
                "context:".log_color_highlight(),
                context,
                "message:".log_color_highlight(),
                message.lines().map(|l| format!("  {l}")).join("\n")
            )
        }
    }

    pub fn add_error(&mut self, error: String) {
        let error = self.format(error);
        if let Some(has_any_errors) = self.has_any_error_stack.last_mut() {
            *has_any_errors = true;
        }
        self.errors.push(error);
    }

    pub fn add_warn(&mut self, warn: String) {
        let warn = self.format(warn);
        self.warns.push(warn);
    }

    pub fn add_errors<T, E, CE>(&mut self, elems: E, context_and_error: CE)
    where
        E: IntoIterator<Item = T>,
        CE: Fn(T) -> Option<(Vec<(&'static str, String)>, String)>,
    {
        self.add_all(elems, context_and_error, Self::add_error);
    }

    pub fn add_warns<T, E, CE>(&mut self, elems: E, context_and_error: CE)
    where
        E: IntoIterator<Item = T>,
        CE: Fn(T) -> Option<(Vec<(&'static str, String)>, String)>,
    {
        self.add_all(elems, context_and_error, Self::add_warn);
    }

    fn add_all<T, E, CE, A>(&mut self, elems: E, context_and_error: CE, add: A)
    where
        E: IntoIterator<Item = T>,
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
}

impl Default for ValidationBuilder {
    fn default() -> Self {
        Self::new()
    }
}
