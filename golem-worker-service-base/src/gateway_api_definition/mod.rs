mod api_common;
pub mod http;

pub(crate) use api_common::HasGolemBindings;
pub use api_common::{
    ApiDefinitionId, ApiDeployment, ApiDeploymentRequest, ApiSite, ApiSiteString, ApiVersion,
};
