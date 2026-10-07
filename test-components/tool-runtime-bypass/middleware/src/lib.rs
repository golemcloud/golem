use futures_concurrency::prelude::*;
use golem_rust::agentic::spawn_local;
use golem_rust::tool::{
    InputStream, InvocationResult, OutputStream, Principal, RawCustomToolError, Tool,
    ToolInvokeError, UnderlyingTool,
};
use golem_rust::{
    FromSchema, FromWire, IntoSchema, IntoTypedSchemaValue, IntoWire, SchemaValue, ToolError,
    TypedSchemaValue, WireSchema, tool_definition, tool_middleware, universal_tool_middleware,
};

#[derive(Debug, Clone, IntoSchema, FromSchema, IntoWire, FromWire, WireSchema)]
pub struct NestedEvidence {
    pub label: String,
    pub ordinal: u64,
}

#[derive(Debug, Clone, IntoSchema, FromSchema, IntoWire, FromWire, WireSchema)]
pub struct ChainEvidence {
    pub claimed_principal: String,
    pub actual_principal: String,
    pub owner_config: String,
    pub owner_secret: String,
    pub nested: NestedEvidence,
}

#[derive(Debug, Clone, ToolError)]
pub enum ProbeError {
    #[tool_error(kind = "runtime-error", exit_code = 23)]
    Rejected { detail: NestedEvidence },
}

#[tool_definition(version = "1.0.0")]
pub trait ChainProbe {
    async fn inspect(
        &self,
        claimed_principal: String,
        mode: String,
        observed_principal: String,
    ) -> Result<ChainEvidence, ProbeError>;
}

macro_rules! typed_layer {
    ($type:ident, [$($attribute:tt)*], $label:literal) => {
        struct $type;

        impl $type {
            fn new() -> Self {
                Self
            }
        }

        #[tool_middleware($($attribute)*, constructor = $type::new)]
        impl ChainProbeMiddleware for $type {
            async fn inspect(
                &self,
                underlying: &ChainProbeUnderlying,
                claimed_principal: String,
                mode: String,
                observed_principal: String,
            ) -> Result<ChainEvidence, ToolInvokeError<ProbeError>> {
                let mut result = underlying
                    .inspect(
                        format!("{}>{claimed_principal}", $label),
                        mode,
                        observed_principal,
                    )
                    .await?;
                result.nested.label = format!("{}>{}", $label, result.nested.label);
                Ok(result)
            }
        }
    };
}

typed_layer!(PerToolOne, [name = "runtime-bypass-p1"], "P1");
typed_layer!(PerToolTwo, [name = "runtime-bypass-p2"], "P2");

fn rewrite_claimed(input: TypedSchemaValue, label: &str) -> TypedSchemaValue {
    let (graph, mut value) = input.into_parts();
    let SchemaValue::Record { fields } = &mut value else {
        panic!("chain input is a record");
    };
    let SchemaValue::String(claimed) = &mut fields[0] else {
        panic!("claimed principal is a string");
    };
    *claimed = format!("{label}>{claimed}");
    TypedSchemaValue::new(graph, value)
}

fn inject_principal(input: TypedSchemaValue, principal: &Principal) -> TypedSchemaValue {
    let (graph, mut value) = input.into_parts();
    let SchemaValue::Record { fields } = &mut value else {
        panic!("chain input is a record");
    };
    fields[2] = SchemaValue::String(format!("{principal:?}"));
    TypedSchemaValue::new(graph, value)
}

async fn universal_layer(
    label: &str,
    tool_name: String,
    command_path: Vec<String>,
    input: TypedSchemaValue,
    stdin: Option<InputStream>,
    stdout: Option<OutputStream>,
    stderr: Option<OutputStream>,
    underlying: UnderlyingTool,
) -> Result<InvocationResult, ToolInvokeError<RawCustomToolError>> {
    let input = if tool_name == "chain-probe" {
        rewrite_claimed(input, label)
    } else {
        input
    };
    let mut result = underlying
        .invoke_forwarding_outputs(command_path, input, stdin, stdout, stderr)
        .await?;
    if tool_name == "chain-probe" {
        let value = result.result.take().expect("chain result");
        let mut evidence = ChainEvidence::from_value(value.value())
            .map_err(|error| ToolInvokeError::InvalidResult(error.to_string()))?;
        evidence.nested.label = format!("{label}>{}", evidence.nested.label);
        result.result = Some(
            evidence
                .into_typed_schema_value()
                .map_err(|error| ToolInvokeError::InvalidResult(error.to_string()))?,
        );
    }
    Ok(result)
}

#[universal_tool_middleware(name = "runtime-bypass-u1")]
async fn universal_one(
    tool_name: String,
    _tool_metadata: Tool,
    command_path: Vec<String>,
    input: TypedSchemaValue,
    stdin: Option<InputStream>,
    stdout: Option<OutputStream>,
    stderr: Option<OutputStream>,
    principal: Principal,
    underlying: UnderlyingTool,
) -> Result<InvocationResult, ToolInvokeError<RawCustomToolError>> {
    let input = if tool_name == "chain-probe" {
        inject_principal(input, &principal)
    } else {
        input
    };
    if tool_name == "chain-probe"
        && matches!(input.value(), SchemaValue::Record { fields } if matches!(fields.get(1), Some(SchemaValue::String(mode)) if mode == "stale"))
    {
        let stale_input = rewrite_claimed(input, "stale-U1");
        let promise = golem_rust::create_promise();
        let checkpoint = promise.oplog_idx;
        spawn_local(async move {
            announce_checkpoint(checkpoint).await;
            golem_rust::await_promise(&promise).await;
            let _ = underlying.invoke(command_path, stale_input, None).await;
        });
        return Ok(InvocationResult {
            result: Some(
                ChainEvidence {
                    claimed_principal: "stale-return".to_string(),
                    actual_principal: format!("{principal:?}"),
                    owner_config: "stale-return".to_string(),
                    owner_secret: "stale-return".to_string(),
                    nested: NestedEvidence {
                        label: "stale-return".to_string(),
                        ordinal: 0,
                    },
                }
                .into_typed_schema_value()
                .map_err(|error| ToolInvokeError::InvalidResult(error.to_string()))?,
            ),
            stdout: None,
            stderr: None,
        });
    }
    if tool_name == "chain-probe"
        && matches!(input.value(), SchemaValue::Record { fields } if matches!(fields.get(1), Some(SchemaValue::String(mode)) if mode == "overlap"))
    {
        let left = underlying.invoke(
            command_path.clone(),
            rewrite_claimed(input.clone(), "U1-left"),
            None,
        );
        let right = underlying.invoke(command_path, rewrite_claimed(input, "U1-right"), None);
        let (left, right) = (left, right).join().await;
        let mut left = left?;
        let right = right?;
        let left_value = left.result.take().expect("left result");
        let mut evidence = ChainEvidence::from_value(left_value.value())
            .map_err(|error| ToolInvokeError::InvalidResult(error.to_string()))?;
        let right_value = right.result.expect("right result");
        let right = ChainEvidence::from_value(right_value.value())
            .map_err(|error| ToolInvokeError::InvalidResult(error.to_string()))?;
        evidence.nested.label = format!("U1[{}|{}]", evidence.nested.label, right.nested.label);
        left.result = Some(
            evidence
                .into_typed_schema_value()
                .map_err(|error| ToolInvokeError::InvalidResult(error.to_string()))?,
        );
        return Ok(left);
    }
    universal_layer(
        "U1",
        tool_name,
        command_path,
        input,
        stdin,
        stdout,
        stderr,
        underlying,
    )
    .await
}

async fn announce_checkpoint(oplog_idx: u64) {
    use golem_rust::wasip3::http::{client, types};
    use golem_rust::wasip3::wit_future;

    let port = std::env::var("EFFECT_PORT").expect("EFFECT_PORT is configured");
    let headers = types::Fields::from_list(&[]).expect("valid checkpoint fields");
    let (trailers_tx, trailers_rx) = wit_future::new(|| Ok(None));
    let (request, transmit) = types::Request::new(headers, None, trailers_rx, None);
    request
        .set_method(&types::Method::Post)
        .expect("set method");
    request
        .set_scheme(Some(&types::Scheme::Http))
        .expect("set scheme");
    request
        .set_authority(Some(&format!("127.0.0.1:{port}")))
        .expect("set authority");
    request
        .set_path_with_query(Some(&format!("/stale-ready/{oplog_idx}")))
        .expect("set path");
    let send = async move { client::send(request).await.expect("send checkpoint") };
    let finish = async move {
        trailers_tx.write(Ok(None)).await.expect("finish trailers");
        transmit.await.expect("transmit checkpoint");
    };
    let (response, ()) = (send, finish).join().await;
    assert_eq!(response.get_status_code(), 204);
}

#[universal_tool_middleware(name = "runtime-bypass-u2")]
async fn universal_two(
    tool_name: String,
    _tool_metadata: Tool,
    command_path: Vec<String>,
    input: TypedSchemaValue,
    stdin: Option<InputStream>,
    stdout: Option<OutputStream>,
    stderr: Option<OutputStream>,
    _principal: Principal,
    underlying: UnderlyingTool,
) -> Result<InvocationResult, ToolInvokeError<RawCustomToolError>> {
    if tool_name == "validation-probe" {
        let mode = match input.value() {
            SchemaValue::Record { fields } => fields.first(),
            _ => None,
        };
        let mode = match mode {
            Some(SchemaValue::String(mode)) => mode.clone(),
            _ => String::new(),
        };
        let result = universal_layer(
            "U2",
            tool_name,
            command_path,
            input,
            stdin,
            stdout,
            stderr,
            underlying,
        )
        .await;
        return match mode.as_str() {
            "wrong-root" => Ok(InvocationResult {
                result: Some(TypedSchemaValue::new(
                    golem_rust::schema::try_into_schema_graph::<ChainEvidence>().unwrap(),
                    SchemaValue::String("not-a-record".to_string()),
                )),
                stdout: None,
                stderr: None,
            }),
            "wrong-nested" => {
                let mut result = result?;
                let value = result.result.take().unwrap();
                let (graph, mut value) = value.into_parts();
                let SchemaValue::Record { fields } = &mut value else {
                    unreachable!()
                };
                fields[4] = SchemaValue::Record {
                    fields: vec![
                        SchemaValue::String("nested".to_string()),
                        SchemaValue::Bool(true),
                    ],
                };
                result.result = Some(TypedSchemaValue::new(graph, value));
                Ok(result)
            }
            "wrong-error" => Err(ToolInvokeError::Tool(RawCustomToolError::from_payload(
                "rejected".to_string(),
                TypedSchemaValue::new(
                    golem_rust::schema::try_into_schema_graph::<NestedEvidence>().unwrap(),
                    SchemaValue::Record {
                        fields: vec![
                            SchemaValue::String("bad".to_string()),
                            SchemaValue::Bool(true),
                        ],
                    },
                ),
            ))),
            "valid-transform" => {
                let mut result = result?;
                let value = result.result.take().unwrap();
                let mut evidence = ChainEvidence::from_value(value.value()).unwrap();
                evidence.nested.label = format!("valid>{}", evidence.nested.label);
                evidence.nested.ordinal += 100;
                result.result = Some(evidence.into_typed_schema_value().unwrap());
                Ok(result)
            }
            _ => result,
        };
    }
    universal_layer(
        "U2",
        tool_name,
        command_path,
        input,
        stdin,
        stdout,
        stderr,
        underlying,
    )
    .await
}
