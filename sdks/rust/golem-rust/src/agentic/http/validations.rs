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

use crate::agentic::{
    AutoInjectedParamType, EnrichedAgentMethod, EnrichedElementSchema, ExtendedDataSchema,
};
use crate::golem_agentic::golem::agent::common::{
    AgentConstructor, DataSchema, ElementSchema, HttpEndpointDetails, HttpMountDetails, PathSegment,
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

pub fn validate_http_endpoint(
    agent_class_name: &str,
    agent_method: &EnrichedAgentMethod,
    http_mount_details: Option<&HttpMountDetails>,
) -> Result<(), String> {
    if agent_method.http_endpoint.is_empty() {
        return Ok(());
    }

    validate_mount_is_defined_for_http_endpoint(
        agent_class_name,
        agent_method,
        http_mount_details,
    )?;

    let method_vars_without_auto_injected_variables =
        collect_method_input_vars(&agent_method.input_schema);

    let principal_params = collect_principal_parameter_names(&agent_method.input_schema);

    let unstructured_binary_params = collect_unstructured_binary_params(&agent_method.input_schema);

    for endpoint in &agent_method.http_endpoint {
        validate_endpoint_variables(
            endpoint,
            &method_vars_without_auto_injected_variables,
            &principal_params,
            &unstructured_binary_params,
        )?;
    }

    Ok(())
}

fn collect_principal_parameter_names(input_schema: &ExtendedDataSchema) -> HashSet<String> {
    let mut principal_params = HashSet::new();

    match input_schema {
        ExtendedDataSchema::Tuple(name_and_schemas) => {
            for (param_name, param_schema) in name_and_schemas {
                if let EnrichedElementSchema::AutoInject(auto_injected_schema) = param_schema {
                    match auto_injected_schema {
                        AutoInjectedParamType::Principal => {
                            principal_params.insert(param_name.clone());
                        }
                    }
                }
            }
        }
        ExtendedDataSchema::Multimodal(_) => {}
    }

    principal_params
}

fn collect_unstructured_binary_params(input_schema: &ExtendedDataSchema) -> HashSet<String> {
    let mut unstructured_binary_params = HashSet::new();

    match input_schema {
        ExtendedDataSchema::Tuple(name_and_schemas) => {
            for (param_name, param_schema) in name_and_schemas {
                if let EnrichedElementSchema::ElementSchema(ElementSchema::UnstructuredBinary(_)) =
                    param_schema
                {
                    unstructured_binary_params.insert(param_name.clone());
                }
            }
        }
        ExtendedDataSchema::Multimodal(_) => {}
    }

    unstructured_binary_params
}

// Collects method input variable names, excluding auto-injected variables.
fn collect_method_input_vars(input_schema: &ExtendedDataSchema) -> HashSet<String> {
    let mut param_names = HashSet::new();

    match input_schema {
        ExtendedDataSchema::Tuple(name_and_schemas) => {
            for (param_name, param_schema) in name_and_schemas {
                if let EnrichedElementSchema::AutoInject(_) = param_schema {
                    continue;
                }
                param_names.insert(param_name.clone());
            }
        }
        ExtendedDataSchema::Multimodal(_) => {}
    }

    param_names
}

fn validate_endpoint_variables(
    endpoint: &HttpEndpointDetails,
    method_vars: &HashSet<String>,
    principal_params: &HashSet<String>,
    unstructured_binary_params: &HashSet<String>,
) -> Result<(), String> {
    fn validate_variable(
        variable_name: &str,
        location: &str,
        principal_params: &HashSet<String>,
        unstructured_binary_params: &HashSet<String>,
        method_vars: &HashSet<String>,
        binary_error: &str,
    ) -> Result<(), String> {
        if principal_params.contains(variable_name) {
            return Err(format!(
                "HTTP endpoint {} variable '{}' cannot be used for parameters of type 'Principal'",
                location, variable_name
            ));
        }

        if unstructured_binary_params.contains(variable_name) {
            return Err(binary_error.to_string());
        }

        if !method_vars.contains(variable_name) {
            return Err(format!(
                "HTTP endpoint {} variable '{}' is not defined in method input parameters.",
                location, variable_name
            ));
        }

        Ok(())
    }

    for var in &endpoint.header_vars {
        validate_variable(
            &var.variable_name,
            "header",
            principal_params,
            unstructured_binary_params,
            method_vars,
            &format!(
                "HTTP endpoint header variable '{}' cannot be used for method parameters of type 'UnstructuredBinary'",
                var.variable_name
            ),
        )?;
    }

    for var in &endpoint.query_vars {
        validate_variable(
            &var.variable_name,
            "query",
            principal_params,
            unstructured_binary_params,
            method_vars,
            &format!(
                "HTTP endpoint query variable '{}' cannot be used when the method has a single 'UnstructuredBinary' parameter.",
                var.variable_name
            ),
        )?;
    }

    for segment in &endpoint.path_suffix {
        match segment {
            PathSegment::RemainingPathVariable(path_variable)
            | PathSegment::PathVariable(path_variable) => {
                let name = &path_variable.variable_name;
                validate_variable(
                    name,
                    "path",
                    principal_params,
                    unstructured_binary_params,
                    method_vars,
                    &format!(
                        "HTTP endpoint path variable '{}' cannot be used when the method has a single 'UnstructuredBinary' parameter.",
                        name
                    ),
                )?;
            }
            PathSegment::Literal(_) => {}
            PathSegment::SystemVariable(_) => {}
        }
    }

    Ok(())
}

fn validate_mount_is_defined_for_http_endpoint(
    agent_class_name: &str,
    agent_method: &EnrichedAgentMethod,
    http_mount_details: Option<&HttpMountDetails>,
) -> Result<(), String> {
    if http_mount_details.is_none() && !agent_method.http_endpoint.is_empty() {
        return Err(format!(
            "Agent method '{}' of '{}' defines HTTP endpoints but the agent is not mounted over HTTP. \
            Please specify mount details in 'agent_definition'",
            agent_method.name, agent_class_name
        ));
    }

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
    use crate::golem_agentic::golem::agent::common::{
        AuthDetails, BinaryDescriptor, CorsOptions, HeaderVariable, HttpMethod, PathVariable,
        QueryVariable,
    };
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
                        StructuredSchema::AutoInject(_) => {
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

    fn make_schema(
        normal_vars: Vec<&str>,
        principal_vars: Vec<&str>,
        unstructured_vars: Vec<&str>,
    ) -> ExtendedDataSchema {
        let mut fields = vec![];

        for name in normal_vars {
            fields.push((
                name.to_string(),
                EnrichedElementSchema::ElementSchema(
                    String::get_type().get_element_schema().unwrap(),
                ),
            ));
        }

        for name in principal_vars {
            fields.push((
                name.to_string(),
                EnrichedElementSchema::AutoInject(AutoInjectedParamType::Principal),
            ));
        }

        for name in unstructured_vars {
            fields.push((
                name.to_string(),
                EnrichedElementSchema::ElementSchema(ElementSchema::UnstructuredBinary(
                    BinaryDescriptor { restrictions: None },
                )),
            ));
        }

        ExtendedDataSchema::Tuple(fields)
    }

    fn make_endpoint(
        http_method: HttpMethod,
        path_vars: Vec<&str>,
        header_vars: Vec<(&str, &str)>,
        query_vars: Vec<&str>,
        auth: Option<bool>,
        cors: Vec<&str>,
    ) -> HttpEndpointDetails {
        HttpEndpointDetails {
            http_method,
            path_suffix: path_vars
                .into_iter()
                .map(|name| {
                    PathSegment::PathVariable(PathVariable {
                        variable_name: name.to_string(),
                    })
                })
                .collect(),
            header_vars: header_vars
                .into_iter()
                .map(|(header_name, variable_name)| HeaderVariable {
                    header_name: header_name.to_string(),
                    variable_name: variable_name.to_string(),
                })
                .collect(),
            query_vars: query_vars
                .into_iter()
                .map(|var| QueryVariable {
                    query_param_name: var.to_string(),
                    variable_name: var.to_string(),
                })
                .collect(),
            auth_details: auth.map(|b| AuthDetails { required: b }),
            cors_options: CorsOptions {
                allowed_patterns: cors.into_iter().map(|s| s.to_string()).collect(),
            },
        }
    }

    fn make_agent_method(
        name: &str,
        input_schema: ExtendedDataSchema,
        endpoints: Vec<HttpEndpointDetails>,
    ) -> EnrichedAgentMethod {
        EnrichedAgentMethod {
            name: name.to_string(),
            description: "".to_string(),
            prompt_hint: None,
            input_schema,
            output_schema: ExtendedDataSchema::Tuple(vec![]),
            http_endpoint: endpoints,
        }
    }

    #[test]
    fn test_no_http_endpoints() {
        let agent_method = make_agent_method("foo", make_schema(vec!["x"], vec![], vec![]), vec![]);
        assert!(validate_http_endpoint("AgentA", &agent_method, None).is_ok());
    }

    #[test]
    fn test_http_endpoint_without_mount() {
        let endpoint = make_endpoint(
            HttpMethod::Get,
            vec!["x"],
            vec![],
            vec![],
            Some(true),
            vec![],
        );
        let agent_method = make_agent_method(
            "foo",
            make_schema(vec!["x"], vec![], vec![]),
            vec![endpoint],
        );
        let err = validate_http_endpoint("AgentA", &agent_method, None).unwrap_err();
        assert!(err.contains("defines HTTP endpoints but the agent is not mounted over HTTP"));
    }

    #[test]
    fn test_valid_endpoint() {
        let endpoint = make_endpoint(
            HttpMethod::Get,
            vec!["x"],
            vec![("X-Test", "x")],
            vec!["x"],
            Some(true),
            vec!["*"],
        );
        let agent_method = make_agent_method(
            "foo",
            make_schema(vec!["x"], vec![], vec![]),
            vec![endpoint],
        );
        let mount = mount_with_segments(vec![path_var("foo")]);
        assert!(validate_http_endpoint("AgentA", &agent_method, Some(&mount)).is_ok());
    }

    #[test]
    fn test_header_principal_error() {
        let endpoint = make_endpoint(
            HttpMethod::Get,
            vec![],
            vec![("X-Test", "p")],
            vec![],
            Some(true),
            vec![],
        );
        let agent_method = make_agent_method(
            "foo",
            make_schema(vec![], vec!["p"], vec![]),
            vec![endpoint],
        );
        let mount = mount_with_segments(vec![path_var("foo")]);
        let err = validate_http_endpoint("AgentA", &agent_method, Some(&mount)).unwrap_err();
        assert!(err.contains("cannot be used for parameters of type 'Principal'"));
    }

    #[test]
    fn test_header_unstructured_binary_error() {
        let endpoint = make_endpoint(
            HttpMethod::Get,
            vec![],
            vec![("X-Test", "b")],
            vec![],
            Some(true),
            vec![],
        );
        let agent_method = make_agent_method(
            "foo",
            make_schema(vec![], vec![], vec!["b"]),
            vec![endpoint],
        );
        let mount = mount_with_segments(vec![path_var("foo")]);
        let err = validate_http_endpoint("AgentA", &agent_method, Some(&mount)).unwrap_err();
        assert!(err.contains("cannot be used for method parameters of type 'UnstructuredBinary'"));
    }

    #[test]
    fn test_path_variable_not_in_method_params() {
        let endpoint = make_endpoint(
            HttpMethod::Get,
            vec!["y"],
            vec![],
            vec![],
            Some(true),
            vec![],
        );
        let agent_method = make_agent_method(
            "foo",
            make_schema(vec!["x"], vec![], vec![]),
            vec![endpoint],
        );
        let mount = mount_with_segments(vec![path_var("foo")]);
        let err = validate_http_endpoint("AgentA", &agent_method, Some(&mount)).unwrap_err();
        assert!(err.contains("is not defined in method input parameters"));
    }
}
