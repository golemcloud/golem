#derive.agent
struct StockAgent {
  sku : String
  mut units : Int
}

fn StockAgent::new(sku : String) -> StockAgent {
  { sku, units: 0 }
}

pub fn StockAgent::restock(self : Self, n : Int) -> Unit {
  self.units += n
}

// repeat calls come from a cache, without loading the agent:
// the cache clears when a restock succeeds:
// outgoing HTTP, agent calls and key-value writes fail:
#derive.read_only //@
pub fn StockAgent::level(self : Self) -> Int {
  self.units
}
