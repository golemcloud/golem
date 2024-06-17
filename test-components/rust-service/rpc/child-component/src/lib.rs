mod bindings;

use crate::bindings::exports::golem::it::api::*;

struct Component;

impl Guest for Component {
    fn echo(input: String) -> String {
        println!("Invoked child echo");
        common::echo(input)
    }

    fn calculate(input: u64) -> u64 {
        println!("Invoked child calculate");
        let (i, s) = common::calculate_sum(10000, input);
        let result = (s / i as u128) as u64;
        result
    }

    fn process(input: Vec<Data>) -> Vec<Data> {
        println!("Invoked child process");
        let result = common::process_data(input.into_iter().map(|i| i.into()).collect());
        result.into_iter().map(|i| i.into()).collect()
    }
}

impl From<Data> for common::CommonData {
    fn from(data: Data) -> Self {
        common::CommonData {
            id: data.id,
            name: data.name,
            desc: data.desc,
            timestamp: data.timestamp,
        }
    }
}

impl From<common::CommonData> for Data {
    fn from(data: common::CommonData) -> Self {
        Data {
            id: data.id,
            name: data.name,
            desc: data.desc,
            timestamp: data.timestamp,
        }
    }
}
