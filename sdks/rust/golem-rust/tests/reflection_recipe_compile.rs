// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0

test_r::enable!();

#[cfg(feature = "export_golem_agentic")]
mod recipe {
    use golem_rust::{AgentMode, GolemReflectError, get_agent_type};
    use serde_json::{Value, json};
    use test_r::test;

    async fn search_dynamically() -> Result<Value, GolemReflectError> {
        let agent_type = get_agent_type("SearchAgent")?;
        if agent_type.mode() != AgentMode::Durable {
            return Err(GolemReflectError::InvalidType(
                "SearchAgent must be durable".to_string(),
            ));
        }
        let method = agent_type.method("search")?;

        let input = method
            .input()
            .pack_json(&json!({ "query": "golem", "cursor": null }))?;
        method.input().validate_value(&input)?;

        let agent_id = agent_type.agent_id_json(&json!({ "tenant": "docs" }), None)?;
        let result = match agent_id
            .dynamic_client()?
            .method(method.name())
            .invoke_value(input)
            .await
        {
            Ok(result) => result,
            Err(error) => {
                eprintln!("dynamic search failed: {error}");
                return Err(error);
            }
        };

        let output = method.output().ok_or_else(|| {
            GolemReflectError::MalformedRemoteOutput(
                "search unexpectedly declares unit output".to_string(),
            )
        })?;
        let value = result.value.ok_or_else(|| {
            GolemReflectError::MalformedRemoteOutput(
                "search returned unit instead of a value".to_string(),
            )
        })?;
        output.validate_value(&value)?;
        output.unpack_json(&value)
    }

    #[test]
    fn discovery_to_dynamic_recipe_typechecks() {
        let _ = search_dynamically;
    }
}
