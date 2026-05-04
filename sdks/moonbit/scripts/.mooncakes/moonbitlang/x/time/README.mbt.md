# Moonbit/Core Time

## Overview

Package time provides functionality for measuring and manipulating time.

The calendrical calculations always assume a ISO 8601 calendar, with no leap seconds.

To create a datatime that represents the current time, you need to obtain the unix second and time zone offset from the [wasi](https://mooncakes.io/docs/#/peter-jerry-ye/wasi/) package (wasm-gc backend) or other FFI functions, and manually create a datetime.

```moonbit check
///|
test {
  // creates a UTC+8 fixed time zone.
  let zone = @time.fixed_zone("Asia/Shanghai", 8 * 60 * 60)

  // creates a ZonedDateTime from unix second and time zone.
  let date_time = try? @time.unix(1714227729L, nanosecond=1000, zone~)
  inspect(
    date_time,
    content="Ok(2024-04-27T22:22:09.000001+08:00[Asia/Shanghai])",
  )
}
```

## TODOs

- Convert from/to RFC format string.
- Custom string formatter.
- Support the time zone offset transition at daylight saving time.
- Support monotonic clock to accurately measure the elapsed time.
- Support different calendar system, such as Chinese calendar system.
