use super::*;
use crate::base_model::card::parsing::CardParseError;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum KvResourcePattern {
    StoreKey { store: String, key_pattern: String },
}

impl KvResourcePattern {
    pub fn any() -> Self {
        Self::StoreKey {
            store: "*".to_string(),
            key_pattern: "**".to_string(),
        }
    }

    pub fn exact(value: impl Into<String>) -> Self {
        Self::parse_value(&value.into()).unwrap_or_else(|value| Self::StoreKey {
            store: value,
            key_pattern: String::new(),
        })
    }

    pub fn glob(value: impl Into<String>) -> Self {
        Self::exact(value)
    }

    fn parse_value(value: &str) -> Result<Self, String> {
        let Some((store, key_pattern)) = value.split_once('.') else {
            return Err(value.to_string());
        };
        if store.is_empty() || key_pattern.is_empty() {
            return Err(value.to_string());
        }
        Ok(Self::StoreKey {
            store: store.to_string(),
            key_pattern: key_pattern.to_string(),
        })
    }
}

impl Subsumes for KvResourcePattern {
    fn subsumes(&self, other: &Self) -> bool {
        match (self, other) {
            (
                Self::StoreKey {
                    store: a_store,
                    key_pattern: a_key,
                },
                Self::StoreKey {
                    store: b_store,
                    key_pattern: b_key,
                },
            ) => (a_store == "*" || a_store == b_store) && glob_subsumes(a_key, b_key),
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum KvVerb {
    Read,
    Write,
    Delete,
    List,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct KvClass;

impl PermissionClass for KvClass {
    type Verb = KvVerb;
    type Owner = EnvironmentOwnerPattern;
    type Recipient = AgentRecipientPattern;
    type Resource = KvResourcePattern;
    const NAME: &'static str = "kv";

    fn parse_verb(verb: &str) -> Option<Self::Verb> {
        match verb {
            "read" => Some(Self::Verb::Read),
            "write" => Some(Self::Verb::Write),
            "delete" => Some(Self::Verb::Delete),
            "list" => Some(Self::Verb::List),
            _ => None,
        }
    }

    fn parse_resource(resource: &str) -> Result<Self::Resource, CardParseError> {
        Self::parse_resource(Self::NAME, resource)
    }

    fn into_permission(pattern: ClassPermissionPattern<Self>) -> PermissionPattern {
        PermissionPattern::Kv(pattern)
    }

    fn into_polymorphic_permission(
        pattern: PolymorphicClassPermissionPattern<Self>,
    ) -> PolymorphicPermissionPattern {
        PolymorphicPermissionPattern::Kv(pattern)
    }
}

pub type KvPermissionPattern = ClassPermissionPattern<KvClass>;
pub type PolymorphicKvPermissionPattern = PolymorphicClassPermissionPattern<KvClass>;

impl KvClass {
    fn parse_resource(_class: &str, resource: &str) -> Result<KvResourcePattern, CardParseError> {
        KvResourcePattern::parse_value(resource).map_err(|_| CardParseError::InvalidResource {
            class: KvClass::NAME.to_string(),
            resource: resource.to_string(),
        })
    }
}
