///|
pub using @wallClock {type Datetime}

///|
/// A span representing a unit of work, wrapping the low-level WIT resource.
pub(all) struct Span {
  priv inner : @raw.Span
}

///|
/// An invocation context for querying trace and attribute information.
pub(all) struct Context {
  priv inner : @raw.InvocationContext
}

///|
/// Starts a new span with the given name and optional attributes.
/// The caller is responsible for calling `finish()` on the returned span.
/// Prefer `with_span` for automatic span lifecycle management.
pub fn start_span(name : String, attributes? : Array[(String, String)]) -> Span {
  let span = @raw.start_span(name)
  match attributes {
    Some(attrs) => span.set_attributes(to_raw_attrs(attrs))
    None => ()
  }
  { inner: span }
}

///|
/// Runs a function within a new span, ensuring the span is finished afterward.
/// The span is always finished via `defer`, even if the function raises an error.
/// Do not call `span.finish()` inside `f`; `with_span` handles finishing automatically.
pub fn[T] with_span(
  name : String,
  attributes? : Array[(String, String)],
  f : (Span) -> T,
) -> T {
  let span = start_span(name, attributes?)
  defer span.finish()
  f(span)
}

///|
/// Gets the current invocation context.
pub fn current_context() -> Context {
  { inner: @raw.current_context() }
}

///|
/// Allows or disallows forwarding of trace context headers in outgoing HTTP requests.
/// Returns the previous value of the setting.
pub fn allow_forwarding_trace_context_headers(allow : Bool) -> Bool {
  @raw.allow_forwarding_trace_context_headers(allow)
}

///|
/// Sets a single attribute on the span.
pub fn Span::set_attribute(self : Span, key : String, value : String) -> Unit {
  self.inner.set_attribute(key, @raw.AttributeValue::String(value))
}

///|
/// Sets multiple attributes on the span.
pub fn Span::set_attributes(
  self : Span,
  attributes : Array[(String, String)],
) -> Unit {
  self.inner.set_attributes(to_raw_attrs(attributes))
}

///|
/// Gets the starting time of the span.
pub fn Span::started_at(self : Span) -> Datetime {
  self.inner.started_at()
}

///|
/// Finishes the span.
pub fn Span::finish(self : Span) -> Unit {
  self.inner.finish()
}

///|
/// Gets the current trace id.
pub fn Context::trace_id(self : Context) -> String {
  self.inner.trace_id()
}

///|
/// Gets the current span id.
pub fn Context::span_id(self : Context) -> String {
  self.inner.span_id()
}

///|
/// Gets the parent context, if any.
pub fn Context::parent(self : Context) -> Context? {
  self.inner.parent().map(fn(p) { { inner: p } })
}

///|
/// Gets an inherited attribute value by key (searches the full span stack).
pub fn Context::get_attribute(self : Context, key : String) -> String? {
  self.inner.get_attribute(key, true).map(unwrap_attribute_value)
}

///|
/// Gets an attribute value by key from the current span only.
pub fn Context::get_current_attribute(self : Context, key : String) -> String? {
  self.inner.get_attribute(key, false).map(unwrap_attribute_value)
}

///|
/// Gets all inherited attributes as a map (merged, latest value wins).
pub fn Context::attributes(self : Context) -> Map[String, String] {
  let attrs = self.inner.get_attributes(true)
  let map : Map[String, String] = {}
  for attr in attrs {
    map[attr.key] = unwrap_attribute_value(attr.value)
  }
  map
}

///|
/// Gets attributes from the current span only as a map.
pub fn Context::current_attributes(self : Context) -> Map[String, String] {
  let attrs = self.inner.get_attributes(false)
  let map : Map[String, String] = {}
  for attr in attrs {
    map[attr.key] = unwrap_attribute_value(attr.value)
  }
  map
}

///|
/// Gets the chain of values for an attribute key (innermost first).
pub fn Context::attribute_chain(self : Context, key : String) -> Array[String] {
  self.inner.get_attribute_chain(key).map(unwrap_attribute_value)
}

///|
/// Gets all attribute chains as a map.
pub fn Context::attribute_chains(self : Context) -> Map[String, Array[String]] {
  let chains = self.inner.get_attribute_chains()
  let map : Map[String, Array[String]] = {}
  for chain in chains {
    map[chain.key] = chain.values.map(unwrap_attribute_value)
  }
  map
}

///|
/// Gets the W3C Trace Context headers.
pub fn Context::trace_context_headers(
  self : Context,
) -> Array[(String, String)] {
  self.inner.trace_context_headers()
}

///|
fn unwrap_attribute_value(v : @raw.AttributeValue) -> String {
  match v {
    String(s) => s
  }
}

///|
fn to_raw_attrs(attrs : Array[(String, String)]) -> Array[@raw.Attribute] {
  attrs.map(fn(pair) {
    { key: pair.0, value: @raw.AttributeValue::String(pair.1) }
  })
}
