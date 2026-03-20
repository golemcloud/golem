use golem_rust::{agent_definition, agent_implementation};
use golem_rust::agentic::{BasicModality, Multimodal, UnstructuredBinary, UnstructuredText};
use crate::location_details::LocationDetails;

// These agent methods are tools, since they take arguments, however
// no constructor params
#[agent_definition]
trait WeatherAgentSingleton {
    fn new() -> Self;

    fn get_weather_report_for_city(&self, city: String) -> String;
    fn get_weather_report_for_city_with_images(&self, city: String) -> Multimodal;
    fn get_weather_report_for_city_text(&self, city: String) -> UnstructuredText;
    fn get_snow_fall_image_for_city(&self, city: String) -> UnstructuredBinary<String>;
    fn get_lat_long_for_city(&self, city: String) -> LocationDetails;
}

struct MyStaticWeatherToolImpl;

#[agent_implementation]
impl WeatherAgentSingleton for MyStaticWeatherToolImpl {
    fn new() -> Self {
        Self
    }

    fn get_weather_report_for_city(&self, city: String) -> String {
        format!("This is a weather report for {}.", city)
    }

    fn get_weather_report_for_city_with_images(&self, city: String) -> Multimodal {
        Multimodal::new([
            BasicModality::text(format!("This is an image of the snow fall in {}.", city)),
            BasicModality::binary(vec![1, 2, 3], "image/png")
        ])
    }

    fn get_weather_report_for_city_text(&self, city: String) -> UnstructuredText {
        UnstructuredText::from_inline_any(format!("This is an unstructured weather report for {}.", city))
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