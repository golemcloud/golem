use golem_rust::agentic::{Component, Principal};
use golem_rust::golem_agentic::exports::golem::{
    agent::guest as agent,
    tool::{guest as tool, tool_middleware_guest as middleware},
};
use golem_rust::{FromSchema, IntoSchema, IntoTypedSchemaValue};
use test_r::test;

#[derive(IntoSchema, FromSchema)]
struct EchoInput {
    value: String,
}

#[test]
async fn only_implemented_capabilities_are_live() {
    let agents = <Component as agent::Guest>::discover_agent_types().unwrap();
    assert_eq!(agents.len(), usize::from(cfg!(feature = "agent")));
    if cfg!(feature = "agent") {
        assert_eq!(agents[0].type_name, "MinimalAgent");
    } else {
        let input = || {
            golem_rust::encode_schema_value(&golem_rust::SchemaValue::String(
                "invalid input must not be decoded".into(),
            ))
            .unwrap()
        };
        for result in [
            <Component as agent::Guest>::initialize("absent".into(), input(), Principal::Anonymous)
                .await,
            <Component as agent::Guest>::invoke("absent".into(), input(), Principal::Anonymous)
                .await
                .map(|_| ()),
        ] {
            assert!(
                matches!(result, Err(agent::AgentError::InvalidInput(message)) if message == "component has no agent implementation")
            );
        }
        use golem_rust::load_snapshot::exports::golem::api::load_snapshot;
        let result = <Component as load_snapshot::Guest>::load(load_snapshot::Snapshot {
            payload: b"not a snapshot envelope".to_vec(),
            mime_type: "application/json".into(),
        })
        .await;
        assert_eq!(
            result.unwrap_err(),
            "component has no agent snapshot support"
        );
    }

    let tools = <Component as tool::Guest>::discover_tools().unwrap();
    assert_eq!(tools.len(), usize::from(cfg!(feature = "tool")));
    let found = <Component as tool::Guest>::get_tool("public-echo".into());
    assert_eq!(found.is_ok(), cfg!(feature = "tool"));
    match found {
        Ok(mut descriptor) => {
            descriptor.commands.nodes[0].name = "modified return value".into();
            let again = <Component as tool::Guest>::get_tool("public-echo".into()).unwrap();
            assert_eq!(again.commands.nodes[0].name, "public-echo");
            assert_eq!(again.version, "1.2.3");
        }
        Err(error) => assert!(
            matches!(error, tool::ToolError::InvalidToolName(name) if name == "public-echo")
        ),
    }
    let result = <Component as tool::Guest>::invoke(
        "public-echo".into(),
        vec!["echo".into()],
        golem_rust::encode_typed_schema_value_owned(
            EchoInput {
                value: "asymmetric".into(),
            }
            .into_typed_schema_value()
            .unwrap(),
        )
        .unwrap(),
        None,
        None,
        Principal::Anonymous,
    )
    .await;
    if cfg!(feature = "tool") {
        let result = result.unwrap();
        assert!(result.stdout.is_none());
        let value = golem_rust::decode_typed_schema_value_owned(result.result.unwrap()).unwrap();
        assert_eq!(
            String::from_value(value.value()).unwrap(),
            "echo:asymmetric"
        );
    } else {
        assert!(
            matches!(result, Err(tool::ToolError::InvalidToolName(name)) if name == "public-echo")
        );
    }

    let middlewares = <Component as middleware::Guest>::discover_tool_middlewares().unwrap();
    assert_eq!(middlewares.len(), usize::from(cfg!(feature = "middleware")));
    let found = <Component as middleware::Guest>::get_tool_middleware("minimal-policy".into());
    assert_eq!(found.is_ok(), cfg!(feature = "middleware"));
    match found {
        Ok(found) => assert_eq!(found.name, "minimal-policy"),
        Err(error) => assert!(
            matches!(error, tool::ToolError::InvalidToolName(name) if name == "minimal-policy")
        ),
    }
}

#[cfg(not(feature = "agent"))]
#[test]
#[should_panic(expected = "component has no agent implementation")]
fn absent_definition_is_explicitly_unsupported() {
    <Component as agent::Guest>::get_definition();
}

#[cfg(not(feature = "agent"))]
#[test]
#[should_panic(expected = "component has no agent snapshot support")]
async fn absent_snapshot_save_is_explicitly_unsupported() {
    use golem_rust::save_snapshot::exports::golem::api::save_snapshot;
    <Component as save_snapshot::Guest>::save().await;
}
