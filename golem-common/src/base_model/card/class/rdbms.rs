use super::*;
use crate::base_model::card::parsing::{
    CardParseError, parse_agent_recipient, parse_environment_owner,
    parse_polymorphic_agent_recipient, parse_polymorphic_environment_owner,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum RdbmsResourcePattern {
    Table {
        database: String,
        schema: String,
        table: String,
    },
}

impl RdbmsResourcePattern {
    pub fn any() -> Self {
        Self::Table {
            database: "*".to_string(),
            schema: "*".to_string(),
            table: "*".to_string(),
        }
    }

    pub fn exact(value: impl Into<String>) -> Self {
        Self::parse_value(&value.into()).unwrap_or_else(|value| Self::Table {
            database: value,
            schema: String::new(),
            table: String::new(),
        })
    }

    pub fn glob(value: impl Into<String>) -> Self {
        Self::exact(value)
    }

    fn parse_value(value: &str) -> Result<Self, String> {
        let parts = value.split('.').collect::<Vec<_>>();
        if parts.len() != 3 || parts.iter().any(|part| part.is_empty()) {
            return Err(value.to_string());
        }
        Ok(Self::Table {
            database: parts[0].to_string(),
            schema: parts[1].to_string(),
            table: parts[2].to_string(),
        })
    }
}

impl Subsumes for RdbmsResourcePattern {
    fn subsumes(&self, other: &Self) -> bool {
        match (self, other) {
            (
                Self::Table {
                    database: a_database,
                    schema: a_schema,
                    table: a_table,
                },
                Self::Table {
                    database: b_database,
                    schema: b_schema,
                    table: b_table,
                },
            ) => {
                component_subsumes(a_database, b_database)
                    && component_subsumes(a_schema, b_schema)
                    && component_subsumes(a_table, b_table)
            }
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum RdbmsVerb {
    Query,
    Mutate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct RdbmsClass;

impl PermissionClass for RdbmsClass {
    type Verb = RdbmsVerb;
    type Owner = EnvironmentOwnerPattern;
    type Recipient = AgentRecipientPattern;
    type Resource = RdbmsResourcePattern;
    const NAME: &'static str = "rdbms";

    fn parse_verb(verb: &str) -> Option<Self::Verb> {
        match verb {
            "query" => Some(Self::Verb::Query),
            "mutate" => Some(Self::Verb::Mutate),
            _ => None,
        }
    }

    fn parse_owner(owner: &str) -> Result<Self::Owner, CardParseError> {
        parse_environment_owner(Self::NAME, owner)
    }

    fn parse_recipient(recipient: &str) -> Result<Self::Recipient, CardParseError> {
        parse_agent_recipient(recipient)
    }

    fn parse_resource(resource: &str) -> Result<Self::Resource, CardParseError> {
        Self::parse_resource(Self::NAME, resource)
    }

    fn parse_polymorphic_owner(
        owner: &str,
    ) -> Result<<Self::Owner as OwnerPattern>::Polymorphic, CardParseError> {
        parse_polymorphic_environment_owner(Self::NAME, owner)
    }

    fn parse_polymorphic_recipient(
        recipient: &str,
    ) -> Result<<Self::Recipient as RecipientPattern>::Polymorphic, CardParseError> {
        parse_polymorphic_agent_recipient(recipient)
    }

    fn into_permission(pattern: ClassPermissionPattern<Self>) -> PermissionPattern {
        PermissionPattern::Rdbms(pattern)
    }

    fn into_polymorphic_permission(
        pattern: PolymorphicClassPermissionPattern<Self>,
    ) -> PolymorphicPermissionPattern {
        PolymorphicPermissionPattern::Rdbms(pattern)
    }
}

pub type RdbmsPermissionPattern = ClassPermissionPattern<RdbmsClass>;
pub type PolymorphicRdbmsPermissionPattern = PolymorphicClassPermissionPattern<RdbmsClass>;

impl RdbmsClass {
    fn parse_resource(
        _class: &str,
        resource: &str,
    ) -> Result<RdbmsResourcePattern, CardParseError> {
        RdbmsResourcePattern::parse_value(resource).map_err(|_| CardParseError::InvalidResource {
            class: RdbmsClass::NAME.to_string(),
            resource: resource.to_string(),
        })
    }
}

fn component_subsumes(left: &str, right: &str) -> bool {
    left == "*" || left == right
}
