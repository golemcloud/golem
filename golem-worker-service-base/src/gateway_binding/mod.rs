use bincode::{Decode, Encode};
use serde::{Deserialize, Serialize};
use crate::gateway_plugins::Plugin;

pub(crate) use worker_binding_compiled::*;
pub(crate) use crate::gateway_binding::worker_binding::WorkerBinding;
pub(crate) use crate::gateway_execution::rib_input_value_resolver::*;
pub(crate) use crate::gateway_execution::worker_binding_resolver::*;

pub(crate) use worker_binding::*;
mod worker_binding_compiled;
mod worker_binding;

// A gateway binding is integration to the backend. This is similar to AWS's x-amazon-gateway-integration
// where it holds the details of where to re-route.

// One of the backends (bindings) is golem worker which is the default one.
// However, there can be other bindings such as file-server, plugin etc.
// While plugins primarily exist as a collection within other bindings (Example: cors plugin can exist within worker-binding),
// a plugin can stay standalone as an integration too if serving an incoming request
// only needs that plugin and nothing else. 
// Example: Cors-Preflight request.
//
// Internal Detail: For static bindings such as `plugins`, any `Rib` script associated with it can be
// executed during API definition registration itself, and stored as a static value binding. This is important
// as we pre-compile and pre-compute wherever we can to make serving the original request faster.
pub enum GatewayBinding {
    Default(WorkerBinding),
    Plugin(Plugin)
}
