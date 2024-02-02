mod bindings;

use crate::bindings::Guest;

use std::fs::{read_to_string, remove_file, write};

struct Component;

impl Guest for Component {
    fn run() -> (Option<String>, Option<String>, Option<String>) {
        println!("file write/read/delete test starting");
        let read_nonexisting_result: Option<String> = read_to_string("testfile.txt").ok();
        println!("read_nonexisting_result: {:?}", read_nonexisting_result);
        let _ = write("/testfile.txt", "hello world").unwrap();
        println!("wrote test file");
        let read_existing_result: Option<String> = read_to_string("/testfile.txt").ok();
        println!("read_existing_result: {:?}", read_existing_result);
        let _ = remove_file("/testfile.txt").unwrap();
        println!("deleted test file");
        let read_nonexisting_result2: Option<String> = read_to_string("/testfile.txt").ok();
        println!("read_nonexisting_result2: {:?}", read_nonexisting_result2);

        (
            read_nonexisting_result,
            read_existing_result,
            read_nonexisting_result2,
        )
    }
}
