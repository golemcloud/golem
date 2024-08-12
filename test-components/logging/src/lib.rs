mod bindings;

use std::thread::sleep;
use std::time::Duration;
use rand::Rng;
use crate::bindings::exports::golem::it::api::*;

struct Component;

impl Guest for Component {
    fn forever_random_entries() {
        let mut rng = rand::thread_rng();
        let mut idx = 0;
        loop {
            let x: u32 = rng.gen();
            Self::print(idx, x);
            sleep(Duration::from_millis(500));
            idx += 1;
        }
    }

    fn some_random_entries() {
        let mut rng = rand::thread_rng();
        let mut idx = 0;
        while idx < 100 {
            let x: u32 = rng.gen();
            Self::print(idx, x);
            if idx % 10 == 0 {
                sleep(Duration::from_millis(100));
            }
            idx += 1;
        }
    }
}

impl Component {
    fn print(idx: i32, x: u32) {
        match idx % 8 {
            0 => {
                println!("{idx}. message: {x}");
            }
            1 => {
                eprintln!("{idx}. message: {x}");
            }
            2 => {
                golem_rust::bindings::wasi::logging::logging::log(
                    golem_rust::bindings::wasi::logging::logging::Level::Critical,
                    &format!("{idx}"),
                    &format!("{x}"),
                )
            }
            3 => {
                golem_rust::bindings::wasi::logging::logging::log(
                    golem_rust::bindings::wasi::logging::logging::Level::Error,
                    &format!("{idx}"),
                    &format!("{x}"),
                )
            }
            4 => {
                golem_rust::bindings::wasi::logging::logging::log(
                    golem_rust::bindings::wasi::logging::logging::Level::Debug,
                    &format!("{idx}"),
                    &format!("{x}"),
                )
            }
            5 => {
                golem_rust::bindings::wasi::logging::logging::log(
                    golem_rust::bindings::wasi::logging::logging::Level::Info,
                    &format!("{idx}"),
                    &format!("{x}"),
                )
            }
            6 => {
                golem_rust::bindings::wasi::logging::logging::log(
                    golem_rust::bindings::wasi::logging::logging::Level::Warn,
                    &format!("{idx}"),
                    &format!("{x}"),
                )
            }
            7 => {
                golem_rust::bindings::wasi::logging::logging::log(
                    golem_rust::bindings::wasi::logging::logging::Level::Trace,
                    &format!("{idx}"),
                    &format!("{x}"),
                )
            }
            _ => {}
        }
    }
}

bindings::export!(Component with_types_in bindings);
