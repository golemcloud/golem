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

//! Agents exercising host-managed capability schema types end-to-end.

use golem_rust::agentic::{Config, Secret};
use golem_rust::bindings::golem::secrets::reveal;
use golem_rust::permissions::{
    self, Bounds, PermissionCard, PolymorphicPermissionCard, WalletCard,
};
use golem_rust::quota::QuotaToken;
use golem_rust::secrets::GuestSecretHandle;
use golem_rust::{
    ConfigSchema, FromSchema, IntoSchema, ParsedAgentId, SchemaValue, agent_client,
    agent_definition, agent_implementation, decode_schema_value, encode_schema_graph,
    encode_schema_value,
};

#[agent_definition]
pub trait CapabilityEchoAgent {
    fn new(name: String) -> Self;

    fn echo_secret(&self, value: GuestSecretHandle) -> GuestSecretHandle;
}

pub struct CapabilityEchoAgentImpl {
    _name: String,
}

#[agent_implementation]
impl CapabilityEchoAgent for CapabilityEchoAgentImpl {
    fn new(name: String) -> Self {
        Self { _name: name }
    }

    fn echo_secret(&self, value: GuestSecretHandle) -> GuestSecretHandle {
        value
    }
}

fn secret_id(secret: &GuestSecretHandle) -> Result<String, String> {
    secret
        .with_handle(golem_rust::bindings::golem::secrets::types::id)
        .map(|id| format!("{:02x?}", id.bytes))
        .ok_or_else(|| "secret handle has already been transferred".to_string())
}

fn reveal_secret(secret: &GuestSecretHandle) -> Result<String, String> {
    let graph =
        golem_rust::schema::try_into_schema_graph::<String>().map_err(|error| error.to_string())?;
    let expected_type = encode_schema_graph(&graph).map_err(|error| error.to_string())?;
    let value = secret
        .with_handle(|handle| reveal::reveal(handle, &expected_type))
        .ok_or_else(|| "secret handle has already been transferred".to_string())?
        .map_err(|error| format!("{error:?}"))?;
    let value = decode_schema_value(value).map_err(|error| error.to_string())?;
    String::from_value(&value).map_err(|error| error.to_string())
}

fn permission_card() -> Result<PermissionCard, String> {
    permissions::derive_from_wallet(&Bounds::default(), None).map_err(|error| error.to_string())
}

/// The agent's initial card, a template card.
fn template_card() -> Result<PolymorphicPermissionCard, String> {
    permissions::wallet()
        .into_iter()
        .find_map(|card| match card {
            WalletCard::Template(card) => Some(card),
            _ => None,
        })
        .ok_or_else(|| "the wallet has no template card".to_string())
}

#[agent_definition]
pub trait CapabilityRpcReceiver {
    fn new(name: String) -> Self;

    /// Returns the received capabilities, the id of the received secret, and
    /// whether `card` is the parent of `child`, which the sender derived
    /// from `card` before sending both.
    async fn return_capabilities(
        &self,
        secret: GuestSecretHandle,
        quota: QuotaToken,
        card: PermissionCard,
        child: PermissionCard,
    ) -> (String, bool, GuestSecretHandle, QuotaToken, PermissionCard);

    /// Returns a received template card.
    fn return_template(&self, card: PolymorphicPermissionCard) -> PolymorphicPermissionCard;

    /// Accepts a concrete card.
    fn accept_card(&self, card: PermissionCard) -> bool;
}

/// A method-only client of `CapabilityRpcReceiver`, whose awaited calls
/// return RPC errors instead of trapping.
#[agent_client]
pub trait CapabilityCardAcceptor {
    fn accept_card(&self, card: PermissionCard) -> bool;
}

pub struct CapabilityRpcReceiverImpl;

#[agent_implementation]
impl CapabilityRpcReceiver for CapabilityRpcReceiverImpl {
    fn new(_name: String) -> Self {
        Self
    }

    async fn return_capabilities(
        &self,
        secret: GuestSecretHandle,
        quota: QuotaToken,
        card: PermissionCard,
        child: PermissionCard,
    ) -> (String, bool, GuestSecretHandle, QuotaToken, PermissionCard) {
        let secret_id = secret_id(&secret).expect("received secret is usable");
        let received_sent_card = child.parents() == vec![card.id()];
        quota
            .reserve(0)
            .expect("received quota token is usable")
            .commit(0);
        (secret_id, received_sent_card, secret, quota, card)
    }

    fn return_template(&self, card: PolymorphicPermissionCard) -> PolymorphicPermissionCard {
        card
    }

    fn accept_card(&self, card: PermissionCard) -> bool {
        let _ = card.id();
        true
    }
}

#[derive(ConfigSchema)]
pub struct CapabilityRpcSenderConfig {
    #[config_schema(secret)]
    secret_path: Secret<String>,
}

#[agent_definition]
pub trait CapabilityRpcSender {
    fn new(name: String, #[agent_config] config: Config<CapabilityRpcSenderConfig>) -> Self;

    /// Sends capabilities to a receiver and back. Returns the secret ids seen
    /// by the sender, the receiver and the sender again, then the revealed
    /// secret, whether the receiver got the sent card, whether the sender got
    /// it back, whether the sender got its template card back, and the error
    /// of the call that sends the template card, decoded as a concrete card,
    /// to a concrete-card parameter.
    async fn round_trip(&self, receiver_name: String) -> Result<RoundTrip, String>;

    fn codec_rejections(&self) -> Result<Vec<String>, String>;
}

type RoundTrip = (Vec<String>, bool, bool, bool, Option<String>);

pub struct CapabilityRpcSenderImpl {
    secret: GuestSecretHandle,
}

#[agent_implementation]
impl CapabilityRpcSender for CapabilityRpcSenderImpl {
    fn new(_name: String, #[agent_config] config: Config<CapabilityRpcSenderConfig>) -> Self {
        Self {
            secret: config
                .get()
                .expect("config access should be allowed")
                .secret_path
                .handle()
                .expect("secret handle access should be allowed"),
        }
    }

    async fn round_trip(&self, receiver_name: String) -> Result<RoundTrip, String> {
        let expected_secret_id = secret_id(&self.secret)?;
        let card = permission_card()?;
        let child = card
            .derive(&Bounds::default(), None)
            .map_err(|error| error.to_string())?;
        let sent_card_id = card.id();
        let quota = QuotaToken::new("capability-rpc", 1);
        let client = CapabilityRpcReceiverClient::get(receiver_name);
        let acceptor =
            CapabilityCardAcceptorClient::bind(&ParsedAgentId::new(client.get_agent_id()))
                .map_err(|error| format!("{error:?}"))?;
        let (receiver_secret_id, receiver_got_sent_card, secret, quota, card) = client
            .return_capabilities(self.secret.clone(), quota, card, child)
            .await;

        quota
            .reserve(0)
            .map_err(|error| format!("{error:?}"))?
            .commit(0);
        let revealed = reveal_secret(&secret)?;
        let returned_secret_id = secret_id(&secret)?;

        let template = template_card()?;
        let template_id = template.id();
        let returned_template = client.return_template(template).await;

        // The guest codecs see only the handle, so a template card value
        // decodes as a concrete card. The host checks the declared kind.
        let mislabelled = PermissionCard::from_value(&template_card()?.to_value())
            .map_err(|error| error.to_string())?;
        let mismatched_kind_error = acceptor
            .accept_card(mislabelled)
            .await
            .err()
            .map(|error| format!("{error:?}"));
        Ok((
            vec![
                expected_secret_id,
                receiver_secret_id,
                returned_secret_id,
                revealed,
            ],
            receiver_got_sent_card,
            card.id() == sent_card_id,
            returned_template.id() == template_id,
            mismatched_kind_error,
        ))
    }

    fn codec_rejections(&self) -> Result<Vec<String>, String> {
        let alias_secret = self.secret.clone();
        let secret_alias = SchemaValue::Record {
            fields: vec![alias_secret.to_value(), alias_secret.to_value()],
        };
        let secret_alias_error = encode_schema_value(&secret_alias)
            .expect_err("aliased secret must be rejected")
            .to_string();

        let quota = QuotaToken::new("capability-rpc", 1);
        let quota_alias = SchemaValue::Record {
            fields: vec![quota.to_value(), quota.to_value()],
        };
        let quota_alias_error = encode_schema_value(&quota_alias)
            .expect_err("aliased quota token must be rejected")
            .to_string();
        encode_schema_value(&quota.to_value()).map_err(|error| error.to_string())?;
        let quota_consumed_error = encode_schema_value(&quota.to_value())
            .expect_err("consumed quota token must be rejected")
            .to_string();

        let card = permission_card()?;
        let card_alias = SchemaValue::Record {
            fields: vec![card.to_value(), card.to_value()],
        };
        let card_alias_error = encode_schema_value(&card_alias)
            .expect_err("aliased permission card must be rejected")
            .to_string();
        encode_schema_value(&card.to_value()).map_err(|error| error.to_string())?;
        let card_consumed_error = encode_schema_value(&card.to_value())
            .expect_err("consumed permission card must be rejected")
            .to_string();

        let template = template_card()?;
        let template_alias = SchemaValue::Record {
            fields: vec![template.to_value(), template.to_value()],
        };
        let template_alias_error = encode_schema_value(&template_alias)
            .expect_err("aliased template card must be rejected")
            .to_string();
        encode_schema_value(&template.to_value()).map_err(|error| error.to_string())?;
        let template_consumed_error = encode_schema_value(&template.to_value())
            .expect_err("consumed template card must be rejected")
            .to_string();

        let consumed_secret = self.secret.clone();
        encode_schema_value(&consumed_secret.to_value()).map_err(|error| error.to_string())?;
        let secret_consumed_error = encode_schema_value(&consumed_secret.to_value())
            .expect_err("consumed secret must be rejected")
            .to_string();
        Ok(vec![
            secret_alias_error,
            secret_consumed_error,
            quota_alias_error,
            quota_consumed_error,
            card_alias_error,
            card_consumed_error,
            template_alias_error,
            template_consumed_error,
        ])
    }
}
