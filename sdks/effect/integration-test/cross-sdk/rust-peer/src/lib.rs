use effect_fixture_guest_client::{
    EffectFixture, Header, Items, TransformInRequest, TransformStreamInRequest,
    TransformStreamInRequestItems, new_transform_stream_in_request_items_stream,
};
use golem_rust::agentic::spawn_local;
use golem_rust::bindings::golem::permissions::{derive, types};
use golem_rust::schema::wit::GuestPermissionCardHandle;
use golem_rust::{agent_definition, agent_implementation};
use std::cell::Cell;
use std::rc::Rc;

#[agent_definition]
pub trait RustPeer {
    fn new(name: String) -> Self;
    async fn echo(&self, value: String) -> String;
    async fn call_effect(&self, tenant: String, request_id: String) -> String;
    async fn call_effect_failure(&self, tenant: String, request_id: String) -> String;
    async fn call_effect_stream(&self, tenant: String) -> String;
    async fn nonfinite(&self, kind: String) -> f64;
    async fn permission_card_through_effect(&self, tenant: String) -> String;
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
