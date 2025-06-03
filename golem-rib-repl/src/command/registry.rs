use crate::command::builtin::{Clear, Exports, TypeInfo};
use crate::UntypedCommand;
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Default)]
pub struct CommandRegistry {
    commands: HashMap<String, Arc<dyn UntypedCommand>>,
}

impl CommandRegistry {
    pub(crate) fn built_in() -> Self {
        let mut registry = Self::default();
        registry.register(TypeInfo);
        registry.register(Clear);
        registry.register(Exports);
        registry
    }

    pub fn merge(&mut self, other: CommandRegistry) {
        for (name, command) in other.commands {
            self.commands.insert(name, command);
        }
    }

    pub fn get_commands(&self) -> Vec<String> {
        self.commands.keys().map(|name| name.to_string()).collect()
    }

    pub fn register<T>(&mut self, command: T)
    where
        T: UntypedCommand + 'static,
    {
        let name = command.command_name().to_string();
        self.commands.insert(name, Arc::new(command));
    }

    pub fn get_command(&self, name: &str) -> Option<Arc<dyn UntypedCommand>> {
        let result = self.commands.get(name);
        match result {
            Some(command) => Some(command.clone()),
            None => None,
        }
    }
}
