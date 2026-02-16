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
use golem_wasm::golem_rpc_0_2_x::types::{AccountId, AgentId, ComponentId, Uuid};
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
                s.serialize_field("type", "anonymous")?;
                s.end()
            }
            Principal::Oidc(oidc) => {
                let mut s = serializer.serialize_struct("Principal", 11)?;
                s.serialize_field("type", "oidc")?;
                s.serialize_field("sub", &oidc.sub)?;
                s.serialize_field("issuer", &oidc.issuer)?;
                s.serialize_field("email", &oidc.email)?;
                s.serialize_field("name", &oidc.name)?;
                s.serialize_field("emailVerified", &oidc.email_verified)?;
                s.serialize_field("givenName", &oidc.given_name)?;
                s.serialize_field("familyName", &oidc.family_name)?;
                s.serialize_field("picture", &oidc.picture)?;
                s.serialize_field("preferredUsername", &oidc.preferred_username)?;
                s.serialize_field("claims", &oidc.claims)?;
                s.end()
            }
            Principal::Agent(agent) => {
                let mut s = serializer.serialize_struct("Principal", 3)?;
                s.serialize_field("type", "agent")?;
                s.serialize_field(
                    "componentId",
                    &uuid_to_string(&agent.agent_id.component_id.uuid),
                )?;
                s.serialize_field("agentId", &agent.agent_id.agent_id)?;
                s.end()
            }
            Principal::GolemUser(user) => {
                let mut s = serializer.serialize_struct("Principal", 2)?;
                s.serialize_field("type", "golemUser")?;
                s.serialize_field("accountId", &uuid_to_string(&user.account_id.uuid))?;
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
                formatter.write_str("a Principal object with a 'type' field")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut type_field: Option<String> = None;
                let mut sub: Option<String> = None;
                let mut issuer: Option<String> = None;
                let mut email: Option<String> = None;
                let mut name: Option<String> = None;
                let mut email_verified: Option<bool> = None;
                let mut given_name: Option<String> = None;
                let mut family_name: Option<String> = None;
                let mut picture: Option<String> = None;
                let mut preferred_username: Option<String> = None;
                let mut claims: Option<String> = None;
                let mut component_id: Option<String> = None;
                let mut agent_id: Option<String> = None;
                let mut account_id: Option<String> = None;

                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "type" => type_field = Some(map.next_value()?),
                        "sub" => sub = Some(map.next_value()?),
                        "issuer" => issuer = Some(map.next_value()?),
                        "email" => email = map.next_value()?,
                        "name" => name = map.next_value()?,
                        "emailVerified" => email_verified = map.next_value()?,
                        "givenName" => given_name = map.next_value()?,
                        "familyName" => family_name = map.next_value()?,
                        "picture" => picture = map.next_value()?,
                        "preferredUsername" => preferred_username = map.next_value()?,
                        "claims" => claims = Some(map.next_value()?),
                        "componentId" => component_id = Some(map.next_value()?),
                        "agentId" => agent_id = Some(map.next_value()?),
                        "accountId" => account_id = Some(map.next_value()?),
                        _ => {
                            let _ = map.next_value::<serde::de::IgnoredAny>()?;
                        }
                    }
                }

                let type_str = type_field.ok_or_else(|| de::Error::missing_field("type"))?;

                match type_str.as_str() {
                    "anonymous" => Ok(Principal::Anonymous),
                    "oidc" => Ok(Principal::Oidc(OidcPrincipal {
                        sub: sub.ok_or_else(|| de::Error::missing_field("sub"))?,
                        issuer: issuer.ok_or_else(|| de::Error::missing_field("issuer"))?,
                        email,
                        name,
                        email_verified,
                        given_name,
                        family_name,
                        picture,
                        preferred_username,
                        claims: claims.ok_or_else(|| de::Error::missing_field("claims"))?,
                    })),
                    "agent" => {
                        let cid =
                            component_id.ok_or_else(|| de::Error::missing_field("componentId"))?;
                        let uuid = uuid_from_string(&cid).map_err(de::Error::custom)?;
                        Ok(Principal::Agent(AgentPrincipal {
                            agent_id: AgentId {
                                component_id: ComponentId { uuid },
                                agent_id: agent_id
                                    .ok_or_else(|| de::Error::missing_field("agentId"))?,
                            },
                        }))
                    }
                    "golemUser" => {
                        let aid =
                            account_id.ok_or_else(|| de::Error::missing_field("accountId"))?;
                        let uuid = uuid_from_string(&aid).map_err(de::Error::custom)?;
                        Ok(Principal::GolemUser(GolemUserPrincipal {
                            account_id: AccountId { uuid },
                        }))
                    }
                    other => Err(de::Error::unknown_variant(
                        other,
                        &["anonymous", "oidc", "agent", "golemUser"],
                    )),
                }
            }
        }

        deserializer.deserialize_map(PrincipalVisitor)
    }
}
