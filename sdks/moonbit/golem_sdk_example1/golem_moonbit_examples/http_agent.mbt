///|
/// A REST API agent exposing weather data via HTTP endpoints
#derive.agent
#derive.mount("/api/{city}/weather")
#derive.mount_auth(false)
#derive.mount_cors("https://app.acme.com", "https://other.com")
pub(all) struct WeatherAgent {
  city : String
  mut last_temperature : Double
}

///|
/// Creates a weather agent for a specific city
fn WeatherAgent::new(city : String) -> WeatherAgent {
  { city, last_temperature: 0.0, }
}

///|
/// Get current temperature
#derive.endpoint(get="/current?unit={unit}")
pub fn WeatherAgent::get_temperature(self : Self, unit : String) -> Double {
  if unit == "fahrenheit" {
    self.last_temperature * 9.0 / 5.0 + 32.0
  } else {
    self.last_temperature
  }
}

///|
/// Set the temperature (for testing)
#derive.endpoint(post="/set")
#derive.endpoint_header("X-Source", "source")
pub fn WeatherAgent::set_temperature(
  self : Self,
  temperature : Double,
  source : String,
) -> String {
  self.last_temperature = temperature
  "Temperature set to " + temperature.to_string() + " from " + source
}

///|
/// MIME types that the weather report endpoint can produce
#warnings("-struct_never_constructed")
pub(all) struct WeatherReportMimeTypes {}

///|
impl @schema.AllowedMimeTypes for WeatherReportMimeTypes with fn mime_types() {
  ["text/csv"]
}

///|
/// Download the weather report as a raw CSV body. There is no report until a
/// temperature is set, and only the `csv` format is supported.
#derive.endpoint(get="/report?format={format}")
pub fn WeatherAgent::get_report(
  self : Self,
  format : String,
) -> Result[@schema.UnstructuredBinary[WeatherReportMimeTypes]?, String] {
  if format != "csv" {
    Err("unsupported report format: " + format)
  } else if self.last_temperature == 0.0 {
    Ok(None)
  } else {
    let csv = "city,temperature\n\{self.city},\{self.last_temperature}\n"
    Ok(
      Some(
        @schema.UnstructuredBinary::inline(
          @utf8.encode(csv),
          mime_type="text/csv",
        ),
      ),
    )
  }
}
