use crate::component::*;
use crate::core::ExprSink;

impl<T: ExprSink + Debug + Clone + PartialEq> From<Component<T>> for wasm_encoder::Component {
    fn from(value: Component<T>) -> Self {
        let mut component = wasm_encoder::Component::new();

        for (section_type, sections) in value.into_grouped() {
            match section_type {
                ComponentSectionType::Module => {} // !
                ComponentSectionType::CoreInstance => {}
                ComponentSectionType::CoreType => {}
                ComponentSectionType::Component => {
                    let inner_component = sections.first().unwrap().as_component();
                    let encoded_component = wasm_encoder::Component::from(inner_component.clone());
                    let nested_component = wasm_encoder::NestedComponentSection(&encoded_component);
                    component.section(&nested_component);
                }
                ComponentSectionType::Instance => {}
                ComponentSectionType::Alias => {}
                ComponentSectionType::Type => {}
                ComponentSectionType::Canon => {}
                ComponentSectionType::Start => {} // !
                ComponentSectionType::Import => {}
                ComponentSectionType::Export => {}
                ComponentSectionType::Custom => {
                    let custom = sections.first().unwrap().as_custom();
                    let section: wasm_encoder::CustomSection = custom.into();
                    component.section(&section);
                }
            }
        }

        component
    }
}
