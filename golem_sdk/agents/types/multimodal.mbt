///|
/// A multimodal collection — array of items of modality T
pub(all) struct Multimodal[T] {
  items : Array[T]
} derive(Show, Eq)

///|
pub fn[T] Multimodal::new(items : Array[T]) -> Multimodal[T] {
  { items, }
}

///|
/// Built-in modality: text or binary
pub(all) enum TextOrBinary {
  Text(UnstructuredText)
  Binary(UnstructuredBinary)
} derive(Show, Eq)

///|
/// Extended modality: text, binary, or a custom structured type
pub(all) enum CustomModality[T] {
  Text(UnstructuredText)
  Binary(UnstructuredBinary)
  Structured(T)
} derive(Show, Eq)
