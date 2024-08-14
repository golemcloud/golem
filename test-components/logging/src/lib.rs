mod bindings;

use crate::bindings::exports::golem::it::api::*;
use rand::Rng;
use std::thread::sleep;
use std::time::Duration;

struct Component;

impl Guest for Component {
    fn init() {
        wasi_logger::Logger::install().expect("failed to install wasi_logger::Logger");
        log::set_max_level(log::LevelFilter::Trace);
    }

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

    fn forever_random_entries_with_log() {
        let mut rng = rand::thread_rng();
        let mut idx = 0;
        loop {
            let x: u32 = rng.gen();
            Self::log(idx, x);
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
            2 => golem_rust::bindings::wasi::logging::logging::log(
                golem_rust::bindings::wasi::logging::logging::Level::Critical,
                &format!("{idx}"),
                &format!("{x}"),
            ),
            3 => golem_rust::bindings::wasi::logging::logging::log(
                golem_rust::bindings::wasi::logging::logging::Level::Error,
                &format!("{idx}"),
                &format!("{x}"),
            ),
            4 => golem_rust::bindings::wasi::logging::logging::log(
                golem_rust::bindings::wasi::logging::logging::Level::Debug,
                &format!("{idx}"),
                &format!("{x}"),
            ),
            5 => golem_rust::bindings::wasi::logging::logging::log(
                golem_rust::bindings::wasi::logging::logging::Level::Info,
                &format!("{idx}"),
                &format!("{x}"),
            ),
            6 => golem_rust::bindings::wasi::logging::logging::log(
                golem_rust::bindings::wasi::logging::logging::Level::Warn,
                &format!("{idx}"),
                &format!("{x}"),
            ),
            7 => golem_rust::bindings::wasi::logging::logging::log(
                golem_rust::bindings::wasi::logging::logging::Level::Trace,
                &format!("{idx}"),
                &format!("{x}"),
            ),
            _ => {}
        }
    }

    fn log(idx: i32, x: u32) {
        match idx % 8 {
            0 => {
                println!("{idx}. message: {x}");
            }
            1 => {
                eprintln!("{idx}. message: {x}");
            }
            2 => log::error!(idx; "{x}"),
            3 => log::error!(idx; "{x}"),
            4 => log::debug!(idx; "{x}"),
            5 => log::info!(idx; "{x}"),
            6 => log::warn!(idx; "{x}"),
            7 => log::trace!(idx; "{x}"),
            _ => {}
        }
    }
}

bindings::export!(Component with_types_in bindings);
