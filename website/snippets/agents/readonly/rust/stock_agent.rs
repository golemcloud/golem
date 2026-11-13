#[agent_definition]
pub trait StockAgent {
  fn new(sku: String) -> Self;
  fn restock(&mut self, n: u64);
  // repeat calls come from a cache, without loading the agent:
  // the cache clears when a restock succeeds:
  // outgoing HTTP, agent calls and key-value writes fail:
  #[read_only] //@
  fn level(&self) -> u64;
}
struct StockAgentImpl { units: u64 }
#[agent_implementation]
impl StockAgent for StockAgentImpl {
  fn new(_sku: String) -> Self { Self { units: 0 } }
  fn restock(&mut self, n: u64) { self.units += n; }
  fn level(&self) -> u64 { self.units }
}
