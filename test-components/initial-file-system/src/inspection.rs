use golem_rust::{agent_definition, agent_implementation};
use std::fs::{create_dir_all, write};
use std::path::Path;
use std::time::Duration;

#[agent_definition]
pub trait Inspection {
    fn new(name: String, path: String, contents: Vec<u8>, fail: bool) -> Self;
    fn replace(&self, path: String, contents: Vec<u8>, delay_ms: u64);
}

struct InspectionImpl;

#[agent_implementation]
impl Inspection for InspectionImpl {
    fn new(_name: String, path: String, contents: Vec<u8>, fail: bool) -> Self {
        assert!(!fail, "inspection initializer failed");
        create_dir_all(Path::new(&path).parent().unwrap()).unwrap();
        write(path, contents).unwrap();
        Self
    }

    fn replace(&self, path: String, contents: Vec<u8>, delay_ms: u64) {
        std::thread::sleep(Duration::from_millis(delay_ms));
        write(path, contents).unwrap();
    }
}
