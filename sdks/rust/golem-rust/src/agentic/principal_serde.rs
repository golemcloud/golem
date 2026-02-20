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

use crate::golem_agentic::golem::agent::common::{
    AgentPrincipal, GolemUserPrincipal, OidcPrincipal, Principal,
};
use golem_wasm::golem_core_1_5_x::types::{AccountId, AgentId, ComponentId, Uuid};
use serde::de::{self, MapAccess, Visitor};
use serde::ser::SerializeStruct;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

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
