# Changelog

## [Unreleased]

## [0.4.40]

### Changed

- Updated MoonBit toolchain support to v0.7.2 (#224)

## [0.4.39]

### Fixed

- Fixed the type of `@sys.exit` for native ffi (#221)

### Changed

- Updated MoonBit toolchain support to v0.7.1 (#223)

## [0.4.38]

### Fixed

- Fix path library `basename` and `dirname` for some edge cases (#211)

## [0.4.37]

### Added

- Added `@path` package for path handling (#208)
- Added `@sys.get_env_var` for accessing single environment varialbe (#201)
- Added `@codec/base64` (#174)

### Fixed

- The `read_dir` for native backend will not skip hidden files/diretories (#210)

### Changed

- Updated MoonBit toolchain support to v0.6.32 (#209)

## [0.4.36]

### Changed

- Prepare for next MoonBit toolchain support (#197)

## [0.4.35]

### Added

- Added `@decimal` package for arbitrary precision decimal (#183)

### Changed

- Updated MoonBit toolchain support to v0.6.29 (#183, #195)
- `@time` package is rewritten to use `lexmatch` (#194)

## [0.4.34]

### Fixed

- Updated MoonBit toolchain support to 0.6.26, updated C FFI annotations (#181)
- Improved performance on `@crypto.uint_to_hex_string` (#179)

## [0.4.33]

### Fixed

- Updated MoonBit toolchain support to 0.6.26, updated info file (#180)

### Changed

- Deprecated `bench` package. It is suggested to use the
  [builtin benchmark](https://docs.moonbitlang.com/en/latest/language/benchmarks.html)
  functionality (#177)

## [0.4.32]

### Added

- Added `ByteSource` trait for `@crypto` such that it accepts `FixedArray[Byte]`
  `Bytes` `@bytes.View` at the same time. (#165)
- Added `CryptoHasher` trait for `@crypto` (#142)
- Added `hmac` support based on `CryptoHasher` (#142)

### Fixed

- Updated the READMEs by switching to `.mbt.md` format (#164)
- Fixed the overflow for rational's equality check (#167)
- Refactored ChaCha series to make them faster (#170)
- Updated MoonBit toolchain support to 0.6.22, updated info file (#169)
- Updated MoonBit toolchain support to 0.6.24, updated info file (#175)

### Changed

- Deprecated `Num` trait since it has never been open and no one can implement
  it (#164)
- Deprecated `Stack::peek_exn` and replace it with `Stack::unsafe_peek` (#164)
- Renamed `MD5Context` to `MD5`, `SM3Context` to `SM3`, `Sha256Context` to
  `Sha256` in `@crypto` (#142)
- `SHA256` and `SM3` can now `update` after `finalize` (#169)
- Deprecated `@crypto.chachax` series and replace them with `@crypto.ChaCha`
  (#173)

## [0.4.31]

### Added

- Added `@rational` package, which was in the core library (#161)
- Added `Stack::from_iter` `Stack::iter` for conversion between different data
  (#162)

### Fixed

- Updated MoonBit toolchain support to 0.6.21, including syntax and the String
  APIs used internally (#162)

### Changed

- Deprecated `Stack::from_list` `Stack::push_list` `Stack::to_list` since the
  `@immut/list` is deprecated (#162)
- Updated the license headers to year 2025 (#162)
