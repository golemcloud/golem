use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct McpContext {
    pub working_dir: PathBuf,
}

impl McpContext {
    pub fn new(working_dir: PathBuf) -> Self {
        Self { working_dir }
    }
}
