#[derive(Default, Clone)]
pub struct RawRibScript {
    rib_statements: Vec<String>,
}

impl RawRibScript {
    pub fn as_text(&self) -> String {
        self.rib_statements.join(";\n")
    }

    pub fn push(&mut self, rib_statement: &str) {
        self.rib_statements.push(rib_statement.to_string());
    }

    pub fn pop(&mut self) {
        self.rib_statements.pop();
    }
}
