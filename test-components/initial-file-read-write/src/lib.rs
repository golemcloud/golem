mod bindings;

use crate::bindings::Guest;

use std::fs::{read_to_string, remove_file, write};

struct Component;

impl Guest for Component {
    fn run() -> (Option<String>, Option<String>, Option<String>, Option<String>) {
        println!("initial ro file read starting");
        let read_ro_result: Option<String> = read_to_string("foo.txt").ok();
        println!("initial ro file write starting");
        let write_ro_result = write("foo.txt", "hello world").ok().map(|_| "success".to_string());
        println!("initial rw file read starting");
        let read_rw_result: Option<String> = read_to_string("bar/baz.txt").ok();
        println!("initial rw file write starting");
        write("bar/baz.txt", "hello world").unwrap();
        let read_rw_result_after_update: Option<String> = read_to_string("bar/baz.txt").ok();
        (
            read_ro_result,
            write_ro_result,
            read_rw_result,
            read_rw_result_after_update
        )
    }
}

bindings::export!(Component with_types_in bindings);
