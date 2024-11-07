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

// A gateway binding is more or less the integration to the backend
// One of the backend is gole worker. A binding can be a static data too.
// An example of a static binding is cors pre-flight (which is part of `gateway-plugins`)
// (refer KONG gateway where CORS is a plugin). Some of these bindings is devoid of Rib
// scripts as they can be static enough which can be `executed` during registration of API definition itself.
pub enum GatewayBinding {
    Default(WorkerBinding),
    Plugin(Plugin)
}
