// Copyright 2024-2026 Golem Cloud
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
    AgentPrincipal, GolemUserPrincipal, OidcPrincipal, Principal,
};
use golem_wasm::golem_core_1_5_x::types::{AccountId, AgentId, ComponentId, Uuid};
use serde::de::{self, MapAccess, Visitor};
use serde::ser::SerializeStruct;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use crate::value_and_type::{FromValueAndType, IntoValue, TypeNodeBuilder};
use golem_wasm::{NodeBuilder, WitValueExtractor};

fn uuid_to_string(uuid: &Uuid) -> String {
    uuid::Uuid::from_u64_pair(uuid.high_bits, uuid.low_bits).to_string()
}

fn uuid_from_string(s: &str) -> Result<Uuid, uuid::Error> {
    let parsed = uuid::Uuid::parse_str(s)?;
    let (high_bits, low_bits) = parsed.as_u64_pair();
    Ok(Uuid {
        high_bits,
        low_bits,
    })
}

impl Serialize for OidcPrincipal {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut s = serializer.serialize_struct("OidcPrincipal", 10)?;
        s.serialize_field("sub", &self.sub)?;
        s.serialize_field("issuer", &self.issuer)?;
        s.serialize_field("email", &self.email)?;
        s.serialize_field("name", &self.name)?;
        s.serialize_field("emailVerified", &self.email_verified)?;
        s.serialize_field("givenName", &self.given_name)?;
        s.serialize_field("familyName", &self.family_name)?;
        s.serialize_field("picture", &self.picture)?;
        s.serialize_field("preferredUsername", &self.preferred_username)?;
        s.serialize_field("claims", &self.claims)?;
        s.end()
    }
}

impl<'de> Deserialize<'de> for OidcPrincipal {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Helper {
            sub: String,
            issuer: String,
            email: Option<String>,
            name: Option<String>,
            email_verified: Option<bool>,
            given_name: Option<String>,
            family_name: Option<String>,
            picture: Option<String>,
            preferred_username: Option<String>,
            claims: String,
        }
        let h = Helper::deserialize(deserializer)?;
        Ok(OidcPrincipal {
            sub: h.sub,
            issuer: h.issuer,
            email: h.email,
            name: h.name,
            email_verified: h.email_verified,
            given_name: h.given_name,
            family_name: h.family_name,
            picture: h.picture,
            preferred_username: h.preferred_username,
            claims: h.claims,
        })
    }
}

impl Serialize for AgentPrincipal {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut s = serializer.serialize_struct("AgentPrincipal", 2)?;
        s.serialize_field(
            "componentId",
            &uuid_to_string(&self.agent_id.component_id.uuid),
        )?;
        s.serialize_field("agentId", &self.agent_id.agent_id)?;
        s.end()
    }
}

impl<'de> Deserialize<'de> for AgentPrincipal {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Helper {
            component_id: String,
            agent_id: String,
        }
        let h = Helper::deserialize(deserializer)?;
        let uuid = uuid_from_string(&h.component_id).map_err(de::Error::custom)?;
        Ok(AgentPrincipal {
            agent_id: AgentId {
                component_id: ComponentId { uuid },
                agent_id: h.agent_id,
            },
        })
    }
}

impl Serialize for GolemUserPrincipal {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut s = serializer.serialize_struct("GolemUserPrincipal", 1)?;
        s.serialize_field("accountId", &uuid_to_string(&self.account_id.uuid))?;
        s.end()
    }
}

impl<'de> Deserialize<'de> for GolemUserPrincipal {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Helper {
            account_id: String,
        }
        let h = Helper::deserialize(deserializer)?;
        let uuid = uuid_from_string(&h.account_id).map_err(de::Error::custom)?;
        Ok(GolemUserPrincipal {
            account_id: AccountId { uuid },
        })
    }
}

impl Serialize for Principal {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Principal::Anonymous => {
                let mut s = serializer.serialize_struct("Principal", 1)?;
                s.serialize_field("tag", "anonymous")?;
                s.end()
            }
            Principal::Oidc(oidc) => {
                let mut s = serializer.serialize_struct("Principal", 2)?;
                s.serialize_field("tag", "oidc")?;
                s.serialize_field("val", oidc)?;
                s.end()
            }
            Principal::Agent(agent) => {
                let mut s = serializer.serialize_struct("Principal", 2)?;
                s.serialize_field("tag", "agent")?;
                s.serialize_field("val", agent)?;
                s.end()
            }
            Principal::GolemUser(user) => {
                let mut s = serializer.serialize_struct("Principal", 2)?;
                s.serialize_field("tag", "golem-user")?;
                s.serialize_field("val", user)?;
                s.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for Principal {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct PrincipalVisitor;

        impl<'de> Visitor<'de> for PrincipalVisitor {
            type Value = Principal;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("a Principal object with a 'tag' field")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut tag_field: Option<String> = None;
                let mut val_field: Option<serde_json::Value> = None;

                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "tag" => tag_field = Some(map.next_value()?),
                        "val" => val_field = Some(map.next_value()?),
                        _ => {
                            let _ = map.next_value::<serde::de::IgnoredAny>()?;
                        }
                    }
                }

                let tag_str = tag_field.ok_or_else(|| de::Error::missing_field("tag"))?;

                match tag_str.as_str() {
                    "anonymous" => Ok(Principal::Anonymous),
                    "oidc" => {
                        let val = val_field.ok_or_else(|| de::Error::missing_field("val"))?;
                        let oidc: OidcPrincipal =
                            serde_json::from_value(val).map_err(de::Error::custom)?;
                        Ok(Principal::Oidc(oidc))
                    }
                    "agent" => {
                        let val = val_field.ok_or_else(|| de::Error::missing_field("val"))?;
                        let agent: AgentPrincipal =
                            serde_json::from_value(val).map_err(de::Error::custom)?;
                        Ok(Principal::Agent(agent))
                    }
                    "golem-user" => {
                        let val = val_field.ok_or_else(|| de::Error::missing_field("val"))?;
                        let user: GolemUserPrincipal =
                            serde_json::from_value(val).map_err(de::Error::custom)?;
                        Ok(Principal::GolemUser(user))
                    }
                    other => Err(de::Error::unknown_variant(
                        other,
                        &["anonymous", "oidc", "agent", "golem-user"],
                    )),
                }
            }
        }

        deserializer.deserialize_map(PrincipalVisitor)
    }
}

impl IntoValue for OidcPrincipal {
    fn add_to_builder<T: NodeBuilder>(self, builder: T) -> T::Result {
        let builder = builder.record();
        let builder = self.sub.add_to_builder(builder.item());
        let builder = self.issuer.add_to_builder(builder.item());
        let builder = self.email.add_to_builder(builder.item());
        let builder = self.name.add_to_builder(builder.item());
        let builder = self.email_verified.add_to_builder(builder.item());
        let builder = self.given_name.add_to_builder(builder.item());
        let builder = self.family_name.add_to_builder(builder.item());
        let builder = self.picture.add_to_builder(builder.item());
        let builder = self.preferred_username.add_to_builder(builder.item());
        let builder = self.claims.add_to_builder(builder.item());
        builder.finish()
    }

    fn add_to_type_builder<T: TypeNodeBuilder>(builder: T) -> T::Result {
        let builder = builder.record(
            Some("OidcPrincipal".to_string()),
            Some("golem:agent".to_string()),
        );
        let builder = <String>::add_to_type_builder(builder.field("sub"));
        let builder = <String>::add_to_type_builder(builder.field("issuer"));
        let builder = <Option<String>>::add_to_type_builder(builder.field("email"));
        let builder = <Option<String>>::add_to_type_builder(builder.field("name"));
        let builder = <Option<bool>>::add_to_type_builder(builder.field("email_verified"));
        let builder = <Option<String>>::add_to_type_builder(builder.field("given_name"));
        let builder = <Option<String>>::add_to_type_builder(builder.field("family_name"));
        let builder = <Option<String>>::add_to_type_builder(builder.field("picture"));
        let builder = <Option<String>>::add_to_type_builder(builder.field("preferred_username"));
        let builder = <String>::add_to_type_builder(builder.field("claims"));
        builder.finish()
    }
}

impl FromValueAndType for OidcPrincipal {
    fn from_extractor<'a, 'b>(
        extractor: &'a impl WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        let sub = <String>::from_extractor(
            &extractor
                .field(0)
                .ok_or_else(|| "Missing sub".to_string())?,
        )?;
        let issuer = <String>::from_extractor(
            &extractor
                .field(0)
                .ok_or_else(|| "Missing issuer".to_string())?,
        )?;
        let email = <Option<String>>::from_extractor(
            &extractor
                .field(0)
                .ok_or_else(|| "Missing email".to_string())?,
        )?;
        let name = <Option<String>>::from_extractor(
            &extractor
                .field(0)
                .ok_or_else(|| "Missing name".to_string())?,
        )?;
        let email_verified = <Option<bool>>::from_extractor(
            &extractor
                .field(0)
                .ok_or_else(|| "Missing email_verified".to_string())?,
        )?;
        let given_name = <Option<String>>::from_extractor(
            &extractor
                .field(0)
                .ok_or_else(|| "Missing given_name".to_string())?,
        )?;
        let family_name = <Option<String>>::from_extractor(
            &extractor
                .field(0)
                .ok_or_else(|| "Missing family_name".to_string())?,
        )?;
        let picture = <Option<String>>::from_extractor(
            &extractor
                .field(0)
                .ok_or_else(|| "Missing picture".to_string())?,
        )?;
        let preferred_username = <Option<String>>::from_extractor(
            &extractor
                .field(0)
                .ok_or_else(|| "Missing preferred_username".to_string())?,
        )?;
        let claims = <String>::from_extractor(
            &extractor
                .field(0)
                .ok_or_else(|| "Missing claims".to_string())?,
        )?;

        Ok(Self {
            sub,
            issuer,
            email,
            name,
            email_verified,
            given_name,
            family_name,
            picture,
            preferred_username,
            claims
        })
    }
}

impl IntoValue for AgentPrincipal {
    fn add_to_builder<T: NodeBuilder>(self, builder: T) -> T::Result {
        let builder = builder.record();
        let builder = self.agent_id.add_to_builder(builder.item());
        builder.finish()
    }

    fn add_to_type_builder<T: TypeNodeBuilder>(builder: T) -> T::Result {
        let builder = builder.record(
            Some("AgentPrincipal".to_string()),
            Some("golem:agent".to_string()),
        );
        let builder = <AgentId>::add_to_type_builder(builder.field("agent_id"));
        builder.finish()
    }
}

impl FromValueAndType for AgentPrincipal {
    fn from_extractor<'a, 'b>(
        extractor: &'a impl WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        let agent_id = <AgentId>::from_extractor(
            &extractor
                .field(0usize)
                .ok_or_else(|| "Missing agent_id field".to_string())?,
        )?;

        Ok(Self {
            agent_id
        })
    }
}

impl IntoValue for GolemUserPrincipal {
    fn add_to_builder<T: NodeBuilder>(self, builder: T) -> T::Result {
        let builder = builder.record();
        let builder = self.account_id.add_to_builder(builder.item());
        builder.finish()
    }

    fn add_to_type_builder<T: TypeNodeBuilder>(builder: T) -> T::Result {
        let builder = builder.record(
            Some("GolemUserPrincipal".to_string()),
            Some("golem:agent".to_string()),
        );
        let builder = <AccountId>::add_to_type_builder(builder.field("account_id"));
        builder.finish()
    }
}

impl FromValueAndType for GolemUserPrincipal {
    fn from_extractor<'a, 'b>(
        extractor: &'a impl WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        let account_id = <AccountId>::from_extractor(
            &extractor
                .field(0usize)
                .ok_or_else(|| "Missing account_id field".to_string())?,
        )?;

        Ok(Self {
            account_id
        })
    }
}

impl IntoValue for Principal {
    fn add_to_builder<T: NodeBuilder>(self, builder: T) -> T::Result {
        match self {
            Principal::Oidc(inner) => {
                let builder = builder.variant(0u32);
                inner.add_to_builder(builder).finish()
            }
            Principal::Agent(inner) => {
                let builder = builder.variant(1u32);
                inner.add_to_builder(builder).finish()
            }
            Principal::GolemUser(inner) => {
                let builder = builder.variant(2u32);
                inner.add_to_builder(builder).finish()
            }
            Principal::Anonymous => {
                builder.variant_unit(3u32)
            }
        }
    }

    fn add_to_type_builder<T: TypeNodeBuilder>(builder: T) -> T::Result {
        let builder = builder.variant(
            Some("Principal".to_string()),
            Some("golem:agent".to_string()),
        );
        let builder = <OidcPrincipal>::add_to_type_builder(builder.case("oidc"));
        let builder = <AgentPrincipal>::add_to_type_builder(builder.case("agent"));
        let builder = <GolemUserPrincipal>::add_to_type_builder(builder.case("golem-user"));
        let builder = builder.unit_case("anonymous");
        builder.finish()
    }
}

impl FromValueAndType for Principal {
    fn from_extractor<'a, 'b>(
        extractor: &'a impl WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        let (idx, inner) = extractor
            .variant()
            .ok_or_else(|| "Expected Principal to be a variant".to_string())?;
        match idx {
            0 => {
                let value = <OidcPrincipal>::from_extractor(
                    &inner.ok_or_else(|| "Missing Principal::Oidc body".to_string())?,
                )?;
                Ok(Principal::Oidc(value))
            }
            1 => {
                let value = <AgentPrincipal>::from_extractor(
                    &inner.ok_or_else(|| "Missing Principal::Agent body".to_string())?,
                )?;
                Ok(Principal::Agent(value))
            }
            2 => {
                let value = <GolemUserPrincipal>::from_extractor(
                    &inner.ok_or_else(|| "Missing Principal::GolemUser body".to_string())?,
                )?;
                Ok(Principal::GolemUser(value))
            }
            3 => {
                Ok(Principal::Anonymous)
            }
            _ => Err(format!(
                "Invalid Principal variant index: {}",
                idx
            ))
        }
    }
}
