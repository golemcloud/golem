///|
/// A modality: either text or image bytes. `#derive.multimodal` generates the
/// `@multimodal.MultimodalModality` impl that drives `@multimodal.Multimodal[_]`.
#derive.multimodal
pub(all) enum TextOrImage {
  Text(String)
  Image(Bytes)
}

///|
/// A vision agent that accepts multimodal input.
#derive.agent
struct VisionAgent {
  mut count : UInt64
}

///|
/// Creates a vision agent.
fn VisionAgent::new() -> VisionAgent {
  { count: 0 }
}

///|
/// Analyze a multimodal input value.
pub fn VisionAgent::analyze(
  self : Self,
  input : @multimodal.Multimodal[TextOrImage],
) -> String {
  self.count += 1
  let mut result = ""
  for item in input.items {
    match item {
      Text(t) => result = result + "text:" + t + " "
      Image(data) =>
        result = result + "image:" + data.length().to_string() + "bytes "
    }
  }
  result
}

///|
/// Multimodal mixed with a regular parameter (allowed in the new model).
pub fn VisionAgent::describe(
  self : Self,
  prefix : String,
  input : @multimodal.Multimodal[TextOrImage],
) -> String {
  prefix + ": " + self.analyze(input)
}

///|
/// A method returning a multimodal value.
pub fn VisionAgent::echo(
  self : Self,
  input : @multimodal.Multimodal[TextOrImage],
) -> @multimodal.Multimodal[TextOrImage] {
  self.count += 1
  input
}

///|
/// Returns how many times this agent has processed input.
pub fn VisionAgent::get_count(self : Self) -> UInt64 {
  self.count
}
