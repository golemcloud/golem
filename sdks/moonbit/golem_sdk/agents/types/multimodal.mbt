///|
/// A multimodal collection — array of items of modality T
pub(all) struct Multimodal[T] {
  items : Array[T]
} derive(Debug, Eq)

///|
/// Creates a new `Multimodal` collection from an array of items.
pub fn[T] Multimodal::new(items : Array[T]) -> Multimodal[T] {
  { items, }
}

///|
/// Built-in modality: text or binary
pub(all) enum TextOrBinary {
  Text(UnstructuredText)
  Binary(UnstructuredBinary)
} derive(Debug, Eq)

///|
/// Extended modality: text, binary, or a custom structured type
pub(all) enum CustomModality[T] {
  Text(UnstructuredText)
  Binary(UnstructuredBinary)
  Structured(T)
} derive(Debug, Eq)
