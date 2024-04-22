mod bindings;

use crate::bindings::Guest;

use std::io;
use std::io::Read;
use std::str::FromStr;

struct Component;

impl Guest for Component {
    fn run() {
        let mut input = String::new();
        io::stdin().read_to_string(&mut input).unwrap();
        eprintln!("Got input through stdin: {}", input);

        let n = u64::from_str(&input).expect("requires an u64 as input");

        let result = n * 2;
        println!("{}", result);
    }
}
