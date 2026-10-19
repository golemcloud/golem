/**
 * Prefer a strict UTF-8 decoder, but follow the official Golem TypeScript SDK
 * by falling back when the runtime rejects the `fatal` option.
 *
 * wasm-rquickjs 0.3.6 throws from its JavaScript `TextDecoder` constructor even
 * though its native Rust decoder implements fatal decoding and the generated
 * guest enables the `encoding` feature. There is currently no feature flag
 * that bypasses that JavaScript guard. The fallback is intentionally lenient:
 * malformed UTF-8 is replaced with U+FFFD until GOL-324 is fixed upstream.
 *
 * @internal
 */
export const strictTextDecoder = () => {
  try {
    return new TextDecoder("utf-8", { fatal: true })
  } catch {
    return new TextDecoder("utf-8")
  }
}
