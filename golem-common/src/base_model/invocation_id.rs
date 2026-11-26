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
use super::caller::Caller;
use super::{AgentFingerprint, AgentId, IdempotencyKey, InvocationId};
use std::collections::BTreeSet;
use std::fmt::{Display, Formatter};
use std::str::FromStr;
use uuid::Uuid;

impl InvocationId {
    /// The constructor belongs to the persisted agent incarnation, not a creating request.
    pub fn constructor(fingerprint: AgentFingerprint) -> Self {
        Self(Uuid::new_v5(&fingerprint.0, b"constructor"))
    }
}

/// The caller scope of an idempotency key: who sent the keyed request. It is a namespace for
/// the caller's keys, not an authorization grant. A request has a scope only when Golem knows
/// its original caller, see [`IdempotencyScope::of`].
///
/// [`IdempotencyScope::encoded`] is the only encoding of a scope. Every derivation that depends
/// on the caller hashes it: invocation IDs, the agent IDs of creates with an idempotency key,
/// ephemeral agent IDs and external-tool-owner agent IDs.
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
pub enum IdempotencyScope {
    /// The pair of the original caller.
    Pair { issuer: String, subject: String },
    /// An admin that acts as another account. The request records the other account as its
    /// original caller, so the admin's account keeps the admin's keys apart from the keys that
    /// the account sends itself.
    AdminImpersonation {
        admin: AccountId,
        issuer: String,
        subject: String,
    },
    /// An anonymous caller: the IDs of the tokens that the request presented, in ascending
    /// order, each ID one time.
    Tokens(Vec<Uuid>),
}

impl IdempotencyScope {
    const PAIR_TAG: u8 = 1;
    const ADMIN_IMPERSONATION_TAG: u8 = 2;
    const TOKENS_TAG: u8 = 3;

    /// The scope of the keys that `caller` sends: the pair of its original principal. An
    /// anonymous caller has the set of its presented tokens as its scope, and without a token
    /// it has no scope, so its keys never deduplicate. The immediate principal, the kind of a
    /// principal and its claims are not part of the scope.
    pub fn of(caller: &Caller) -> Option<Self> {
        match caller.original.pair() {
            Some(pair) => Some(Self::Pair {
                issuer: pair.issuer,
                subject: pair.subject,
            }),
            None => Self::tokens(caller.tokens.iter().map(|token| token.id)),
        }
    }

    /// The scope of an admin with the account `admin` that acts as another account, where
    /// `caller` is the caller that the request records.
    pub fn admin_impersonation(admin: AccountId, caller: &Caller) -> Option<Self> {
        let pair = caller.original.pair()?;
        Some(Self::AdminImpersonation {
            admin,
            issuer: pair.issuer,
            subject: pair.subject,
        })
    }

    /// The scope of an anonymous caller that presented the tokens `ids`, or none when it
    /// presented no token.
    pub fn tokens(ids: impl IntoIterator<Item = Uuid>) -> Option<Self> {
        let ids: BTreeSet<Uuid> = ids.into_iter().collect();
        (!ids.is_empty()).then(|| Self::Tokens(ids.into_iter().collect()))
    }

    /// The bytes that stand for this scope in every derivation: a tag byte, then the fields.
    /// A string has a u64 big-endian byte length before its UTF-8 bytes, and an ID is its 16
    /// bytes. A token set has a u64 big-endian count before its IDs, which are in ascending
    /// order, each one time.
    ///
    /// The other request kinds have a text tag in the place of a scope (`internal`, `route:`
    /// and `unkeyed`). A scope starts with a control byte, so it is never equal to one of them.
    pub fn encoded(&self) -> Vec<u8> {
        fn string(bytes: &mut Vec<u8>, value: &str) {
            bytes.extend_from_slice(&(value.len() as u64).to_be_bytes());
            bytes.extend_from_slice(value.as_bytes());
        }

        let mut bytes = Vec::new();
        match self {
            Self::Pair { issuer, subject } => {
                bytes.push(Self::PAIR_TAG);
                string(&mut bytes, issuer);
                string(&mut bytes, subject);
            }
            Self::AdminImpersonation {
                admin,
                issuer,
                subject,
            } => {
                bytes.push(Self::ADMIN_IMPERSONATION_TAG);
                bytes.extend_from_slice(admin.0.as_bytes());
                string(&mut bytes, issuer);
                string(&mut bytes, subject);
            }
            Self::Tokens(ids) => {
                let ids: BTreeSet<&Uuid> = ids.iter().collect();
                bytes.push(Self::TOKENS_TAG);
                bytes.extend_from_slice(&(ids.len() as u64).to_be_bytes());
                for id in ids {
                    bytes.extend_from_slice(id.as_bytes());
                }
            }
        }
        bytes
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
                Some(scope.clone()),
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
            Self::External { scope, key } => (scope.encoded(), key.value.as_bytes()),
            Self::Internal(key) => (INTERNAL_SCOPE.to_vec(), key.value.as_bytes()),
            Self::RouteSession { scope, key } => {
                (route_scope(scope).into_bytes(), key.value.as_bytes())
            }
            Self::Unkeyed(uuid) => (b"unkeyed".to_vec(), uuid.as_bytes().as_slice()),
        };
        derive_key(
            "golem-invocation-owner-key",
            &[&scope, operation.as_bytes(), key],
        )
    }
}

const INTERNAL_SCOPE: &[u8] = b"internal";

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

/// An agent-owned invocation, printed as `<agent ID>.<UUID>`.
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
                Self::derive(agent_id, fingerprint, &scope.encoded(), operation, key)
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
        Self::derive(agent_id, fingerprint, INTERNAL_SCOPE, operation, key)
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
            self::route_scope(route_scope).as_bytes(),
            operation,
            key,
        )
    }

    // Each field has a u64 big-endian byte length followed by its bytes. The
    // fingerprint is the raw 16-byte UUID. The scope is the encoding of a caller scope
    // (`IdempotencyScope::encoded`) or the text tag of another request kind. The operation
    // and the key are UTF-8. Field order is fingerprint, scope, operation, key. The fixed
    // base key supplies the UUIDv5 namespace through IdempotencyKey::derived_from_bytes.
    fn derive(
        agent_id: AgentId,
        fingerprint: AgentFingerprint,
        scope: &[u8],
        operation: &str,
        key: &IdempotencyKey,
    ) -> Self {
        let derived = derive_key(
            "golem-invocation-id",
            &[
                fingerprint.0.as_bytes().as_slice(),
                scope,
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
        let invalid = || "invalid invocation id: expected <agent ID>.<UUID>".to_string();
        let suffix_start = value.len().checked_sub(37).ok_or_else(invalid)?;
        let owner = value.get(..suffix_start).ok_or_else(invalid)?;
        let suffix = value.get(suffix_start..).ok_or_else(invalid)?;
        let uuid_text = suffix.strip_prefix('.').ok_or_else(invalid)?;
        let uuid = uuid_text.parse::<InvocationId>().map_err(|_| invalid())?;
        let agent_id = owner.parse::<AgentId>().map_err(|_| invalid())?;
        Ok(Self::new(agent_id, uuid))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base_model::Timestamp;
    use crate::base_model::caller::test_support::{
        account_principal, agent_principal, sample_caller, user_principal,
    };
    use crate::base_model::caller::{
        Claims, Pair, Principal, PrincipalRef, TokenProvenance, UserPrincipal,
    };
    use test_r::test;

    fn owner() -> AgentId {
        AgentId(uuid::uuid!("00000007-0cfd-8d08-825f-b29b4e7457ad"))
    }

    fn fingerprint() -> AgentFingerprint {
        AgentFingerprint(Uuid::from_u128(2))
    }

    fn pair(issuer: &str, subject: &str) -> IdempotencyScope {
        IdempotencyScope::Pair {
            issuer: issuer.to_string(),
            subject: subject.to_string(),
        }
    }

    fn alice() -> IdempotencyScope {
        pair("https://issuer.example", "alice")
    }

    fn admin_as_alice(admin: u128) -> IdempotencyScope {
        IdempotencyScope::AdminImpersonation {
            admin: AccountId(Uuid::from_u128(admin)),
            issuer: "https://issuer.example".to_string(),
            subject: "alice".to_string(),
        }
    }

    fn tokens(ids: &[u128]) -> IdempotencyScope {
        IdempotencyScope::tokens(ids.iter().copied().map(Uuid::from_u128))
            .expect("a caller with a token has a scope")
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    fn anonymous_with_tokens(ids: &[u128]) -> Caller {
        Caller {
            original: Principal::Anonymous,
            immediate: Principal::Anonymous,
            tokens: ids
                .iter()
                .map(|id| TokenProvenance {
                    id: Uuid::from_u128(*id),
                    maker: None,
                })
                .collect(),
        }
    }

    #[test]
    fn constructor_identity_is_incarnation_bound_and_separate_from_methods() {
        let constructor = InvocationId::constructor(fingerprint());
        assert_eq!(
            constructor.0,
            uuid::uuid!("f8d406b8-7838-5397-8385-e22008a2ddcf")
        );
        assert_eq!(constructor, InvocationId::constructor(fingerprint()));
        assert_eq!(
            InvocationId::constructor(AgentFingerprint(Uuid::from_u128(99))).0,
            uuid::uuid!("c1e4f2e6-4312-506e-b331-7f0e517fadbf")
        );
        for operation in ["constructor", "constructor/雪:internal"] {
            for key in ["constructor", "constructor/雪:internal"] {
                let key = IdempotencyKey::from(key);
                for method in [
                    OwnedInvocationId::external(
                        owner(),
                        fingerprint(),
                        operation,
                        Some(IdempotencyScope::User(AccountId(Uuid::from_u128(10)))),
                        Some(&key),
                    ),
                    OwnedInvocationId::internal(owner(), fingerprint(), operation, &key),
                    OwnedInvocationId::route_session(
                        owner(),
                        fingerprint(),
                        operation,
                        "constructor",
                        &key,
                    ),
                ] {
                    assert_ne!(constructor, method.id());
                }
            }
        }
        let unkeyed =
            OwnedInvocationId::external(owner(), fingerprint(), "constructor", None, None);
        assert_eq!(unkeyed.uuid().get_version_num(), 4);
        assert_eq!(constructor.0.get_version_num(), 5);
    }

    #[test]
    fn prepared_request_retains_unkeyed_uuid_and_ignores_unscoped_keys() {
        let key = IdempotencyKey::from("same supplied key");
        for (scope, supplied) in [(None, Some(&key)), (Some(alice()), None), (None, None)] {
            let prepared = InvocationRequestKey::external(scope.clone(), supplied);
            let InvocationRequestKey::Unkeyed(seed) = prepared else {
                panic!("missing scope or key must prepare an unkeyed request")
            };
            assert_eq!(seed.get_version_num(), 4);
            assert_ne!(
                prepared,
                InvocationRequestKey::external(scope.clone(), supplied)
            );
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
        let key = IdempotencyKey::from("key/雪🦀");
        let scope = alice();
        let requests = [
            InvocationRequestKey::external(Some(scope.clone()), Some(&key)),
            InvocationRequestKey::Internal(key.clone()),
            InvocationRequestKey::RouteSession {
                scope: "internal".to_string(),
                key: key.clone(),
            },
            InvocationRequestKey::External {
                scope: pair("https://issuer.example", "bob"),
                key: key.clone(),
            },
            InvocationRequestKey::External {
                scope: pair("https://other.example", "alice"),
                key: key.clone(),
            },
            InvocationRequestKey::External {
                scope: admin_as_alice(10),
                key: key.clone(),
            },
            InvocationRequestKey::External {
                scope: tokens(&[5, 7]),
                key: key.clone(),
            },
        ];
        let expected = [
            OwnedInvocationId::external(
                owner(),
                fingerprint(),
                "method",
                Some(scope.clone()),
                Some(&key),
            ),
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
            InvocationRequestKey::external(Some(pair("https://雪.example", "雪🦀")), Some(&key)),
            InvocationRequestKey::External {
                scope: admin_as_alice(10),
                key: key.clone(),
            },
            InvocationRequestKey::External {
                scope: tokens(&[5, 7]),
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
        let text = "00000007-0cfd-8d08-825f-b29b4e7457ad.00000000-0000-0000-0000-000000000003";
        assert_eq!(id.to_string(), text);
        assert_eq!(text.parse::<OwnedInvocationId>().unwrap(), id);
        assert_eq!(id.agent_id(), &owner());
        assert_eq!(id.uuid(), Uuid::from_u128(3));
        assert_eq!(id.id(), InvocationId(Uuid::from_u128(3)));
    }

    #[test]
    fn invocation_id_rejects_invalid_suffixes_without_unicode_panics() {
        for text in [
            "",
            ".",
            "雪",
            "é00000000-0000-0000-0000-000000000003",
            "雪.00000000-0000-0000-0000-000000000003",
            "00000000-0000-0000-0000-000000000001/name.00000000-0000-0000-0000-000000000003",
            "counter(\"a\").00000000-0000-0000-0000-000000000003",
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

    /// The expected IDs were computed independently of this module, from the layout in the
    /// documentation of [`IdempotencyScope::encoded`] and of `OwnedInvocationId::derive`.
    #[test]
    fn invocation_id_external_derivation_separates_all_fields_and_scopes() {
        let scope = alice();
        let key = IdempotencyKey::from("c");
        let derive = |fingerprint, scope: &IdempotencyScope, operation, key: &IdempotencyKey| {
            OwnedInvocationId::external(
                owner(),
                fingerprint,
                operation,
                Some(scope.clone()),
                Some(key),
            )
        };
        let id = derive(fingerprint(), &scope, "ab", &key);
        assert_eq!(id, derive(fingerprint(), &scope, "ab", &key));
        assert_eq!(id.uuid().get_version_num(), 5);
        assert_eq!(
            id.uuid().to_string(),
            "8215ed03-b675-5fb0-8706-739c412f60b0"
        );
        assert_eq!(
            derive(fingerprint(), &admin_as_alice(10), "ab", &key)
                .uuid()
                .to_string(),
            "559b0013-1130-5f4d-9ad6-e7b8e9dcd24e"
        );
        assert_eq!(
            derive(fingerprint(), &tokens(&[7, 5]), "ab", &key)
                .uuid()
                .to_string(),
            "6a5dac75-59b5-5b5d-a55e-6642c39bdd22"
        );
        for other in [
            derive(AgentFingerprint(Uuid::from_u128(3)), &scope, "ab", &key),
            derive(fingerprint(), &scope, "a", &IdempotencyKey::from("bc")),
            derive(fingerprint(), &scope, "ab", &IdempotencyKey::from("d")),
            derive(
                fingerprint(),
                &pair("https://issuer.example", "bob"),
                "ab",
                &key,
            ),
            derive(
                fingerprint(),
                &pair("https://other.example", "alice"),
                "ab",
                &key,
            ),
            derive(fingerprint(), &admin_as_alice(10), "ab", &key),
            derive(fingerprint(), &tokens(&[5, 7]), "ab", &key),
            OwnedInvocationId::internal(owner(), fingerprint(), "ab", &key),
            OwnedInvocationId::route_session(
                owner(),
                fingerprint(),
                "ab",
                r#"("https://issuer.example","alice")"#,
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
            (Some(alice()), None),
            (None, None),
        ] {
            let first =
                OwnedInvocationId::external(owner(), fingerprint(), "method", scope.clone(), key);
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

    /// The expected bytes were written by hand from the layout in the documentation of
    /// [`IdempotencyScope::encoded`].
    #[test]
    fn scope_encoding_has_a_tag_and_length_prefixed_fields() {
        assert_eq!(
            hex(&pair("ab", "c").encoded()),
            "0100000000000000026162000000000000000163"
        );
        assert_eq!(
            hex(&IdempotencyScope::AdminImpersonation {
                admin: AccountId(Uuid::from_u128(10)),
                issuer: "ab".to_string(),
                subject: "c".to_string(),
            }
            .encoded()),
            "020000000000000000000000000000000a00000000000000026162000000000000000163"
        );
        assert_eq!(
            hex(&tokens(&[7, 5]).encoded()),
            "0300000000000000020000000000000000000000000000000500000000000000000000000000000007"
        );
        assert_ne!(pair("ab", "c").encoded(), pair("a", "bc").encoded());
        assert_ne!(pair("", "ab").encoded(), pair("ab", "").encoded());
        // The other request kinds have a text tag (`internal`, `route:` and `unkeyed`). A scope
        // starts with a control byte, so it is never equal to a text tag.
        for scope in [alice(), admin_as_alice(10), tokens(&[5])] {
            assert!(scope.encoded()[0].is_ascii_control());
        }
    }

    #[test]
    fn admin_scope_depends_on_the_admin_and_on_the_target() {
        let derive = |scope| {
            OwnedInvocationId::external(
                owner(),
                fingerprint(),
                "method",
                Some(scope),
                Some(&IdempotencyKey::from("key")),
            )
        };
        let scope = admin_as_alice(10);
        assert_eq!(derive(scope.clone()), derive(admin_as_alice(10)));
        assert_ne!(derive(scope.clone()), derive(admin_as_alice(11)));
        assert_ne!(
            derive(scope.clone()),
            derive(IdempotencyScope::AdminImpersonation {
                admin: AccountId(Uuid::from_u128(10)),
                issuer: "https://issuer.example".to_string(),
                subject: "bob".to_string(),
            })
        );
        // The admin and the account that it acts as never share a key.
        assert_ne!(derive(scope), derive(alice()));
    }

    #[test]
    fn token_scope_ignores_the_order_and_the_repeats_of_the_tokens() {
        assert_eq!(tokens(&[7, 5, 7]), tokens(&[5, 7]));
        assert_ne!(tokens(&[5, 7]), tokens(&[5]));
        assert_eq!(IdempotencyScope::tokens(std::iter::empty()), None);
        // A value that some other code built has the same encoding as the normal form.
        let unordered = IdempotencyScope::Tokens(vec![
            Uuid::from_u128(7),
            Uuid::from_u128(5),
            Uuid::from_u128(7),
        ]);
        assert_eq!(unordered.encoded(), tokens(&[5, 7]).encoded());
    }

    #[test]
    fn the_scope_of_a_caller_is_the_pair_of_its_original_principal() {
        let user = Caller::entry(user_principal());
        let account = Caller::entry(account_principal());
        assert_eq!(
            IdempotencyScope::of(&user),
            Some(pair("https://idp.example/", "user-1"))
        );
        let key = IdempotencyKey::from("key");
        let id = |caller: &Caller| {
            OwnedInvocationId::external(
                owner(),
                fingerprint(),
                "method",
                IdempotencyScope::of(caller),
                Some(&key),
            )
        };
        // Two original callers never share a key.
        assert_ne!(id(&user), id(&account));
        // One original caller has one scope at every entry point and through every chain: the
        // immediate principal, the tokens, the claims and the sign-in time are not part of it.
        assert_eq!(id(&user), id(&user.propagate(agent_principal())));
        assert_eq!(id(&user), id(&sample_caller()));
        let signed_in_again = Caller::entry(Principal::User(UserPrincipal {
            pair: Pair::new("https://idp.example/", "user-1"),
            claims: Claims::from_json(r#"{"sub":"user-1","email":"other@b.example"}"#.to_string())
                .unwrap(),
            authenticated_at: "2027-01-02T03:04:05.678Z".parse::<Timestamp>().unwrap(),
        }));
        assert_eq!(id(&user), id(&signed_in_again));
        // The pair is compared byte for byte.
        let other_subject = Caller::entry(Principal::User(UserPrincipal {
            pair: Pair::new("https://idp.example/", "User-1"),
            claims: Claims::from_json(r#"{"sub":"User-1"}"#.to_string()).unwrap(),
            authenticated_at: "2026-01-02T03:04:05.678Z".parse::<Timestamp>().unwrap(),
        }));
        assert_ne!(id(&user), id(&other_subject));
    }

    #[test]
    fn an_anonymous_caller_has_the_scope_of_its_tokens() {
        let key = IdempotencyKey::from("key");
        // Without a token there is no scope: the key is ignored, and each request is new.
        assert_eq!(IdempotencyScope::of(&Caller::anonymous()), None);
        assert!(matches!(
            InvocationRequestKey::external(IdempotencyScope::of(&Caller::anonymous()), Some(&key)),
            InvocationRequestKey::Unkeyed(_)
        ));
        assert_eq!(
            IdempotencyScope::of(&anonymous_with_tokens(&[7, 5])),
            Some(tokens(&[5, 7]))
        );
        assert_eq!(
            IdempotencyScope::of(&anonymous_with_tokens(&[7, 5])),
            IdempotencyScope::of(&anonymous_with_tokens(&[5, 7, 5]))
        );
        assert_ne!(
            IdempotencyScope::of(&anonymous_with_tokens(&[5])),
            IdempotencyScope::of(&anonymous_with_tokens(&[5, 7]))
        );
        // The maker of a token is not part of the scope.
        let mut with_a_maker = anonymous_with_tokens(&[5]);
        with_a_maker.tokens[0].maker = Some(PrincipalRef::new(Pair::new("https://m", "maker")));
        assert_eq!(
            IdempotencyScope::of(&with_a_maker),
            IdempotencyScope::of(&anonymous_with_tokens(&[5]))
        );
        // A caller with a pair has the scope of the pair, whatever tokens the request presented.
        let mut user_with_a_token = Caller::entry(user_principal());
        user_with_a_token.tokens = anonymous_with_tokens(&[5]).tokens;
        assert_eq!(
            IdempotencyScope::of(&user_with_a_token),
            IdempotencyScope::of(&Caller::entry(user_principal()))
        );
    }

    #[test]
    fn an_admin_that_acts_as_an_account_has_a_scope_of_its_own() {
        let target = Caller::entry(account_principal());
        let admin = AccountId(Uuid::from_u128(10));
        let scope = IdempotencyScope::admin_impersonation(admin, &target)
            .expect("the target account has a pair");
        assert_ne!(Some(scope.clone()), IdempotencyScope::of(&target));
        assert_ne!(
            Some(scope),
            IdempotencyScope::admin_impersonation(AccountId(Uuid::from_u128(11)), &target)
        );
        assert_eq!(
            IdempotencyScope::admin_impersonation(admin, &Caller::anonymous()),
            None
        );
    }

    #[test]
    fn invocation_id_all_scope_roles_are_disjoint_even_for_derived_key_text() {
        let key = IdempotencyKey::derived_from_bytes(&IdempotencyKey::from("internal"), b"key");
        let mut ids = vec![
            OwnedInvocationId::internal(owner(), fingerprint(), "method", &key),
            OwnedInvocationId::route_session(owner(), fingerprint(), "method", "internal", &key),
        ];
        for scope in [alice(), admin_as_alice(10), tokens(&[5])] {
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
