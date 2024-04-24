use lazy_static::lazy_static;
use prometheus::*;

lazy_static! {
    static ref COMPILATION_QUEUE_LENGTH: Gauge = register_gauge!(
        "component_compilation_queue_length",
        "Number of outstanding compilation requests"
    )
    .unwrap();
}

pub fn increment_queue_length() {
    COMPILATION_QUEUE_LENGTH.inc();
}

pub fn decrement_queue_length() {
    COMPILATION_QUEUE_LENGTH.dec();
}

pub fn register_all() -> Registry {
    default_registry().clone()
}
