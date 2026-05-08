# Path Utilities for Windows Systems

This package provides path manipulation utilities specifically designed for Windows systems. It handles paths using backslashes (`\`) as the path separator and supports various Windows path formats including UNC paths, device paths, and volume identifiers.

## Overview

The package offers a complete set of functions for working with Windows file paths:

- Path decomposition: `basename`, `dirname`, `extname`
- Path testing: `is_absolute`
- Path construction: `join`
- Path transformation: `normalize`, `relative`, `resolve`
- Platform constants: `sep`, `delimiter`

## Basic Path Operations

### Basename and Dirname

Extract the last component of a path or get the directory part:

```moonbit check
///|
test "basename and dirname examples" {
  // Get the last component (filename)
  let path : Path = "C:\\Users\\user"
  inspect(path.basename(), content="user")
  let path : Path = "project\\src\\main.mbt"
  inspect(path.basename(), content="main.mbt")

  // Get the directory part
  let path : Path = "C:\\Users\\user"
  inspect(path.dirname(), content="C:\\Users")
  let path : Path = "project\\src\\main.mbt"
  inspect(path.dirname(), content="project\\src")

  // Handle trailing backslashes
  let path : Path = "C:\\Users\\"
  inspect(path.basename(), content="")
  inspect(path.dirname(), content="C:\\Users")
}
```

### Extension Name

Extract file extensions from paths:

```moonbit check
///|
test "extension extraction" {
  // Get file extension including the dot
  let path : Path = "document.txt"
  inspect(path.extname(), content=".txt")
  let path : Path = "archive.tar.gz"
  inspect(path.extname(), content=".gz")
  let path : Path = "project\\main.mbt.md"
  inspect(path.extname(), content=".md")
  // Files without extensions
  let path : Path = "README"
  inspect(path.extname(), content="")
  let path : Path = "project\\"
  inspect(path.extname(), content="")
}
```

## Path Testing

### Absolute Path Detection

Windows has various types of absolute paths. The function correctly identifies them all:

```moonbit check
///|
test "absolute path detection" {
  // Standard drive letter paths
  let path : Path = "C:\\"
  json_inspect(path.is_absolute(), content=true)
  let path : Path = "D:\\folder\\file"
  json_inspect(path.is_absolute(), content=true)
  // UNC paths (network shares)
  let path : Path = "\\\\server\\share\\file"
  json_inspect(path.is_absolute(), content=true)
  // Verbatim UNC paths
  let path : Path = "\\\\?\\UNC\\server\\share\\file"
  json_inspect(path.is_absolute(), content=true)
  // Verbatim drive letter paths
  let path : Path = "\\\\?\\C:\\file"
  json_inspect(path.is_absolute(), content=true)
  // Volume GUID paths
  let path : Path = "\\\\?\\Volume{12345678-1234-1234-1234-1234567890ab}\\file"
  json_inspect(path.is_absolute(), content=true)
  // Device namespace paths
  let path : Path = "\\\\.\\COM56"
  json_inspect(path.is_absolute(), content=true)
  // Verbatim symlink paths
  let path : Path = "\\\\?\\GLOBALROOT\\file"
  json_inspect(path.is_absolute(), content=true)
  // Relative paths
  let path : Path = "C:folder\\file" // Drive-relative
  json_inspect(path.is_absolute(), content=false)
  let path : Path = "Users\\user"
  json_inspect(path.is_absolute(), content=false)
  let path : Path = ""
  json_inspect(path.is_absolute(), content=false)
}
```

## Path Construction

### Joining Paths

Combine path components with proper separator handling:

```moonbit check
///|
test "path joining" {
  // Basic joining
  let path : Path = "Users"
  inspect(path.join("user"), content="Users\\user")
  let path : Path = "project"
  inspect(path.join("src"), content="project\\src")
  // Handle trailing backslashes
  let path : Path = "Users\\"
  inspect(path.join("user"), content="Users\\user")
  // Absolute paths override
  let path : Path = "relative"
  inspect(path.join("\\absolute"), content="\\absolute")
  let path : Path = "C:\\"
  inspect(
    path.join("folder").join("file.txt").to_string(),
    content="C:\\folder\\file.txt",
  )
}
```

## Path Transformation

### Normalization

Clean up redundant components and resolve `.` and `..`:

```moonbit check
///|
test "path normalization" {
  // Remove redundant components
  let path : Path = "a\\.\\b\\..\\c\\"
  inspect(path.normalize(), content="a\\c")
  let path : Path = "C:\\Users\\..\\Windows"
  inspect(path.normalize(), content="C:\\Windows")
  // Handle complex cases
  let path : Path = "\\a\\b\\..\\..\\c\\."
  inspect(path.normalize(), content="\\c")
  let path : Path = "a\\b\\c\\.."
  inspect(path.normalize(), content="a\\b")
}
```

### Relative Paths

Calculate the relative path between two Windows locations:

```moonbit check
///|
test "relative path calculation" {
  // Same directory level
  let base = "C:\\Users\\user_name"
  let path : Path = "C:\\Users\\user_name\\proj_a"
  // inspect(path)
  inspect(path.relative(base~), content="proj_a")

  // Go up one level
  let base = "C:\\Users\\user_name\\proj_a"
  let path : Path = "C:\\Users\\user_name"
  inspect(path.relative(base~), content="..")
  // Same path
  let base = "C:\\Users\\user_name"
  let path : Path = "C:\\Users\\user_name"
  inspect(path.relative(base~), content="")

  // Sibling directories
  let base = "C:\\Users\\user_name\\proj_a"
  let path : Path = "C:\\Users\\user_name\\proj_b"
  inspect(path.relative(base~), content="..\\proj_b")
}
```

### Path Resolution

Convert relative paths to absolute paths and normalize them:

```moonbit check
///|
test "path resolution" {
  // Resolve and normalize absolute paths
  let path : Path = "\\Users\\..\\Windows\\System32"
  inspect(path.resolve(), content="\\Windows\\System32")
  let path : Path = "\\a\\b\\c\\..\\..\\.."
  inspect(path.resolve(), content="\\")

  // Note: resolve() with relative paths depends on current working directory
  // and will join with the current directory before normalizing
}
```

```moonbit skip nocheck
///|
test {
  let path : Path = "a\\b\\..\\c"
  inspect(path.resolve(), content="C:\\current\\working\\directory\\a\\c")
}
```



## Platform Constants

The package provides Windows-specific constants:

```moonbit check
///|
test "platform constants" {
  // Path component separator
  inspect(@win32.sep, content="\\")

  // Path list delimiter (for PATH environment variable)
  inspect(@win32.delimiter, content=";")
}
```

## Windows Path Types

Windows supports several types of absolute paths:

1. **Drive Letter Paths**: `C:\folder\file`
2. **UNC Paths**: `\\server\share\file` (network shares)
3. **Verbatim Paths**: `\\?\C:\file` (bypass path processing)
4. **Verbatim UNC**: `\\?\UNC\server\share\file`
5. **Volume GUID**: `\\?\Volume{guid}\file`
6. **Device Paths**: `\\.\device`
7. **Symlink Paths**: `\\?\GLOBALROOT\file`

The `is_absolute` function correctly identifies all these formats.

## Key Properties

The package maintains several important properties:

1. **Basename/Dirname relationship**: For most paths, joining dirname and basename gives the original path
2. **Relative/Join relationship**: `join(from, relative(base=from, to))` equals `normalize(to)`
3. **Idempotent normalization**: `normalize(normalize(path))` equals `normalize(path)`

## Edge Cases

The implementation handles various Windows-specific edge cases:

- Drive-relative paths (`C:folder`) are treated as relative, not absolute
- Empty paths return appropriate default values
- Trailing backslashes are preserved where semantically important
- All Windows path prefix types are properly recognized
- The functions are consistent with Python's `os.path` module behavior

This makes the package reliable for real-world path manipulation in Windows environments, handling the complexity of Windows path formats while providing a clean, consistent API.
