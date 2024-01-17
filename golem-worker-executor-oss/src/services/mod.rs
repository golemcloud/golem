pub mod config;

#[derive(Clone)]
pub struct AdditionalDeps {}

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
