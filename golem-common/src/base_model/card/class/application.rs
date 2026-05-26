use super::*;
use crate::base_model::card::parsing::{
    CardParseError, parse_account_recipient, parse_application_owner,
    parse_polymorphic_account_recipient, parse_polymorphic_application_owner,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct ApplicationResourcePattern;

impl Subsumes for ApplicationResourcePattern {
    fn subsumes(&self, _other: &Self) -> bool {
        true
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum PolymorphicApplicationResourcePattern {
    Concrete(ApplicationResourcePattern),
    Slot(SlotVariable),
    Template(String),
}

impl ResourcePattern for ApplicationResourcePattern {
    type Polymorphic = PolymorphicApplicationResourcePattern;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum ApplicationVerb {
    View,
    Create,
    Update,
    Delete,
    Restore,
    MintCredential,
    RotateCredential,
    RevokeCredential,
    ViewCredentials,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct ApplicationClass;

impl PermissionClass for ApplicationClass {
    type Verb = ApplicationVerb;
    type Owner = ApplicationOwnerPattern;
    type Recipient = AccountRecipientPattern;
    type Resource = ApplicationResourcePattern;
    const NAME: &'static str = "application";
}

pub type ApplicationPermissionPattern = ClassPermissionPattern<ApplicationClass>;
pub type PolymorphicApplicationPermissionPattern =
    PolymorphicClassPermissionPattern<ApplicationClass>;

impl ApplicationClass {
    pub(crate) fn parse_permission(
        owner: &str,
        recipient: &str,
        verb: &str,
        resource: &str,
    ) -> Result<PermissionPattern, CardParseError> {
        let owner = parse_application_owner(Self::NAME, owner)?;
        let recipient = parse_account_recipient(recipient)?;
        let resource = Self::parse_resource(Self::NAME, resource)?;
        Ok(PermissionPattern::Application(match verb {
            "*" => ApplicationPermissionPattern::Any {
                owner,
                recipient,
                resource,
            },
            "view" => ApplicationPermissionPattern::Verb {
                verb: ApplicationVerb::View,
                owner,
                recipient,
                resource,
            },
            "create" => ApplicationPermissionPattern::Verb {
                verb: ApplicationVerb::Create,
                owner,
                recipient,
                resource,
            },
            "update" => ApplicationPermissionPattern::Verb {
                verb: ApplicationVerb::Update,
                owner,
                recipient,
                resource,
            },
            "delete" => ApplicationPermissionPattern::Verb {
                verb: ApplicationVerb::Delete,
                owner,
                recipient,
                resource,
            },
            "restore" => ApplicationPermissionPattern::Verb {
                verb: ApplicationVerb::Restore,
                owner,
                recipient,
                resource,
            },
            "mint-credential" => ApplicationPermissionPattern::Verb {
                verb: ApplicationVerb::MintCredential,
                owner,
                recipient,
                resource,
            },
            "rotate-credential" => ApplicationPermissionPattern::Verb {
                verb: ApplicationVerb::RotateCredential,
                owner,
                recipient,
                resource,
            },
            "revoke-credential" => ApplicationPermissionPattern::Verb {
                verb: ApplicationVerb::RevokeCredential,
                owner,
                recipient,
                resource,
            },
            "view-credentials" => ApplicationPermissionPattern::Verb {
                verb: ApplicationVerb::ViewCredentials,
                owner,
                recipient,
                resource,
            },
            other => {
                return Err(CardParseError::UnknownVerb {
                    class: Self::NAME.to_string(),
                    verb: other.to_string(),
                });
            }
        }))
    }

    pub(crate) fn parse_polymorphic_permission(
        owner: &str,
        recipient: &str,
        verb: &str,
        resource: &str,
    ) -> Result<PolymorphicPermissionPattern, CardParseError> {
        let owner = parse_polymorphic_application_owner(Self::NAME, owner)?;
        let recipient = parse_polymorphic_account_recipient(recipient)?;
        let resource = Self::parse_polymorphic_resource(Self::NAME, resource)?;
        Ok(PolymorphicPermissionPattern::Application(match verb {
            "*" => PolymorphicApplicationPermissionPattern::Any {
                owner,
                recipient,
                resource,
            },
            "view" => PolymorphicApplicationPermissionPattern::Verb {
                verb: ApplicationVerb::View,
                owner,
                recipient,
                resource,
            },
            "create" => PolymorphicApplicationPermissionPattern::Verb {
                verb: ApplicationVerb::Create,
                owner,
                recipient,
                resource,
            },
            "update" => PolymorphicApplicationPermissionPattern::Verb {
                verb: ApplicationVerb::Update,
                owner,
                recipient,
                resource,
            },
            "delete" => PolymorphicApplicationPermissionPattern::Verb {
                verb: ApplicationVerb::Delete,
                owner,
                recipient,
                resource,
            },
            "restore" => PolymorphicApplicationPermissionPattern::Verb {
                verb: ApplicationVerb::Restore,
                owner,
                recipient,
                resource,
            },
            "mint-credential" => PolymorphicApplicationPermissionPattern::Verb {
                verb: ApplicationVerb::MintCredential,
                owner,
                recipient,
                resource,
            },
            "rotate-credential" => PolymorphicApplicationPermissionPattern::Verb {
                verb: ApplicationVerb::RotateCredential,
                owner,
                recipient,
                resource,
            },
            "revoke-credential" => PolymorphicApplicationPermissionPattern::Verb {
                verb: ApplicationVerb::RevokeCredential,
                owner,
                recipient,
                resource,
            },
            "view-credentials" => PolymorphicApplicationPermissionPattern::Verb {
                verb: ApplicationVerb::ViewCredentials,
                owner,
                recipient,
                resource,
            },
            other => {
                return Err(CardParseError::UnknownVerb {
                    class: Self::NAME.to_string(),
                    verb: other.to_string(),
                });
            }
        }))
    }

    fn parse_resource(
        class: &str,
        resource: &str,
    ) -> Result<ApplicationResourcePattern, CardParseError> {
        if resource.is_empty() {
            Ok(ApplicationResourcePattern)
        } else {
            Err(CardParseError::InvalidResource {
                class: class.to_string(),
                resource: resource.to_string(),
            })
        }
    }

    fn parse_polymorphic_resource(
        class: &str,
        resource: &str,
    ) -> Result<PolymorphicApplicationResourcePattern, CardParseError> {
        if let Ok(resource) = Self::parse_resource(class, resource) {
            Ok(PolymorphicApplicationResourcePattern::Concrete(resource))
        } else if let Ok(slot) = SlotVariable::parse(resource) {
            Ok(PolymorphicApplicationResourcePattern::Slot(slot))
        } else {
            Err(CardParseError::InvalidResource {
                class: class.to_string(),
                resource: resource.to_string(),
            })
        }
    }
}
