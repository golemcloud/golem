use golem_rust::{agent_definition, agent_implementation};
use golem_rust::agentic::{BasicModality, Multimodal, UnstructuredBinary, UnstructuredText};
use crate::location_details::LocationDetails;

// These agent methods are tools, since they take arguments, but with constructor params
#[agent_definition]
trait WeatherAgent {
    fn new(name: String) -> Self;

    fn get_weather_report_for_city(&self, city: String) -> String;
    fn get_weather_report_for_city_with_images(&self, city: String) -> Multimodal;
    fn get_weather_report_for_city_text(&self, city: String) -> UnstructuredText;
    fn get_snow_fall_image_for_city(&self, city: String) -> UnstructuredBinary<String>;
    fn get_lat_long_for_city(&self, city: String) -> LocationDetails;
}


struct MyDynamicWeatherToolImpl {
    name: String
}

#[agent_implementation]
impl WeatherAgent for MyDynamicWeatherToolImpl {
    fn new(name: String) -> Self {
        Self { name }
    }

    fn get_weather_report_for_city(&self, city: String) -> String {
        format!("Agent {}: This is a weather report for {}", self.name, city)
    }

    fn get_weather_report_for_city_with_images(&self, city: String) -> Multimodal {
        Multimodal::new([
            BasicModality::text(format!("Agent: {}, This is an image of the snow fall in {}.", self.name, city)),
            BasicModality::binary(vec![1, 2, 3], "image/png")
        ])
    }

    fn get_weather_report_for_city_text(&self, city: String) -> UnstructuredText {
        UnstructuredText::from_inline_any(format!("Agent: {}, This is an unstructured weather report for {}.", self.name, city))
    }

    fn get_snow_fall_image_for_city(&self, _city: String) -> UnstructuredBinary<String> {
        UnstructuredBinary::from_inline(
            vec![1, 2, 3],
            "image/png".to_string(),
        )
    }

    fn get_lat_long_for_city(&self, _city: String) -> LocationDetails {
        // For simplicity, we return dummy lat/long values
        LocationDetails {
            lat: 0.0,
            long: 0.0,
            country: "Unknown".to_string(),
            population: 0
        }
    }
}