///|
#derive.multimodal
pub(all) enum TextOrImage {
  Text(String)
  Image(Bytes)
}

///|
/// A vision agent that accepts multimodal input
#derive.agent
struct VisionAgent {
  mut count : UInt64
}

///|
fn VisionAgent::new() -> VisionAgent {
  { count: 0 }
}

///|
/// Analyze multimodal input
pub fn VisionAgent::analyze(
  self : Self,
  input : @types.Multimodal[TextOrImage],
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
