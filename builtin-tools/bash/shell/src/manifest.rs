//! Documentation for the local command inventory.
pub(crate) struct Manifest {
    pub name: String,
    pub synopsis: String,
    pub help_text: String,
}
impl Manifest {
    pub fn builtin(name: impl Into<String>, synopsis: impl Into<String>) -> Self {
        let synopsis = synopsis.into();
        Self {
            name: name.into(),
            help_text: synopsis.clone(),
            synopsis,
        }
    }
    pub fn with_help(mut self, help: impl Into<String>) -> Self {
        self.help_text = help.into();
        self
    }
}
