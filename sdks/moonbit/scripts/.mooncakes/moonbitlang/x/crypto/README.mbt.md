# moonbitlang/x/crypto

## Overview

A collection of cryptographic hash functions and utilities.

## Usage

> Strings in MoonBit are UTF-16 LE encoded.

### SHA-1

```moonbit check
///|
test {
  let input = "The quick brown fox jumps over the lazy dog"
  inspect(
    bytes_to_hex_string(sha1(@encoding.encode(UTF16, input))),
    content="bd136cb58899c93173c33a90dde95ead0d0cf6df",
  )
}
```

### MD5

```moonbit check
///|
test {
  let input = "The quick brown fox jumps over the lazy dog"
  inspect(
    bytes_to_hex_string(md5(@encoding.encode(UTF16, input))),
    content="b0986ae6ee1eefee8a4a399090126837",
  )

  // buffered
  let ctx = MD5::new()
  ctx.update(b"a")
  ctx.update(b"b")
  ctx.update(b"c")
  inspect(
    bytes_to_hex_string(ctx.finalize()),
    content="900150983cd24fb0d6963f7d28e17f72",
  )
}
```

### SM3

```moonbit check
///|
test {
  let input = "The quick brown fox jumps over the lazy dog"
  inspect(
    bytes_to_hex_string(sm3(@encoding.encode(UTF16, input))),
    content="fc2b31896629e88652ca1e3be449ec7ec93f7e5e29769f273fb973bc1858c66d",
  )

  //buffered
  let ctx = SM3::new()
  ctx.update(b"a".to_fixedarray())
  ctx.update(b"b".to_fixedarray())
  ctx.update(b"c".to_fixedarray())
  inspect(
    bytes_to_hex_string(ctx.finalize()),
    content="66c7f0f462eeedd9d1f2d46bdc10e4e24167c4875cf2f7a2297da02b8f4ba8e0",
  )
}
```
