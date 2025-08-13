pub struct WitPackage {
    pub name: String,   // e.g., "api:todos"
    pub version: String, // e.g., "1.0.0"
}

impl WitPackage {
    pub fn render_header(&self) -> String {
        format!("package {}@{};\n\n", self.name, self.version)
    }
}

#[cfg(test)]
mod tests {
    use super::WitPackage;

    #[test]
    fn header_renders() {
        let pkg = WitPackage { name: "api:todos".to_string(), version: "1.0.0".to_string() };
        let out = pkg.render_header();
        assert!(out.starts_with("package api:todos@1.0.0;"));
    }
} 