#derive.golem_schema
struct Item { sku : String; quantity : Int }

// the first call creates one durable CartAgent per user:
#derive.agent
struct CartAgent {
  user : String
  mut items : Array[Item]
}

fn CartAgent::new(user : String) -> CartAgent {
  { user, items: [] }
}

pub fn CartAgent::add(self : Self, item : Item) -> Int { //@
  self.items.push(item) //@
  self.items.length()
}

pub async fn CartAgent::checkout(self : Self) -> String {
  let order = place_order(self.user, self.items)
  self.items = []
  order.id
}
