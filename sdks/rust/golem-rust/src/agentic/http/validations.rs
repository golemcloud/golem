// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::golem_agentic::golem::agent::common::{
    AgentConstructor, DataSchema, ElementSchema, HttpMountDetails, PathSegment,
};
use std::collections::HashSet;

// This validation is applied along with other details in the definition
// unlike basic parsing of the HTTP mount which is done earlier.
pub fn validate_http_mount(
    agent_class_name: &str,
    agent_mount: &HttpMountDetails,
    agent_constructor: &AgentConstructor,
    parameters_for_principal: &HashSet<String>,
) -> Result<(), String> {
    let constructor_input_params = collect_constructor_input_parameter_names(agent_constructor);

    validate_no_catch_all_in_http_mount(agent_class_name, agent_mount)?;
    validate_constructor_params_are_http_safe(agent_class_name, agent_constructor)?;
    validate_mount_variables_are_not_principal(agent_mount, parameters_for_principal)?;
    validate_mount_variables_exist_in_constructor(agent_mount, &constructor_input_params)?;
    validate_constructor_vars_are_satisfied(agent_mount, &constructor_input_params)?;

    Ok(())
}

pub(crate) fn reject_query_param_in_string(path: &str, entity_name: &str) -> Result<(), String> {
    if path.contains('?') {
        return Err(format!("{} cannot contain query parameters", entity_name));
    }

    Ok(())
}

pub(crate) fn reject_empty_string(name: &str, entity_name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err(format!("{} cannot be empty", entity_name));
    }
    Ok(())
}

fn validate_no_catch_all_in_http_mount(
    agent_class_name: &str,
    agent_mount: &HttpMountDetails,
) -> Result<(), String> {
    if let Some(catch_all_variable) =
        agent_mount
            .path_prefix
            .iter()
            .find_map(|segment| match segment {
                PathSegment::RemainingPathVariable(variable) => Some(&variable.variable_name),
                _ => None,
            })
    {
        return Err(format!(
            "HTTP mount for agent '{}' cannot contain catch-all path variable '{}'",
            agent_class_name, catch_all_variable
        ));
    }

    Ok(())
}

fn collect_http_mount_variables(agent_mount: &HttpMountDetails) -> HashSet<String> {
    let mut vars = HashSet::new();

    for segment in &agent_mount.path_prefix {
        if let PathSegment::PathVariable(path_variable) = segment {
            vars.insert(path_variable.variable_name.clone());
        }
    }

    vars
}

fn collect_constructor_input_parameter_names(
    agent_constructor: &AgentConstructor,
) -> HashSet<String> {
    let mut param_names = HashSet::new();

    match &agent_constructor.input_schema {
        DataSchema::Tuple(name_and_schemas) => {
            for (param_name, _param_schema) in name_and_schemas {
                param_names.insert(param_name.clone());
            }
        }
        DataSchema::Multimodal(_) => {}
    }

    param_names
}

fn validate_constructor_params_are_http_safe(
    agent_class_name: &str,
    agent_constructor: &AgentConstructor,
) -> Result<(), String> {
    match &agent_constructor.input_schema {
        DataSchema::Tuple(name_and_schemas) => {
            for (param_name, param_schema) in name_and_schemas {
                if let ElementSchema::UnstructuredBinary(_) = param_schema {
                    return Err(format!(
                            "Agent '{}' constructor parameter '{}' cannot be of type 'UnstructuredBinary' when used with HTTP mount",
                            agent_class_name,
                            param_name,
                        ));
                }
            }
        }

        DataSchema::Multimodal(_) => {
            return Err(format!(
                "Agent '{}' constructor cannot use 'Multimodal' schema when used with HTTP mount",
                agent_class_name,
            ));
        }
    }

    Ok(())
}

fn validate_mount_variables_are_not_principal(
    agent_mount: &HttpMountDetails,
    parameters_for_principal: &HashSet<String>,
) -> Result<(), String> {
    for segment in &agent_mount.path_prefix {
        if let PathSegment::PathVariable(variable) = segment {
            let variable_name = &variable.variable_name;

            if parameters_for_principal.contains(variable_name) {
                return Err(format!(
                    "HTTP mount path variable '{}' cannot be used for constructor parameters of type 'Principal'",
                    variable_name,
                ));
            }
        }
    }

    Ok(())
}

fn validate_mount_variables_exist_in_constructor(
    agent_mount: &HttpMountDetails,
    constructor_vars: &std::collections::HashSet<String>,
) -> Result<(), String> {
    for (segment_index, segment) in agent_mount.path_prefix.iter().enumerate() {
        if let PathSegment::PathVariable(path_variable) = segment {
            let variable_name = &path_variable.variable_name;

            if !constructor_vars.contains(variable_name) {
                return Err(format!(
                    "HTTP mount path variable '{}' (in path segment {}) is not defined in the agent constructor.",
                    variable_name,
                    segment_index,
                ));
            }
        }
    }

    Ok(())
}

fn validate_constructor_vars_are_satisfied(
    agent_mount: &HttpMountDetails,
    constructor_vars: &std::collections::HashSet<String>,
) -> Result<(), String> {
    let provided_vars = collect_http_mount_variables(agent_mount);

    for constructor_var in constructor_vars {
        if !provided_vars.contains(constructor_var) {
            return Err(format!(
                "Agent constructor variable '{}' is not provided by the HTTP mount path.",
                constructor_var,
            ));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {

    use test_r::test;

    use super::*;
    use crate::agentic::{Schema, StructuredSchema};
    use crate::golem_agentic::golem::agent::common::{AuthDetails, CorsOptions, PathVariable};
    use golem_rust_macro::AllowedMimeTypes;
    use golem_wasm::agentic::unstructured_binary::UnstructuredBinary;
    use std::collections::HashSet;

    fn principal_params(names: &[&str]) -> HashSet<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    fn constructor_with_params(params: Vec<(&str, StructuredSchema)>) -> AgentConstructor {
        let data_schema = DataSchema::Tuple(
            params
                .into_iter()
                .map(|(name, schema)| {
                    let element_schema = match schema {
                        StructuredSchema::Default(es) => es,
                        StructuredSchema::Multimodal(_) => {
                            panic!("Multimodal schema not supported in this test constructor")
                        }
                        StructuredSchema::AutoInjected(_) => {
                            panic!("AutoInjected schema not supported in this test constructor")
                        }
                    };

                    (name.to_string(), element_schema)
                })
                .collect(),
        );

        AgentConstructor {
            name: None,
            description: "".to_string(),
            prompt_hint: None,
            input_schema: data_schema,
        }
    }

    fn mount_with_segments(segments: Vec<PathSegment>) -> HttpMountDetails {
        HttpMountDetails {
            path_prefix: segments,
            auth_details: Some(AuthDetails { required: true }),
            phantom_agent: false,
            cors_options: CorsOptions {
                allowed_patterns: vec![],
            },
            webhook_suffix: vec![],
        }
    }

    fn path_var(name: &str) -> PathSegment {
        PathSegment::PathVariable(PathVariable {
            variable_name: name.to_string(),
        })
    }

    fn catch_all(name: &str) -> PathSegment {
        PathSegment::RemainingPathVariable(PathVariable {
            variable_name: name.to_string(),
        })
    }

    fn literal() -> PathSegment {
        PathSegment::Literal("literal".to_string())
    }

    #[test]
    fn validate_http_mount_success() {
        let constructor = constructor_with_params(vec![
            ("user_id", String::get_type()),
            ("org_id", String::get_type()),
        ]);

        let mount = mount_with_segments(vec![literal(), path_var("user_id"), path_var("org_id")]);

        let principal_params = principal_params(&[]);

        let result = validate_http_mount("MyAgent", &mount, &constructor, &principal_params);

        assert!(result.is_ok());
    }

    #[test]
    fn fails_on_catch_all_segment() {
        let constructor = constructor_with_params(vec![("id", String::get_type())]);

        let mount = mount_with_segments(vec![path_var("id"), catch_all("rest")]);

        let err =
            validate_http_mount("MyAgent", &mount, &constructor, &HashSet::new()).unwrap_err();

        assert_eq!(
            err,
            "HTTP mount for agent 'MyAgent' cannot contain catch-all path variable 'rest'"
        );
    }

    #[derive(AllowedMimeTypes, Clone, Debug)]
    enum MimeTypes {
        #[mime_type("application/json")]
        ApplicationJson,
        Fr,
    }

    #[test]
    fn fails_on_unstructured_binary_constructor_param() {
        let constructor =
            constructor_with_params(vec![("blob", UnstructuredBinary::<MimeTypes>::get_type())]);

        let mount = mount_with_segments(vec![path_var("blob")]);

        let err =
            validate_http_mount("MyAgent", &mount, &constructor, &HashSet::new()).unwrap_err();

        assert_eq!(
            err,
            "Agent 'MyAgent' constructor parameter 'blob' cannot be of type 'UnstructuredBinary' when used with HTTP mount"
        );
    }

    #[test]
    fn fails_when_mount_variable_is_principal() {
        let constructor = constructor_with_params(vec![("user", String::get_type())]);

        let mount = mount_with_segments(vec![path_var("user")]);

        let principal_params = principal_params(&["user"]);

        let err =
            validate_http_mount("MyAgent", &mount, &constructor, &principal_params).unwrap_err();

        assert_eq!(
            err,
            "HTTP mount path variable 'user' cannot be used for constructor parameters of type 'Principal'"
        );
    }

    #[test]
    fn fails_when_mount_variable_not_in_constructor() {
        let constructor = constructor_with_params(vec![("id", String::get_type())]);

        let mount = mount_with_segments(vec![path_var("missing")]);

        let err =
            validate_http_mount("MyAgent", &mount, &constructor, &HashSet::new()).unwrap_err();

        assert_eq!(
            err,
            "HTTP mount path variable 'missing' (in path segment 0) is not defined in the agent constructor."
        );
    }

    #[test]
    fn fails_when_constructor_var_not_satisfied() {
        let constructor = constructor_with_params(vec![
            ("id", String::get_type()),
            ("org", String::get_type()),
        ]);

        let mount = mount_with_segments(vec![path_var("id")]);

        let err =
            validate_http_mount("MyAgent", &mount, &constructor, &HashSet::new()).unwrap_err();

        assert_eq!(
            err,
            "Agent constructor variable 'org' is not provided by the HTTP mount path."
        );
    }
}
