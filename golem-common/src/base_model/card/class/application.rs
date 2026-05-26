use super::*;
use crate::base_model::card::parsing::CardParseError;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum ApplicationResourcePattern {
    Empty,
    AnyCredential,
    Credential(Uuid),
}

impl Subsumes for ApplicationResourcePattern {
    fn subsumes(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Empty, Self::Empty) => true,
            (Self::AnyCredential, Self::AnyCredential | Self::Credential(_)) => true,
            (Self::Credential(a), Self::Credential(b)) => a == b,
            _ => false,
        }
    }
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
    ListAllEnvironments,
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

    fn parse_verb(verb: &str) -> Option<Self::Verb> {
        match verb {
            "view" => Some(Self::Verb::View),
            "create" => Some(Self::Verb::Create),
            "update" => Some(Self::Verb::Update),
            "delete" => Some(Self::Verb::Delete),
            "restore" => Some(Self::Verb::Restore),
            "mint-credential" => Some(Self::Verb::MintCredential),
            "rotate-credential" => Some(Self::Verb::RotateCredential),
            "revoke-credential" => Some(Self::Verb::RevokeCredential),
            "view-credentials" => Some(Self::Verb::ViewCredentials),
            "list-all-environments" => Some(Self::Verb::ListAllEnvironments),
            _ => None,
        }
    }

    fn parse_resource(resource: &str) -> Result<Self::Resource, CardParseError> {
        Self::parse_resource(Self::NAME, resource)
    }

    fn into_permission(pattern: ClassPermissionPattern<Self>) -> PermissionPattern {
        PermissionPattern::Application(pattern)
    }

    fn into_polymorphic_permission(
        pattern: PolymorphicClassPermissionPattern<Self>,
    ) -> PolymorphicPermissionPattern {
        PolymorphicPermissionPattern::Application(pattern)
    }
}

pub type ApplicationPermissionPattern = ClassPermissionPattern<ApplicationClass>;
pub type PolymorphicApplicationPermissionPattern =
    PolymorphicClassPermissionPattern<ApplicationClass>;

impl ApplicationClass {
    fn parse_resource(
        class: &str,
        resource: &str,
    ) -> Result<ApplicationResourcePattern, CardParseError> {
        if resource.is_empty() {
            Ok(ApplicationResourcePattern::Empty)
        } else if resource == "cred=*" {
            Ok(ApplicationResourcePattern::AnyCredential)
        } else if let Some(credential_id) = resource.strip_prefix("cred=") {
            Uuid::parse_str(credential_id)
                .map(ApplicationResourcePattern::Credential)
                .map_err(|_| CardParseError::InvalidResource {
                    class: class.to_string(),
                    resource: resource.to_string(),
                })
        } else {
            Err(CardParseError::InvalidResource {
                class: class.to_string(),
                resource: resource.to_string(),
            })
        }
    }
}
