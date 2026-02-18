use golem_rust::{agent_definition, agent_implementation, Schema};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Schema)]
pub struct Item {
    pub id: String,
    pub description: String,
    pub count: u64,
}

#[agent_definition]
trait Repository {
    fn new(name: String) -> Self;

    fn add(&mut self, id: String, description: String);
    fn get(&self, id: String) -> Option<Item>;
    fn list(&self) -> Vec<Item>;
}

struct RepositoryImpl {
    _name: String,
    items: BTreeMap<String, Item>,
}

#[agent_implementation]
impl Repository for RepositoryImpl {
    fn new(name: String) -> Self {
        Self {
            _name: name,
            items: BTreeMap::new(),
        }
    }

    fn add(&mut self, id: String, description: String) {
        println!("Adding item {id} to repository");
        let entry = self.items.entry(id.clone()).or_insert(Item {
            id,
            description,
            count: 0,
        });
        entry.count += 1;
    }

    fn get(&self, id: String) -> Option<Item> {
        self.items.get(&id).cloned()
    }

    fn list(&self) -> Vec<Item> {
        self.items.values().cloned().collect()
    }
}
