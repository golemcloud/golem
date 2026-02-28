///|
pub(all) struct SourceMap {
  mappings : Array[SourceMapping]
} derive(ToJson)

///|
pub fn SourceMap::new() -> SourceMap {
  { mappings: [] }
}

///|
pub(all) struct SourceMapping {
  source : String
  original_offset : Int
  generated_offset : Int
  length : Int
} derive(ToJson)

///|
pub trait SourceMapBuilder {
  add_mapping(Self, String, Int, Int, Int) -> Unit
}

///|
pub impl SourceMapBuilder for SourceMap with add_mapping(
  self,
  source,
  original_offset_in_utf8,
  generated_offset_in_utf8,
  length_in_utf8,
) {
  self.mappings.push({
    source,
    original_offset: original_offset_in_utf8,
    generated_offset: generated_offset_in_utf8,
    length: length_in_utf8,
  })
}
