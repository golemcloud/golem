pub use api_common::{ApiDefinitionId, ApiVersion, Host};
pub(crate) use api_common::{HasApiDefinitionId, HasGolemWorkerBindings, HasVersion};
mod api_common;
pub mod http;
