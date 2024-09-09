mod bindings;

use crate::bindings::exports::golem::component::api::*;
use rand::RngCore;
use reqwest::{Client, Response};
use std::cell::RefCell;
use std::thread::sleep;
use std::time::Duration;

struct State {
    last: u64,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State { last: 0 });
}

struct Component;

impl Guest for Component {
    fn f1(speed_ms: u64) -> u64 {
        STATE.with_borrow_mut(|state| {
            let mut current = state.last;

            println!("Starting to count..."); // newly added log line

            for _ in 0..30 {
                current += 10;
                report_f1(current);
                sleep(Duration::from_millis(speed_ms));
            }

            state.last = current; // In v2, we replace this to current/2
            current
        })
    }

    fn f2() -> u64 {
        let mut rng = rand::thread_rng();
        rng.next_u64() // In v2, we replace this with returning the current state
    }

    fn f3() -> u64 {
        std::env::args().collect::<Vec<_>>().len() as u64
            + std::env::vars().collect::<Vec<_>>().len() as u64
    }
}

fn report_f1(current: u64) {
    let port = std::env::var("PORT").unwrap_or("9999".to_string());
    let client = Client::builder().build().unwrap();

    let url = format!("http://localhost:{port}/f1");

    println!("Sending POST {url}");

    let response: Response = client
        .post(&url)
        .body(current.to_string())
        .send()
        .expect("Request failed");

    let status = response.status();
    let _ = response.text(); // ignoring response body

    println!("Received {status}");
}

bindings::export!(Component with_types_in bindings);
