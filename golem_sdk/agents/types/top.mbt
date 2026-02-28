///|
pub(all) enum UnstructuredText {
  Url(String)
  Inline(text~ : String, language_code~ : String?)
} derive(Show, Eq)

///|
pub fn UnstructuredText::from_url(url : String) -> UnstructuredText {
  Url(url)
}

///|
pub fn UnstructuredText::from_inline(
  text : String,
  language_code? : String? = None,
) -> UnstructuredText {
  Inline(text~, language_code~)
}

///|
pub(all) enum UnstructuredBinary {
  Url(String)
  Inline(data~ : Bytes, mime_type~ : String)
} derive(Show, Eq)

///|
pub fn UnstructuredBinary::from_url(url : String) -> UnstructuredBinary {
  Url(url)
}

///|
pub fn UnstructuredBinary::from_inline(
  data : Bytes,
  mime_type~ : String,
) -> UnstructuredBinary {
  Inline(data~, mime_type~)
}
