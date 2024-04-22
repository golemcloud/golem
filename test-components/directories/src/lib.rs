mod bindings;

use crate::bindings::Guest;

use std::fs::*;

struct Component;

impl Guest for Component {
    fn run() -> (u32, Vec<(String, bool)>, Vec<(String, bool)>, u32)
    {
        let rd1 = read_dir("/").unwrap();
        let count1 = rd1.into_iter().count();
        println!("number of entries in root at start: {}", count1);

        let _ = create_dir_all("/test/dir1").unwrap();
        let _ = create_dir_all("/test/dir2").unwrap();
        let _ = write("/test/hello.txt", "hello world").unwrap();

        println!("Created test entries");

        let root_entries: Vec<(String, bool)> = read_dir("/").unwrap().into_iter().map(|entry| {
            let dir_entry = entry.unwrap();
            (dir_entry.path().to_str().unwrap().to_string(), dir_entry.metadata().unwrap().is_dir())
        }).collect();

        let test_entries: Vec<(String, bool)> = read_dir("/test").unwrap().into_iter().map(|entry| {
            let dir_entry = entry.unwrap();
            (dir_entry.path().to_str().unwrap().to_string(), dir_entry.metadata().unwrap().is_dir())
        }).collect();

        println!("Deleting test entries");

        // let _ = remove_dir_all("/test").unwrap(); // NOTE: disabled because it panics

        let rd2 = read_dir("/").unwrap();
        let count2 = rd2.into_iter().count();
        println!("number of entries in root at end: {}", count2);

        (count1 as u32, root_entries, test_entries, count2 as u32)
    }
}
