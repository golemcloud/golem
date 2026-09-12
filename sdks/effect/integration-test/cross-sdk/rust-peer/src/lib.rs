use effect_fixture_guest_client::{EffectFixture, Header, Items, TransformInRequest};
use golem_rust::{agent_definition, agent_implementation};

#[agent_definition]
pub trait RustPeer {
    fn new(name: String) -> Self;
    async fn echo(&self, value: String) -> String;
    async fn call_effect(&self, tenant: String, request_id: String) -> String;
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
}
