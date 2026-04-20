///|
/// Represents unstructured text content, either as a URL reference or inline data
/// with an optional language code.
pub(all) enum UnstructuredText {
  Url(String)
  Inline(text~ : String, language_code~ : String?)
} derive(Show, Eq)

///|
/// Creates an `UnstructuredText` from a URL pointing to the text content.
pub fn UnstructuredText::from_url(url : String) -> UnstructuredText {
  Url(url)
}

///|
/// Creates an `UnstructuredText` with inline text and an optional language code.
pub fn UnstructuredText::from_inline(
  text : String,
  language_code? : String? = None,
) -> UnstructuredText {
  Inline(text~, language_code~)
}

///|
/// Represents unstructured binary content, either as a URL reference or inline data
/// with a MIME type.
pub(all) enum UnstructuredBinary {
  Url(String)
  Inline(data~ : Bytes, mime_type~ : String)
} derive(Show, Eq)

///|
/// Creates an `UnstructuredBinary` from a URL pointing to the binary content.
pub fn UnstructuredBinary::from_url(url : String) -> UnstructuredBinary {
  Url(url)
}

///|
/// Creates an `UnstructuredBinary` with inline binary data and a MIME type.
pub fn UnstructuredBinary::from_inline(
  data : Bytes,
  mime_type~ : String,
) -> UnstructuredBinary {
  Inline(data~, mime_type~)
}
