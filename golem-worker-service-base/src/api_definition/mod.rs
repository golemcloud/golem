pub use api_common::{ApiDefinitionId, ApiVersion, Host, ApiDeployment, Domain, SubDomain};
pub(crate) use api_common::{HasApiDefinitionId, HasGolemWorkerBindings, HasVersion, HasHost};
mod api_common;
pub mod http;
