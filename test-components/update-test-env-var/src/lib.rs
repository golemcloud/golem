#[allow(static_mut_refs)]
#[allow(unused_imports)]
mod bindings;

struct Component;

impl crate::bindings::exports::golem::component::api::Guest for Component {
    fn get_version_from_env_var() -> String {
        std::env::var("GOLEM_COMPONENT_VERSION").unwrap_or_default()
    }
}

impl golem_rust::save_snapshot::exports::golem::api::save_snapshot::Guest for Component {
    fn save() -> Vec<u8> {
        Vec::new()
    }
}

impl golem_rust::load_snapshot::exports::golem::api::load_snapshot::Guest for Component {
    fn load(_bytes: Vec<u8>) -> Result<(), String> {
        Ok(())
    }
}

bindings::export!(Component with_types_in bindings);
golem_rust::save_snapshot::export_save_snapshot!(Component with_types_in golem_rust::save_snapshot);
golem_rust::load_snapshot::export_load_snapshot!(Component with_types_in golem_rust::load_snapshot);
