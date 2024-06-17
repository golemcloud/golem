use std::env;
use crate::bindings::exports::golem::itrpc::rpc_api::Guest;
use crate::bindings::golem::it_stub::stub_child_component::{Api, Data};
use crate::bindings::golem::rpc::types::Uri;


mod bindings;

struct Component;

impl Guest for Component {
    fn echo(input: String) -> String {
        dbg!("Invoked parent echo");
        let component_id =
            env::var("CHILD_COMPONENT_ID").expect("PARENT_COMPONENT_ID not set");

        let uri = Uri { value: format!("worker://{component_id}/{}", "new-worker") };

        let api = Api::new(&uri);

        api.echo(input.as_str())
    }

    fn calculate(input: u64) -> u64 {
        dbg!("Invoked parent calculate");
        let component_id =
            env::var("CHILD_COMPONENT_ID").expect("PARENT_COMPONENT_ID not set");

        let uri = Uri { value: format!("worker://{component_id}/{}", "new-worker") };

        let api = Api::new(&uri);

        api.calculate(input)
    }

    fn process(input: Vec<Data>) -> Vec<Data> {
        dbg!("Invoked parent process");

        let component_id =
            env::var("ROOT_COMPONENT_ID").expect("ROOT_COMPONENT_ID not set");

        let uri = Uri { value: format!("worker://{component_id}/{}", "new-worker") };

        let api = Api::new(&uri);

        api.process(&input)
    }
}