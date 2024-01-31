pub mod config;

#[derive(Clone)]
pub struct AdditionalDeps {}

impl Default for AdditionalDeps {
    fn default() -> Self {
        Self::new()
    }
}

impl AdditionalDeps {
    pub fn new() -> Self {
        Self {}
    }

    #[cfg(test)]
    #[allow(unused)]
    pub async fn mocked() -> Self {
        Self {}
    }
}
