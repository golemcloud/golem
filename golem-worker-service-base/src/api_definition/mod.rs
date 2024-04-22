pub use api_common::{ApiDefinitionId, ApiDeployment, ApiSite, ApiVersion, ApiSiteString};
pub(crate) use api_common::{HasApiDefinitionId, HasGolemWorkerBindings, HasVersion};
mod api_common;
pub mod http;
