use futures_concurrency::prelude::*;
use golem_rust::agentic::{OutputStream, Secret};
use golem_rust::quota::QuotaToken;
use golem_rust::{
    FromSchema, FromWire, IntoSchema, IntoWire, ToolError, WireSchema, tool_definition,
    tool_implementation,
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

#[tool_definition(version = "1.0.0")]
pub trait ValidationProbe {
    async fn validate(
        &self,
        mode: String,
        stdout: Option<OutputStream>,
    ) -> Result<ChainEvidence, ProbeError>;
}

struct ChainProbeImpl;

#[tool_implementation]
impl ChainProbe for ChainProbeImpl {
    async fn inspect(
        &self,
        claimed_principal: String,
        mode: String,
        observed_principal: String,
    ) -> Result<ChainEvidence, ProbeError> {
        announce_effect(&format!("chain/{mode}")).await;
        Ok(evidence(claimed_principal, observed_principal, mode))
    }
}

struct ValidationProbeImpl;

#[tool_implementation]
impl ValidationProbe for ValidationProbeImpl {
    async fn validate(
        &self,
        mode: String,
        stdout: Option<OutputStream>,
    ) -> Result<ChainEvidence, ProbeError> {
        announce_effect(&format!("validation/{mode}")).await;
        if let Some(mut stdout) = stdout {
            stdout
                .write(format!("leaf-stream:{mode}").into_bytes())
                .await
                .expect("write validation stdout");
            stdout.finish().await.expect("finish validation stdout");
        }
        if mode == "leaf-error" {
            Err(ProbeError::Rejected {
                detail: NestedEvidence {
                    label: "leaf-error".to_string(),
                    ordinal: 41,
                },
            })
        } else {
            Ok(evidence(mode.clone(), "validation".to_string(), mode))
        }
    }
}

fn evidence(claimed_principal: String, actual_principal: String, label: String) -> ChainEvidence {
    let owner_config = std::env::var("OWNER_MARKER").unwrap_or_else(|_| "missing".to_string());
    let owner_secret = Secret::<String>::new(vec!["context_secret".to_string()])
        .get()
        .unwrap_or_else(|error| format!("secret-error:{error:?}"));
    let quota = QuotaToken::new("context-quota", 7);
    quota
        .reserve(3)
        .expect("calling owner quota reservation")
        .commit(2);
    ChainEvidence {
        claimed_principal,
        actual_principal,
        owner_config,
        owner_secret,
        nested: NestedEvidence { label, ordinal: 17 },
    }
}

async fn announce_effect(path: &str) {
    use golem_rust::wasip3::http::{client, types};
    use golem_rust::wasip3::wit_future;

    let port = std::env::var("EFFECT_PORT").expect("EFFECT_PORT is configured");
    let headers = types::Fields::from_list(&[]).expect("valid effect fields");
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
        .set_path_with_query(Some(&format!("/{path}")))
        .expect("set path");
    let send = async move { client::send(request).await.expect("send effect") };
    let finish = async move {
        trailers_tx.write(Ok(None)).await.expect("finish trailers");
        transmit.await.expect("transmit effect");
    };
    let (response, ()) = (send, finish).join().await;
    assert_eq!(response.get_status_code(), 204);
}
