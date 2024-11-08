use bincode::{Decode, Encode};
use serde::{Deserialize, Serialize};

pub(crate) use worker_binding_compiled::*;
pub(crate) use crate::gateway_binding::worker_binding::WorkerBinding;
pub(crate) use crate::gateway_execution::rib_input_value_resolver::*;
pub(crate) use crate::gateway_execution::gateway_binding_resolver::*;

pub(crate) use worker_binding::*;
pub(crate) use gateway_binding_compiled::*;
pub(crate) use static_binding::*;

mod worker_binding_compiled;
mod worker_binding;
mod gateway_binding_compiled;
mod static_binding;

// A gateway binding is integration to the backend. This is similar to AWS's x-amazon-gateway-integration
// where it holds the details of where to re-route.

// The default integration is to golem-worker.
// Certain integrations can exist as a static binding, which is restricted
// from anything dynamic in nature. This implies, there will not be Rib in either pre-compiled or raw form.
#[derive(Debug, Clone, PartialEq)]
pub enum GatewayBinding {
    Default(WorkerBinding),
    Static(StaticBinding)
}
