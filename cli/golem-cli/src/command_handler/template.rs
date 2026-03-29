// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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

use std::collections::{HashMap, HashSet};

/// Renders minijinja templates using the host process's environment variables as context.
pub struct EnvVarRenderer {
    proc_env_map: HashMap<String, String>,
    proc_env_vars: minijinja::value::Value,
    minijinja_env: minijinja::Environment<'static>,
}

impl EnvVarRenderer {
    pub fn new() -> Self {
        let proc_env_map: HashMap<String, String> = std::env::vars().collect();
        let proc_env_vars = minijinja::value::Value::from(proc_env_map.clone());

        let minijinja_env = {
            let mut env = minijinja::Environment::new();
            env.set_undefined_behavior(minijinja::UndefinedBehavior::Strict);
            env
        };

        Self {
            proc_env_map,
            proc_env_vars,
            minijinja_env,
        }
    }

    /// Renders a single string template, substituting `{{ VAR }}` with host env vars.
    pub fn render_str(&self, template: &str) -> Result<String, minijinja::Error> {
        self.minijinja_env
            .render_str(template, &self.proc_env_vars)
    }

    /// Recursively renders all string values in a JSON value.
    pub fn render_json_value(
        &self,
        value: &serde_json::Value,
    ) -> Result<serde_json::Value, minijinja::Error> {
        match value {
            serde_json::Value::String(s) => {
                let resolved = self.render_str(s)?;
                Ok(serde_json::Value::String(resolved))
            }
            serde_json::Value::Array(arr) => {
                let resolved: Result<Vec<_>, _> =
                    arr.iter().map(|v| self.render_json_value(v)).collect();
                Ok(serde_json::Value::Array(resolved?))
            }
            serde_json::Value::Object(obj) => {
                let resolved: Result<serde_json::Map<String, serde_json::Value>, _> = obj
                    .iter()
                    .map(|(k, v)| self.render_json_value(v).map(|rv| (k.clone(), rv)))
                    .collect();
                Ok(serde_json::Value::Object(resolved?))
            }
            other => Ok(other.clone()),
        }
    }

    /// Returns the names of environment variables referenced in the template
    /// that are not present in the host environment.
    pub fn missing_env_vars(
        &self,
        template: &str,
        err: &minijinja::Error,
    ) -> Vec<String> {
        fn is_known_var(
            var: &str,
            env_vars: &HashMap<String, String>,
            global_vars: &HashSet<String>,
        ) -> bool {
            if env_vars.contains_key(var) || global_vars.contains(var) {
                return true;
            }

            if let Some((root, _)) = var.split_once('.') {
                return env_vars.contains_key(root) || global_vars.contains(root);
            }

            false
        }

        if err.kind() != minijinja::ErrorKind::UndefinedError {
            return Vec::new();
        }

        let Ok(template) = self.minijinja_env.template_from_str(template) else {
            return Vec::new();
        };

        let global_vars: HashSet<String> = self
            .minijinja_env
            .globals()
            .map(|(name, _)| name.to_string())
            .collect();

        let mut missing: Vec<String> = template
            .undeclared_variables(true)
            .into_iter()
            .filter(|var| !is_known_var(var, &self.proc_env_map, &global_vars))
            .collect();

        missing.sort();
        missing
    }
}
