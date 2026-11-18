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

use super::account::AccountId;
use super::component::ComponentId;
use super::{AgentFingerprint, AgentId, IdempotencyKey, InvocationId};
use std::fmt::{Display, Formatter};
use std::str::FromStr;
use uuid::Uuid;

/// The authenticated caller's idempotency namespace, not an authorization grant.
/// System and route external callers have no caller scope.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    Hash,
    serde::Serialize,
    serde::Deserialize,
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum IdempotencyScope {
    User(AccountId),
    Agent(AccountId),
    AdminImpersonation { admin: AccountId, target: AccountId },
}

impl Display for IdempotencyScope {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::User(account) => write!(f, "user:{account}"),
            Self::Agent(account) => write!(f, "agent:{account}"),
            Self::AdminImpersonation { admin, target } => {
                write!(f, "admin:{admin}:as:{target}")
            }
        }
    }
}

/// A logical request's key provenance, prepared before its actual owner and
/// fingerprint are known. Clone or transport this value to retain retry identity.
#[derive(
    Clone,
    Debug,
    PartialEq,
    Eq,
    Hash,
    serde::Serialize,
    serde::Deserialize,
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum InvocationRequestKey {
    External {
        scope: IdempotencyScope,
        key: IdempotencyKey,
    },
    Internal(IdempotencyKey),
    RouteSession {
        scope: String,
        key: IdempotencyKey,
    },
    Unkeyed(Uuid),
}

impl InvocationRequestKey {
    /// Ignore external keys without an authenticated scope. Allocate an unkeyed
    /// request's UUID exactly once, even when the caller supplies key text.
    pub fn external(scope: Option<IdempotencyScope>, key: Option<&IdempotencyKey>) -> Self {
        match (scope, key) {
            (Some(scope), Some(key)) => Self::External {
                scope,
                key: key.clone(),
            },
            _ => Self::Unkeyed(Uuid::new_v4()),
        }
    }

    pub fn finalize(
        &self,
        agent_id: AgentId,
        fingerprint: AgentFingerprint,
        operation: &str,
    ) -> OwnedInvocationId {
        match self {
            Self::External { scope, key } => OwnedInvocationId::external(
                agent_id,
                fingerprint,
                operation,
                Some(*scope),
                Some(key),
            ),
            Self::Internal(key) => {
                OwnedInvocationId::internal(agent_id, fingerprint, operation, key)
            }
            Self::RouteSession { scope, key } => {
                OwnedInvocationId::route_session(agent_id, fingerprint, operation, scope, key)
            }
            Self::Unkeyed(uuid) => OwnedInvocationId::new(agent_id, InvocationId(*uuid)),
        }
    }

    /// Normalize an ephemeral or virtual owner's seed without a fingerprint.
    /// Provenance, caller scope, operation and key all participate. Unkeyed
    /// requests use their retained UUID as the seed, not a fresh random value.
    pub fn owner_key(&self, operation: &str) -> IdempotencyKey {
        let (scope, key) = match self {
            Self::External { scope, key } => (scope.to_string(), key.value.as_bytes()),
            Self::Internal(key) => ("internal".to_string(), key.value.as_bytes()),
            Self::RouteSession { scope, key } => (route_scope(scope), key.value.as_bytes()),
            Self::Unkeyed(uuid) => ("unkeyed".to_string(), uuid.as_bytes().as_slice()),
        };
        derive_key(
            "golem-invocation-owner-key",
            &[scope.as_bytes(), operation.as_bytes(), key],
        )
    }
}

fn route_scope(scope: &str) -> String {
    format!("route:{scope}")
}

// Each field has a u64 big-endian byte length followed by its bytes.
fn derive_key(domain: &str, fields: &[&[u8]]) -> IdempotencyKey {
    let mut bytes = Vec::new();
    for field in fields {
        bytes.extend_from_slice(&(field.len() as u64).to_be_bytes());
        bytes.extend_from_slice(field);
    }
    IdempotencyKey::derived_from_bytes(&IdempotencyKey::from(domain), &bytes)
}

/// An agent-owned invocation, printed as `<component UUID>/<agent name>.<UUID>`.
/// Tool-owned transport identities are not represented by this type yet.
#[derive(
    Clone,
    Debug,
    PartialEq,
    Eq,
    Hash,
    serde::Serialize,
    serde::Deserialize,
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct OwnedInvocationId {
    agent_id: AgentId,
    id: InvocationId,
}

impl OwnedInvocationId {
    pub fn new(agent_id: AgentId, id: InvocationId) -> Self {
        Self { agent_id, id }
    }

    pub fn agent_id(&self) -> &AgentId {
        &self.agent_id
    }

    pub fn id(&self) -> InvocationId {
        self.id
    }

    pub fn uuid(&self) -> Uuid {
        self.id.0
    }

    /// External keys are deterministic only with an authenticated caller scope.
    /// Key text never grants internal trust, even when it looks like a derived UUID.
    pub fn external(
        agent_id: AgentId,
        fingerprint: AgentFingerprint,
        operation: &str,
        scope: Option<IdempotencyScope>,
        key: Option<&IdempotencyKey>,
    ) -> Self {
        match (scope, key) {
            (Some(scope), Some(key)) => {
                Self::derive(agent_id, fingerprint, &scope.to_string(), operation, key)
            }
            _ => Self::new(agent_id, InvocationId(Uuid::new_v4())),
        }
    }

    /// For keys derived by trusted internal code, never for an external request key.
    pub fn internal(
        agent_id: AgentId,
        fingerprint: AgentFingerprint,
        operation: &str,
        key: &IdempotencyKey,
    ) -> Self {
        Self::derive(agent_id, fingerprint, "internal", operation, key)
    }

    /// Durable route sessions require an explicit route namespace. This does not
    /// give route external callers an authenticated caller scope.
    pub fn route_session(
        agent_id: AgentId,
        fingerprint: AgentFingerprint,
        operation: &str,
        route_scope: &str,
        key: &IdempotencyKey,
    ) -> Self {
        Self::derive(
            agent_id,
            fingerprint,
            &self::route_scope(route_scope),
            operation,
            key,
        )
    }

    // Each field has a u64 big-endian byte length followed by its bytes. The
    // fingerprint is the raw 16-byte UUID; the other fields are UTF-8. Field order
    // is fingerprint, tagged scope, operation, key. The fixed base key supplies
    // the UUIDv5 namespace through IdempotencyKey::derived_from_bytes.
    fn derive(
        agent_id: AgentId,
        fingerprint: AgentFingerprint,
        scope: &str,
        operation: &str,
        key: &IdempotencyKey,
    ) -> Self {
        let derived = derive_key(
            "golem-invocation-id",
            &[
                fingerprint.0.as_bytes().as_slice(),
                scope.as_bytes(),
                operation.as_bytes(),
                key.value.as_bytes(),
            ],
        );
        Self::new(
            agent_id,
            InvocationId(
                Uuid::parse_str(&derived.value).expect("derived idempotency keys are UUIDs"),
            ),
        )
    }
}

impl Display for OwnedInvocationId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}", self.agent_id, self.id)
    }
}

impl FromStr for OwnedInvocationId {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let invalid =
            || "invalid invocation id: expected <component UUID>/<agent name>.<UUID>".to_string();
        let suffix_start = value.len().checked_sub(37).ok_or_else(invalid)?;
        let owner = value.get(..suffix_start).ok_or_else(invalid)?;
        let suffix = value.get(suffix_start..).ok_or_else(invalid)?;
        let uuid_text = suffix.strip_prefix('.').ok_or_else(invalid)?;
        let uuid = uuid_text.parse::<InvocationId>().map_err(|_| invalid())?;
        // AgentId::FromStr accepts a colon, but its Display uses a slash.
        // Split only once: slashes and dots can occur inside the agent name.
        let (component, name) = owner.split_once('/').ok_or_else(invalid)?;
        let component_id = ComponentId(Uuid::parse_str(component).map_err(|_| invalid())?);
        Ok(Self::new(
            AgentId {
                component_id,
                agent_id: name.to_string(),
            },
            uuid,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    fn owner() -> AgentId {
        AgentId {
            component_id: ComponentId(Uuid::from_u128(1)),
            agent_id: "counter(\"a.b/c:雪\")".to_string(),
        }
    }

    fn fingerprint() -> AgentFingerprint {
        AgentFingerprint(Uuid::from_u128(2))
    }

    #[test]
    fn prepared_request_retains_unkeyed_uuid_and_ignores_unscoped_keys() {
        let key = IdempotencyKey::from("same supplied key");
        for (scope, supplied) in [
            (None, Some(&key)),
            (
                Some(IdempotencyScope::User(AccountId(Uuid::from_u128(10)))),
                None,
            ),
            (None, None),
        ] {
            let prepared = InvocationRequestKey::external(scope, supplied);
            let InvocationRequestKey::Unkeyed(seed) = prepared else {
                panic!("missing scope or key must prepare an unkeyed request")
            };
            assert_eq!(seed.get_version_num(), 4);
            assert_ne!(prepared, InvocationRequestKey::external(scope, supplied));
            let first = prepared.finalize(owner(), fingerprint(), "雪");
            assert_eq!(first.uuid(), seed);
            assert_eq!(first, prepared.finalize(owner(), fingerprint(), "雪"));
            assert_eq!(
                prepared
                    .finalize(owner(), AgentFingerprint(Uuid::from_u128(99)), "other")
                    .uuid(),
                seed
            );
            let normalized = prepared.owner_key("雪");
            assert_eq!(normalized, prepared.clone().owner_key("雪"));
            assert_ne!(normalized, prepared.owner_key("other"));
            assert_ne!(
                normalized,
                InvocationRequestKey::external(scope, supplied).owner_key("雪")
            );
            assert_ne!(
                normalized,
                InvocationRequestKey::Internal(IdempotencyKey::from_uuid(seed)).owner_key("雪")
            );
        }
    }

    #[test]
    fn prepared_request_finalizes_with_existing_factories_and_normalizes_without_fingerprint() {
        let account = AccountId(Uuid::from_u128(10));
        let key = IdempotencyKey::from("key/雪🦀");
        let scope = IdempotencyScope::User(account);
        let requests = [
            InvocationRequestKey::external(Some(scope), Some(&key)),
            InvocationRequestKey::Internal(key.clone()),
            InvocationRequestKey::RouteSession {
                scope: "internal".to_string(),
                key: key.clone(),
            },
            InvocationRequestKey::External {
                scope: IdempotencyScope::Agent(account),
                key: key.clone(),
            },
            InvocationRequestKey::External {
                scope: IdempotencyScope::User(AccountId(Uuid::from_u128(11))),
                key: key.clone(),
            },
            InvocationRequestKey::External {
                scope: IdempotencyScope::AdminImpersonation {
                    admin: account,
                    target: account,
                },
                key: key.clone(),
            },
        ];
        let expected = [
            OwnedInvocationId::external(owner(), fingerprint(), "method", Some(scope), Some(&key)),
            OwnedInvocationId::internal(owner(), fingerprint(), "method", &key),
            OwnedInvocationId::route_session(owner(), fingerprint(), "method", "internal", &key),
        ];
        for (request, expected) in requests.iter().zip(expected) {
            assert_eq!(request.finalize(owner(), fingerprint(), "method"), expected);
        }
        for (index, request) in requests.iter().enumerate() {
            let id = request.finalize(owner(), fingerprint(), "method");
            let normalized = request.owner_key("method");
            assert_eq!(id, request.finalize(owner(), fingerprint(), "method"));
            assert_ne!(
                id.uuid(),
                request
                    .finalize(owner(), AgentFingerprint(Uuid::from_u128(99)), "method")
                    .uuid()
            );
            assert_eq!(normalized, request.owner_key("method"));
            assert_ne!(normalized.value, id.uuid().to_string());
            assert_ne!(normalized, request.owner_key("other"));
            for other in &requests[index + 1..] {
                assert_ne!(
                    id.uuid(),
                    other.finalize(owner(), fingerprint(), "method").uuid()
                );
                assert_ne!(normalized, other.owner_key("method"));
            }
        }
        assert_ne!(
            requests[0].owner_key("method"),
            InvocationRequestKey::External {
                scope,
                key: IdempotencyKey::from("other")
            }
            .owner_key("method")
        );
    }

    #[test]
    fn prepared_request_unicode_field_boundaries_are_unambiguous() {
        let route = |scope: &str, key: &str| InvocationRequestKey::RouteSession {
            scope: scope.to_string(),
            key: IdempotencyKey::from(key),
        };
        for ((first, first_op), (second, second_op)) in [
            ((route("雪🦀", "key"), "é"), (route("雪", "key"), "🦀é")),
            ((route("雪", "ékey"), "🦀"), (route("雪", "key"), "🦀é")),
            ((route("", "雪"), ""), (route("雪", ""), "")),
        ] {
            assert_ne!(first.owner_key(first_op), second.owner_key(second_op));
            assert_ne!(
                first.finalize(owner(), fingerprint(), first_op).uuid(),
                second.finalize(owner(), fingerprint(), second_op).uuid()
            );
        }
    }

    #[test]
    fn prepared_request_serde_roundtrip_preserves_identity() {
        let key = IdempotencyKey::from("雪");
        for request in [
            InvocationRequestKey::external(
                Some(IdempotencyScope::User(AccountId(Uuid::from_u128(10)))),
                Some(&key),
            ),
            InvocationRequestKey::External {
                scope: IdempotencyScope::Agent(AccountId(Uuid::from_u128(10))),
                key: key.clone(),
            },
            InvocationRequestKey::External {
                scope: IdempotencyScope::AdminImpersonation {
                    admin: AccountId(Uuid::from_u128(10)),
                    target: AccountId(Uuid::from_u128(11)),
                },
                key: key.clone(),
            },
            InvocationRequestKey::Internal(key.clone()),
            InvocationRequestKey::RouteSession {
                scope: "雪".to_string(),
                key,
            },
            InvocationRequestKey::external(None, None),
        ] {
            let json = serde_json::to_string(&request).unwrap();
            let decoded: InvocationRequestKey = serde_json::from_str(&json).unwrap();
            assert_eq!(request, decoded);
            #[cfg(feature = "full")]
            {
                let bytes = desert_rust::serialize_to_byte_vec(&request).unwrap();
                let decoded: InvocationRequestKey = desert_rust::deserialize(&bytes).unwrap();
                assert_eq!(request, decoded);
            }
            assert_eq!(request.owner_key("method"), decoded.owner_key("method"));
            assert_eq!(
                request.finalize(owner(), fingerprint(), "method"),
                decoded.finalize(owner(), fingerprint(), "method")
            );
        }
    }

    #[test]
    fn invocation_id_text_roundtrip_and_golden_vector() {
        let id = OwnedInvocationId::new(owner(), InvocationId(Uuid::from_u128(3)));
        let text = "00000000-0000-0000-0000-000000000001/counter(\"a.b/c:雪\").00000000-0000-0000-0000-000000000003";
        assert_eq!(id.to_string(), text);
        assert_eq!(text.parse::<OwnedInvocationId>().unwrap(), id);
        assert_eq!(id.agent_id(), &owner());
        assert_eq!(id.uuid(), Uuid::from_u128(3));
        assert_eq!(id.id(), InvocationId(Uuid::from_u128(3)));
        for name in ["", ".", "/", "\"", "雪", "a.b/c.d/\"雪\"", "é🦀."] {
            let mut agent = owner();
            agent.agent_id = name.to_string();
            let id = OwnedInvocationId::new(agent, InvocationId(Uuid::from_u128(4)));
            assert_eq!(id.to_string().parse::<OwnedInvocationId>().unwrap(), id);
        }
    }

    #[test]
    fn invocation_id_rejects_invalid_suffixes_without_unicode_panics() {
        for text in [
            "",
            ".",
            "雪",
            "é00000000-0000-0000-0000-000000000003",
            "雪.00000000-0000-0000-0000-000000000003",
            "00000000-0000-0000-0000-000000000001:name.00000000-0000-0000-0000-000000000003",
        ] {
            assert!(text.parse::<OwnedInvocationId>().is_err(), "{text}");
        }
        let valid = OwnedInvocationId::new(owner(), InvocationId(Uuid::from_u128(3))).to_string();
        for invalid in [
            format!("{valid}x"),
            valid[..valid.len() - 1].to_string(),
            valid.replace(".00000000", "!00000000"),
        ] {
            assert!(invalid.parse::<OwnedInvocationId>().is_err());
        }
        // Every possible offset through a multi-byte codepoint at the suffix boundary.
        for count in 0..40 {
            let text = format!("🦀{}", "x".repeat(count));
            assert!(text.parse::<OwnedInvocationId>().is_err());
        }
    }

    #[test]
    fn invocation_id_external_derivation_separates_all_fields_and_scopes() {
        let account = AccountId(Uuid::from_u128(10));
        let scope = IdempotencyScope::User(account);
        let key = IdempotencyKey::from("c");
        let derive = |fingerprint, scope, operation, key: &IdempotencyKey| {
            OwnedInvocationId::external(owner(), fingerprint, operation, Some(scope), Some(key))
        };
        let id = derive(fingerprint(), scope, "ab", &key);
        assert_eq!(id, derive(fingerprint(), scope, "ab", &key));
        assert_eq!(id.uuid().get_version_num(), 5);
        assert_eq!(
            id.uuid().to_string(),
            "3915410d-393e-5715-bb09-82c9bd3416f3"
        );
        for other in [
            derive(AgentFingerprint(Uuid::from_u128(3)), scope, "ab", &key),
            derive(fingerprint(), scope, "a", &IdempotencyKey::from("bc")),
            derive(fingerprint(), scope, "ab", &IdempotencyKey::from("d")),
            derive(
                fingerprint(),
                IdempotencyScope::User(AccountId(Uuid::from_u128(11))),
                "ab",
                &key,
            ),
            derive(fingerprint(), IdempotencyScope::Agent(account), "ab", &key),
            derive(
                fingerprint(),
                IdempotencyScope::AdminImpersonation {
                    admin: account,
                    target: account,
                },
                "ab",
                &key,
            ),
            OwnedInvocationId::internal(owner(), fingerprint(), "ab", &key),
            OwnedInvocationId::route_session(
                owner(),
                fingerprint(),
                "ab",
                "user:00000000-0000-0000-0000-00000000000a",
                &key,
            ),
        ] {
            assert_ne!(id.uuid(), other.uuid());
        }
    }

    #[test]
    fn invocation_id_missing_external_key_or_scope_is_random() {
        let key = IdempotencyKey::from("internal");
        let derived = IdempotencyKey::derived_from_bytes(&key, b"trusted-looking");
        for (scope, key) in [
            (None, Some(&key)),
            (None, Some(&derived)),
            (Some(IdempotencyScope::User(AccountId(Uuid::nil()))), None),
            (None, None),
        ] {
            let first = OwnedInvocationId::external(owner(), fingerprint(), "method", scope, key);
            let second = OwnedInvocationId::external(owner(), fingerprint(), "method", scope, key);
            assert_ne!(first, second);
            assert_eq!(first.uuid().get_version_num(), 4);
        }
    }

    #[test]
    fn invocation_id_internal_and_route_derivation_are_explicit_and_deterministic() {
        let key = IdempotencyKey::from("a.long/key\"雪");
        let internal = OwnedInvocationId::internal(owner(), fingerprint(), "method", &key);
        assert_eq!(
            internal,
            OwnedInvocationId::internal(owner(), fingerprint(), "method", &key)
        );
        assert_eq!(
            internal.uuid().to_string(),
            "ae2bb760-5a90-5e2c-86ac-d4158cfc0ec8"
        );
        assert_ne!(
            internal,
            OwnedInvocationId::internal(owner(), fingerprint(), "other", &key)
        );
        assert_ne!(
            internal,
            OwnedInvocationId::internal(
                owner(),
                AgentFingerprint(Uuid::from_u128(3)),
                "method",
                &key
            )
        );
        assert_ne!(
            internal,
            OwnedInvocationId::internal(
                owner(),
                fingerprint(),
                "method",
                &IdempotencyKey::from("other")
            )
        );
        let route = OwnedInvocationId::route_session(owner(), fingerprint(), "method", "ab", &key);
        assert_eq!(
            route,
            OwnedInvocationId::route_session(owner(), fingerprint(), "method", "ab", &key)
        );
        assert_eq!(
            route.uuid().to_string(),
            "643850d7-cb1c-55f7-b1e5-833b08295140"
        );
        assert_ne!(internal, route);
        assert_ne!(
            route,
            OwnedInvocationId::route_session(owner(), fingerprint(), "method", "a", &key)
        );
        assert_ne!(
            OwnedInvocationId::route_session(owner(), fingerprint(), "c", "ab", &key),
            OwnedInvocationId::route_session(owner(), fingerprint(), "bc", "a", &key),
        );
        assert_eq!(
            internal.to_string().parse::<OwnedInvocationId>().unwrap(),
            internal
        );
    }

    #[test]
    fn invocation_id_scope_encoding_and_admin_boundaries() {
        let admin = AccountId(Uuid::from_u128(10));
        let target = AccountId(Uuid::from_u128(11));
        assert_eq!(
            IdempotencyScope::User(admin).to_string(),
            "user:00000000-0000-0000-0000-00000000000a"
        );
        assert_eq!(
            IdempotencyScope::Agent(admin).to_string(),
            "agent:00000000-0000-0000-0000-00000000000a"
        );
        let scope = IdempotencyScope::AdminImpersonation { admin, target };
        assert_eq!(
            scope.to_string(),
            "admin:00000000-0000-0000-0000-00000000000a:as:00000000-0000-0000-0000-00000000000b"
        );
        let derive = |scope| {
            OwnedInvocationId::external(
                owner(),
                fingerprint(),
                "method",
                Some(scope),
                Some(&IdempotencyKey::from("key")),
            )
        };
        assert_ne!(
            derive(scope),
            derive(IdempotencyScope::AdminImpersonation {
                admin: target,
                target
            })
        );
        assert_ne!(
            derive(scope),
            derive(IdempotencyScope::AdminImpersonation {
                admin,
                target: admin
            })
        );
    }

    #[test]
    fn invocation_id_all_scope_roles_are_disjoint_even_for_derived_key_text() {
        let account = AccountId(Uuid::from_u128(10));
        let key = IdempotencyKey::derived_from_bytes(&IdempotencyKey::from("internal"), b"key");
        let mut ids = vec![
            OwnedInvocationId::internal(owner(), fingerprint(), "method", &key),
            OwnedInvocationId::route_session(owner(), fingerprint(), "method", "internal", &key),
        ];
        for scope in [
            IdempotencyScope::User(account),
            IdempotencyScope::Agent(account),
            IdempotencyScope::AdminImpersonation {
                admin: account,
                target: account,
            },
        ] {
            ids.push(OwnedInvocationId::external(
                owner(),
                fingerprint(),
                "method",
                Some(scope),
                Some(&key),
            ));
        }
        for (index, id) in ids.iter().enumerate() {
            for other in &ids[index + 1..] {
                assert_ne!(id.uuid(), other.uuid());
            }
        }
    }
}
