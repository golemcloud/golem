//! A Rust agent statically composed with `gc-probe/gc_probe.wat`, a component that allocates wasm
//! GC objects and throws wasm exceptions. No Golem SDK compiles to those proposals yet, so this
//! fixture exercises them end to end through the executor.

mod bindings {
    wit_bindgen::generate!({
        path: "wit",
        world: "gc-probe-imports",
        generate_all,
    });
}

use bindings::golem_it::gc_probe::heap;
use golem_rust::{agent_definition, agent_implementation};

#[agent_definition]
pub trait GcHeapAgent {
    fn new(name: String) -> Self;
    fn retain(&mut self, kib: u32) -> u32;
    fn release(&mut self);
    fn retained_kib(&self) -> u32;
    fn catch_thrown(&self, value: u32) -> u32;
    fn throw_uncaught(&self);
}

struct GcHeapAgentImpl {
    _name: String,
}

#[agent_implementation]
impl GcHeapAgent for GcHeapAgentImpl {
    fn new(name: String) -> Self {
        Self { _name: name }
    }

    fn retain(&mut self, kib: u32) -> u32 {
        heap::retain(kib)
    }

    fn release(&mut self) {
        heap::release()
    }

    fn retained_kib(&self) -> u32 {
        heap::retained_kib()
    }

    fn catch_thrown(&self, value: u32) -> u32 {
        heap::catch_thrown(value)
    }

    fn throw_uncaught(&self) {
        heap::throw_uncaught()
    }
}
