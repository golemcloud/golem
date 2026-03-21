use golem_rust::{agent_definition, agent_implementation};
use golem_rust::agentic::{BasicModality, Multimodal, UnstructuredBinary, UnstructuredText};

// Agent methods that don't take any arguments becomes a resource
// However they are static because they don't depend on the agent identity,
// and therefore becomes "static resources" according to MCP
#[agent_definition]
pub trait StaticResource {
    // The resource is static, it doesn't depend on the agent identity
    fn new() -> Self;
    fn get_static_weather_report(&self) -> String;
    fn get_static_weather_report_with_images(&self) -> Multimodal;
    fn get_static_weather_report_text(&self) -> UnstructuredText;
    fn get_static_now_fall_image(&self) -> UnstructuredBinary<String>;
}

struct MyStaticResourceImpl;

#[agent_implementation]
impl StaticResource for MyStaticResourceImpl {
    fn new() -> Self {
        Self
    }

    fn get_static_weather_report(&self) -> String {
        let weather_reports = vec![
            ("Sydney", "Sunny"),
            ("Darwin", "Rainy"),
            ("Hobart", "Cloudy"),
        ];

        weather_reports
            .iter()
            .map(|(country, weather)| format!("{}: {}", country, weather))
            .collect::<Vec<_>>()
            .join(", ")

    }

    fn get_static_weather_report_with_images(&self) -> Multimodal {
        Multimodal::new([
            BasicModality::text("This is an image of the snow fall in Sydney.".to_string()),
            BasicModality::binary(vec![1, 2, 3], "image/png")
        ])
    }

    fn get_static_weather_report_text(&self) -> UnstructuredText {
        UnstructuredText::from_inline_any("This is an unstructured weather report.".to_string())
    }

    fn get_static_now_fall_image(&self) -> UnstructuredBinary<String> {
        UnstructuredBinary::from_inline(
            vec![1, 2, 3],
            "image/png".to_string(),
        )
    }
}