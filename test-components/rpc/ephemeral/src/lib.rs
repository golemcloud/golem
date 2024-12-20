use crate::bindings::exports::rpc::ephemeral_exports::api::Guest;

mod bindings;

pub struct Component;

impl Guest for Component {
    fn get_worker_name() -> String {
        std::env::var("GOLEM_WORKER_NAME").unwrap()
    }

    fn get_idempotency_key() -> String {
        golem_rust::generate_idempotency_key().to_string()
    }
}

bindings::export!(Component with_types_in bindings);
