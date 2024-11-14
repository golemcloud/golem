use crate::validation::ValidationBuilder;
use std::collections::BTreeMap;

pub type UnknownProperties = BTreeMap<String, serde_json::Value>;

pub trait HasUnknownProperties {
    fn unknown_properties(&self) -> &UnknownProperties;

    fn add_unknown_property_warns<F>(&self, context: F, validation_builder: &mut ValidationBuilder)
    where
        F: FnOnce() -> Vec<(&'static str, String)>,
    {
        let unknown_properties = self.unknown_properties();
        if unknown_properties.is_empty() {
            return;
        }

        let context = context();
        let context_count = context.len();

        for context in context {
            validation_builder.push_context(context.0, context.1);
        }

        validation_builder.add_warns(self.unknown_properties().keys(), |unknown_property_name| {
            Some((
                vec![],
                format!("Unknown property: {}", unknown_property_name),
            ))
        });

        for _ in 0..context_count {
            validation_builder.pop_context();
        }
    }
}
