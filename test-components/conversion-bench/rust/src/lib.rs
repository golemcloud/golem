use golem_rust::{agent_definition, agent_implementation};

#[agent_definition]
trait ConversionBenchRust {
    fn new(name: String) -> Self;
    fn checksum(&mut self, input: Vec<u8>) -> u32;
    fn produce(&mut self, length: u32) -> Vec<u8>;
}

struct ConversionBenchRustImpl;

#[agent_implementation]
impl ConversionBenchRust for ConversionBenchRustImpl {
    fn new(_name: String) -> Self {
        Self
    }

    fn checksum(&mut self, input: Vec<u8>) -> u32 {
        input
            .into_iter()
            .fold(0u32, |sum, byte| sum.wrapping_add(byte as u32))
    }

    fn produce(&mut self, length: u32) -> Vec<u8> {
        (0..length).map(|i| (i % 251) as u8).collect()
    }
}
