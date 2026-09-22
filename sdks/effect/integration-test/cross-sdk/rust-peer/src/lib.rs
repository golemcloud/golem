use effect_fixture_guest_client::{
    EffectFixture, Header, Items, TransformInRequest, TransformStreamInRequest,
    TransformStreamInRequestItems, new_transform_stream_in_request_items_stream,
};
use golem_rust::agentic::{DynamicToolClient, get_tool_type, spawn_local};
use golem_rust::bindings::golem::permissions::{derive, types};
use golem_rust::schema::wit::GuestPermissionCardHandle;
use golem_rust::{
    GolemReflectError, IntoSchema, MethodOnlyAgentClientDefinition, SchemaValue, TypedSchemaValue,
    agent_definition, agent_implementation, get_agent_type, get_agent_type_for,
};
use std::cell::Cell;
use std::rc::Rc;

#[derive(IntoSchema)]
struct PrincipalPeerId {
    tenant: String,
}

#[derive(IntoSchema)]
struct EmptyAgentInput {}

#[agent_definition]
pub trait RustPeer {
    fn new(name: String) -> Self;
    async fn echo(&self, value: String) -> String;
    async fn call_effect(&self, tenant: String, request_id: String) -> String;
    async fn call_effect_failure(&self, tenant: String, request_id: String) -> String;
    async fn call_effect_stream(&self, tenant: String) -> String;
    async fn nonfinite(&self, kind: String) -> f64;
    async fn permission_card_through_effect(&self, tenant: String) -> String;
    async fn reflected_ts_tool(&self, label: String) -> String;
    async fn reflected_optional_tool(&self) -> String;
    async fn reflected_ts_agent(&self) -> String;
    async fn principal_identity_round_trip(&self) -> String;
}

struct RustPeerImpl {
    name: String,
}

#[agent_implementation]
impl RustPeer for RustPeerImpl {
    fn new(name: String) -> Self {
        Self { name }
    }
    async fn echo(&self, value: String) -> String {
        format!("rust:{}:{value}", self.name)
    }
    async fn reflected_ts_agent(&self) -> String {
        let result = async {
            let agent_type = get_agent_type("TsPeer")?;
            let client = agent_type.client().get_json(
                &serde_json::json!({ "name": format!("rust-reflected-{}", self.name) }),
            )?;
            let count = client.method("scheduledCount")?;
            let mark = client.method("markScheduled")?;
            let empty = SchemaValue::Record { fields: vec![] };
            let before_json = count.invoke_json(&serde_json::json!({})).await?;
            let before_native = count.invoke_value(empty.clone()).await?;
            mark.invoke_value(empty.clone()).await?;
            let after_json = count.invoke_json(&serde_json::json!({})).await?;
            let parts = before_json.metadata.agent_id.parts()?;
            let discovered = get_agent_type_for(&before_json.metadata.agent_id)?;
            let rebound = discovered.bind(&before_json.metadata.agent_id)?;
            let rebound_count = rebound
                .method("scheduledCount")?
                .invoke_value(empty.clone())
                .await?;
            let dynamic = before_json.metadata.agent_id.dynamic_client()?;
            let dynamic_count = dynamic.method("scheduledCount").invoke_value(empty).await?;
            Ok::<_, GolemReflectError>(format!(
                "{}|{}|{}|{:?}|{}|{:?}|{:?}",
                parts.type_name,
                discovered.name(),
                before_json
                    .value
                    .map_or("unit".to_string(), |value| value.to_string()),
                before_native.value,
                after_json
                    .value
                    .map_or("unit".to_string(), |value| value.to_string()),
                rebound_count.value,
                dynamic_count.value,
            ))
        }
        .await;
        result.unwrap_or_else(|error| format!("error:{error}"))
    }
    async fn principal_identity_round_trip(&self) -> String {
        let result = async {
            let tenant = format!("principal-rust-{}", self.name);
            let agent_type = get_agent_type("TsPrincipalPeer")?;
            let reflected = agent_type
                .client()
                .get_json(&serde_json::json!({ "tenant": tenant }))?;
            let first = reflected
                .method("value")?
                .invoke_json(&serde_json::json!({}))
                .await?;
            let host_id = first.metadata.agent_id;
            let parts = host_id.parts()?;
            if parts.type_name != "TsPrincipalPeer"
                || parts.constructor_value
                    != (SchemaValue::Record {
                        fields: vec![SchemaValue::String(tenant.clone())],
                    })
            {
                return Ok::<_, GolemReflectError>("principal-in-id".to_string());
            }
            let full = MethodOnlyAgentClientDefinition::builder()
                .durable::<PrincipalPeerId>("TsPrincipalPeer")
                .method::<EmptyAgentInput, String>("value")?
                .build();
            let method_only = MethodOnlyAgentClientDefinition::builder()
                .method_only()
                .method::<EmptyAgentInput, String>("value")?
                .build();
            let local_id = full.agent_id(
                &PrincipalPeerId {
                    tenant: tenant.clone(),
                },
                None,
            )?;
            if local_id != host_id {
                return Ok("identity-mismatch".to_string());
            }
            let full_call = full
                .bind(&host_id)?
                .method::<EmptyAgentInput, String>("value")?
                .invoke(&EmptyAgentInput {})
                .await?;
            let method_call = method_only
                .bind(&host_id)?
                .method::<EmptyAgentInput, String>("value")?
                .invoke(&EmptyAgentInput {})
                .await?;
            Ok(format!(
                "{}|{}|{}",
                first
                    .value
                    .and_then(|value| value.as_str().map(str::to_owned))
                    .unwrap_or_default(),
                full_call.value.unwrap_or_default(),
                method_call.value.unwrap_or_default(),
            ))
        }
        .await;
        result.unwrap_or_else(|error| format!("error:{error}"))
    }
    async fn reflected_ts_tool(&self, label: String) -> String {
        let result = async {
            let tool = get_tool_type("ts-cross-plain")?;
            let command = tool.command(&[])?;
            let value = SchemaValue::Record {
                fields: vec![SchemaValue::String(label.clone())],
            };
            let native = command.invoke_value(value).await?;
            let json = command
                .invoke_json(&serde_json::json!({ "label": label }))
                .await?;
            let invalid = command
                .invoke_json(&serde_json::json!({ "label": 42 }))
                .await;
            let input = TypedSchemaValue::new(
                command.input_schema().graph().clone(),
                SchemaValue::Record {
                    fields: vec![SchemaValue::String(label)],
                },
            );
            let dynamic = DynamicToolClient::new("ts-cross-plain")
                .invoke(&[], &input)
                .await
                .map_err(golem_rust::agentic::ToolReflectionError::Tool)?;
            Ok::<_, golem_rust::agentic::ToolReflectionError>((
                native,
                json,
                invalid.is_err(),
                dynamic,
            ))
        }
        .await;
        match result {
            Ok((Some(SchemaValue::String(native)), Some(json), invalid, dynamic)) => {
                format!(
                    "{native}|{json}|{invalid}|{:?}",
                    dynamic.result.map(|value| value.into_parts().1)
                )
            }
            Ok(_) => "unexpected tool output".to_string(),
            Err(error) => format!("error:{error}"),
        }
    }
    async fn reflected_optional_tool(&self) -> String {
        let result = async {
            let tool = get_tool_type("ts-optional-reflection")?;
            let command = tool.command(&[])?;
            let omitted_json = command
                .invoke_json(&serde_json::json!({ "maybe": null }))
                .await?;
            let supplied_json = command
                .invoke_json(&serde_json::json!({ "maybe": "supplied" }))
                .await?;
            let omitted_native = command
                .invoke_value(SchemaValue::Record {
                    fields: vec![SchemaValue::Option { inner: None }],
                })
                .await?;
            let supplied_native = command
                .invoke_value(SchemaValue::Record {
                    fields: vec![SchemaValue::Option {
                        inner: Some(Box::new(SchemaValue::String("supplied".to_string()))),
                    }],
                })
                .await?;
            Ok::<_, golem_rust::agentic::ToolReflectionError>(format!(
                "{}|{}|{}|{}",
                omitted_json
                    .and_then(|value| value.as_str().map(str::to_owned))
                    .unwrap_or_else(|| "unexpected".to_string()),
                supplied_json
                    .and_then(|value| value.as_str().map(str::to_owned))
                    .unwrap_or_else(|| "unexpected".to_string()),
                match omitted_native {
                    Some(SchemaValue::String(value)) => value,
                    _ => "unexpected".to_string(),
                },
                match supplied_native {
                    Some(SchemaValue::String(value)) => value,
                    _ => "unexpected".to_string(),
                },
            ))
        }
        .await;
        result.unwrap_or_else(|error| format!("error:{error}"))
    }
    async fn call_effect(&self, tenant: String, request_id: String) -> String {
        let client = EffectFixture::get_with_config(tenant, Some("rust-override".into()))
            .expect("generated Effect client");
        let request = TransformInRequest {
            header: Header {
                request_id,
                flags: vec![true, false, true],
            },
            items: vec![Items {
                sku: "RUST".into(),
                quantities: vec![4.0, 5.0],
            }],
        };
        match client.transform(request).await {
            Ok(Ok(result)) => format!("{}:{}", result.summary, result.accepted[0].total),
            Ok(Err(error)) => format!("error:{}", error.code),
            Err(error) => format!("error:{error:?}"),
        }
    }
    async fn nonfinite(&self, kind: String) -> f64 {
        match kind.as_str() {
            "nan" => f64::NAN,
            "positive" => f64::INFINITY,
            _ => f64::NEG_INFINITY,
        }
    }
    async fn call_effect_failure(&self, tenant: String, request_id: String) -> String {
        let client = EffectFixture::get_with_config(tenant, Some("rust-failure".into()))
            .expect("generated Effect client");
        let request = TransformInRequest {
            header: Header {
                request_id,
                flags: vec![false],
            },
            items: vec![],
        };
        match client.transform(request).await {
            Ok(Err(error)) => format!("{}:{}", error.code, error.request_id),
            Ok(Ok(result)) => format!("unexpected:{}", result.summary),
            Err(error) => format!("transport:{error:?}"),
        }
    }
    async fn call_effect_stream(&self, tenant: String) -> String {
        let (mut writer, input) = new_transform_stream_in_request_items_stream();
        let pulled = Rc::new(Cell::new(0_u32));
        let producer_pulled = pulled.clone();
        spawn_local(async move {
            for id in 1..=2050 {
                producer_pulled.set(id);
                let values = match id {
                    1 => vec![-3.5, 8.25],
                    2 => vec![101.0],
                    _ => vec![999.0],
                };
                if writer
                    .write_one(TransformStreamInRequestItems {
                        id: id as f64,
                        values,
                    })
                    .await
                    .is_err()
                {
                    break;
                }
            }
        });

        let client = EffectFixture::get(tenant).expect("generated Effect client");
        let mut output = client
            .transform_stream(TransformStreamInRequest {
                prefix: "rusty".into(),
                items: input,
            })
            .await
            .expect("Effect stream invocation")
            .items;
        let first = output
            .next()
            .await
            .expect("first output item")
            .expect("first item");
        let second = output
            .next()
            .await
            .expect("second output item")
            .expect("second item");
        drop(output);
        format!(
            "first:{}:{:?}|second:{}:{:?}|stopped-early:{}|output-closed:true",
            first.id,
            first.values,
            second.id,
            second.values,
            pulled.get() < 2050
        )
    }
    async fn permission_card_through_effect(&self, tenant: String) -> String {
        let card = GuestPermissionCardHandle::new(
            derive::derive_from_wallet(&[], &[], &[], &[], None).expect("derive permission card"),
        );
        let expected = card
            .with_handle(|card| format!("{:?}", types::id(card).uuid))
            .expect("new permission card is usable");
        let client = EffectFixture::get(tenant).expect("generated Effect client");
        let returned = client
            .echo_permission_card(card.clone())
            .await
            .expect("Effect permission card echo");
        let returned_id = returned
            .with_handle(|card| format!("{:?}", types::id(card).uuid))
            .expect("returned permission card is usable");
        let old_consumed = card.with_handle(|card| types::id(card).uuid).is_none();
        format!(
            "same:{}:old-consumed:{}",
            expected == returned_id,
            old_consumed
        )
    }
}
