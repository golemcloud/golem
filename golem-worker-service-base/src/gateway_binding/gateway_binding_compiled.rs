use crate::gateway_binding::{WorkerBindingCompiled};
use crate::gateway_binding::static_binding::StaticBinding;

#[derive(Debug, Clone, PartialEq)]
pub enum GatewayBindingCompiled {
    Worker(WorkerBindingCompiled),
    Static(StaticBinding)
}
