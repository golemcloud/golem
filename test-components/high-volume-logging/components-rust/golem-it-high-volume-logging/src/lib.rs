#[allow(static_mut_refs)]
mod bindings;

use crate::bindings::exports::golem_it::high_volume_logging_exports::golem_it_high_volume_logging_api::*;

const LOREM_IPSUM: &str = "
Lorem ipsum dolor sit amet consectetur adipiscing elit. Quisque faucibus ex sapien vitae pellentesque sem placerat.
In id cursus mi pretium tellus duis convallis. Tempus leo eu aenean sed diam urna tempor.
Pulvinar vivamus fringilla lacus nec metus bibendum egestas. Iaculis massa nisl malesuada lacinia integer nunc posuere.
Ut hendrerit semper vel class aptent taciti sociosqu. Ad litora torquent per conubia nostra inceptos himenaeos.
";

struct Component;

impl Guest for Component {
    fn run() {
        for n in 1..=100 {
            println!("Iteration {n}: {LOREM_IPSUM}");
        }
    }
}

bindings::export!(Component with_types_in bindings);
