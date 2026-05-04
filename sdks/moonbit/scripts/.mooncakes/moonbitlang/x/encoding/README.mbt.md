# Moonbit/Core Encoding

## Overview

The `@encoding` package provides an implementation for encoding and decoding
strings using various character encodings (e.g., UTF-8).

It supports both streaming and non-streaming (bulk) operations, making it
flexible for different use-cases.

## Supported Encoding

- UTF8
- UTF16 // alias for UTF16LE
- UTF16LE
- UTF16BE

## Usage

### Decoding

Decode a UTF-8 byte stream:

```moonbit check
///|
test {
  // Initialize a streaming UTF-8 decoder
  let decoder = @encoding.decoder(UTF8)

  // Consume byte chunks
  let inputs = [b"abc", b"\xf0", b"\x9f\x90\xb0"] // UTF8(🐰) == <F09F 90B0>
  inspect(decoder.consume(inputs[0]), content="abc")
  inspect(decoder.consume(inputs[1]), content="")
  inspect(decoder.consume(inputs[2]), content="🐰")

  // Finish decoding
  assert_true(decoder.finish().is_empty())
}
```

### Encoding

Encode a string to UTF-8 bytes:

```moonbit check
///|
test {
  // Encode a string to UTF-8
  let src = "你好👀"
  let bytes = @encoding.encode(UTF8, src)
  inspect(
    bytes,
    content=(
      #|b"\xe4\xbd\xa0\xe5\xa5\xbd\xf0\x9f\x91\x80"
    ),
  )
}
```

Encode a single character to UTF-8 bytes:

```moonbit check
///|
test {
  inspect(
    @encoding.to_utf8_bytes('A'),
    content=(
      #|b"A"
    ),
  )
}
```
