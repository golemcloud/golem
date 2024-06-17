use std::env;
use crate::bindings::exports::golem::itrpc::rpc_api::{Data, Guest};
use crate::bindings::golem::it_stub::stub_rust_component_service::Api;
use crate::bindings::golem::rpc::types::Uri;

mod bindings;

struct Component;

impl Guest for Component {
    fn echo(input: String) -> String {
        let component_id =
            env::var("ROOT_COMPONENT_ID").expect("ROOT_COMPONENT_ID not set");

        let uri = Uri { value: format!("worker://{component_id}/{}", "new-worker") };

        let api = Api::new(&uri);

        api.echo(input.as_str())
    }

    fn calculate(input: u64) -> u64 {
        let component_id =
            env::var("ROOT_COMPONENT_ID").expect("ROOT_COMPONENT_ID not set");

        let uri = Uri { value: format!("worker://{component_id}/{}", "new-worker") };

        let api = Api::new(&uri);

        api.calculate(input)
    }

    fn process(input: Vec<Data>) -> Vec<Data> {
        let component_id =
            env::var("ROOT_COMPONENT_ID").expect("ROOT_COMPONENT_ID not set");

        let uri = Uri { value: format!("worker://{component_id}/{}", "new-worker") };

        let api = Api::new(&uri);

        api.process(&input)
    }
}