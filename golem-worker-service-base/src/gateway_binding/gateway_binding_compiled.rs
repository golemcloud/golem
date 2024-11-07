use crate::gateway_binding::{WorkerBindingCompiled};
use crate::gateway_plugins::Plugin;

#[derive(Debug, Clone, PartialEq)]
pub enum GatewayBindingCompiled {
    Default(WorkerBindingCompiled),
    Plugin(Plugin)
}
