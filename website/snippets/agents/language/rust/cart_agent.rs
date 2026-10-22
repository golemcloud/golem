#[derive(Schema)] pub struct Item { sku: String, quantity: u32 }

// the first call creates one durable CartAgent per user:
#[agent_definition]
pub trait CartAgent {
  fn new(user: String) -> Self;
  fn add(&mut self, item: Item) -> u32; //@
  async fn checkout(&mut self) -> String;
}
struct CartAgentImpl { user: String, items: Vec<Item> }
#[agent_implementation]
impl CartAgent for CartAgentImpl {
  fn new(user: String) -> Self { Self { user, items: vec![] } }
  fn add(&mut self, item: Item) -> u32 {
    self.items.push(item); //@
    self.items.len() as u32
  }
  async fn checkout(&mut self) -> String {
    let order = place_order(&self.user, &self.items).await;
    self.items.clear();
    order.id
  }
}
