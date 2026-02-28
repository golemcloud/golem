# Moonbit/Core UUID

## Overview

The `@uuid` package provides an implementation of the Universally Unique
IDentifier per RFC 4122.

Currently, `@uuid` does not assume any random generation; you'll have to bring
your own random bytes to construct UUIDs.

## Usage

Construct a version 4 UUID:

```moonbit check
///|
test {
  let u = @uuid.from_hex("ddf99703-742f-7505-4c54-df36a9c243fe").as_version(
    @uuid.V4,
  )
  inspect(u.to_string(), content="ddf99703-742f-4505-8c54-df36a9c243fe")
}
```

You can then use it as a key in mappings as it's immutable and implemented
Hash, Eq (and Compare for b-tree).
