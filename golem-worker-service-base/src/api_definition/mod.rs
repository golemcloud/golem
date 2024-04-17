pub use api_common::{ApiDefinitionId, ApiDeployment, ApiVersion, Domain, Host, SubDomain};
pub(crate) use api_common::{HasApiDefinitionId, HasGolemWorkerBindings, HasHost, HasVersion};
mod api_common;
pub mod http;
