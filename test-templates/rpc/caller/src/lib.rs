mod bindings;

use std::env;
use bindings::*;
use crate::bindings::golem::rpc::types::Uri;
use crate::bindings::rpc::counters_stub::stub_counters::{Api, Counter};

struct Component;

impl Guest for Component {
    fn test1() -> Vec<(String, u64)> {
        println!("Creating, using and dropping counters");
        let template_id = env::var("COUNTERS_TEMPLATE_ID").expect("COUNTERS_TEMPLATE_ID not set");
        let counters_uri = Uri { value: format!("worker://{template_id}/counters_test1") };

        create_use_and_drop_counters(&counters_uri);
        println!("All counters dropped, querying result");

        let remote_api = Api::new(&counters_uri);
        remote_api.get_all_dropped()
    }
}

fn create_use_and_drop_counters(counters_uri: &Uri) {
    let counter1 = Counter::new(counters_uri, "counter1");
    let counter2 = Counter::new(counters_uri, "counter2");
    let counter3 = Counter::new(counters_uri, "counter3");
    counter1.inc_by(1);
    counter1.inc_by(1);
    counter1.inc_by(1);

    counter2.inc_by(2);
    counter2.inc_by(1);

    counter3.inc_by(3);

    let value1 = counter1.get_value();
    let value2 = counter2.get_value();
    let value3 = counter3.get_value();

    println!("Counter1 value: {}", value1);
    println!("Counter2 value: {}", value2);
    println!("Counter3 value: {}", value3);
}
