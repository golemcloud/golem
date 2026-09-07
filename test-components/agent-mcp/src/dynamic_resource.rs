use golem_rust::agentic::{BasicModality, Multimodal, UnstructuredBinary, UnstructuredText};
use golem_rust::{agent_definition, agent_implementation, read_only};

// Read-only agent methods that don't take any arguments become resources.
// However they are dynamic because it depends on agent identity (constructor)
// and therefore become "resource templates" according to MCP.

#[agent_definition]
pub trait DynamicResource {
    // The resource depends on the agent identity
    fn new(name: String) -> Self;

    #[read_only]
    fn get_weather_report(&self) -> String;
    #[read_only]
    fn get_weather_report_with_images(&self) -> Multimodal;
    #[read_only]
    fn get_weather_report_text(&self) -> UnstructuredText;
    #[read_only]
    fn get_snow_fall_image(&self) -> UnstructuredBinary<String>;
}

pub struct MyDynamicResourceImpl {
    name: String,
}

#[agent_implementation]
impl DynamicResource for MyDynamicResourceImpl {
    fn new(name: String) -> Self {
        Self { name }
    }

    fn get_weather_report(&self) -> String {
        format!("This is a dynamic weather report for {}.", self.name)
    }

    fn get_weather_report_with_images(&self) -> Multimodal {
        Multimodal::new([
            BasicModality::text(format!(
                "This is an image of the snow fall in {}.",
                self.name
            )),
            BasicModality::binary(vec![1, 2, 3], "image/png"),
        ])
    }

    fn get_weather_report_text(&self) -> UnstructuredText {
        UnstructuredText::from_inline_any(format!(
            "This is an unstructured weather report for {}.",
            self.name
        ))
    }

    fn get_snow_fall_image(&self) -> UnstructuredBinary<String> {
        UnstructuredBinary::from_inline(vec![1, 2, 3], "image/png".to_string())
    }
}
