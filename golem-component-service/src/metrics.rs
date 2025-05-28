use crate::VERSION;
use lazy_static::lazy_static;
use prometheus::*;

lazy_static! {
    static ref VERSION_INFO: IntCounterVec =
        register_int_counter_vec!("version_info", "Version info of the server", &["version"])
            .unwrap();
}

pub fn register_all() -> Registry {
    VERSION_INFO.with_label_values(&[VERSION]).inc();

    default_registry().clone()
}
