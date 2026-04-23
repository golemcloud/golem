// Copyright 2025 International Digital Economy Academy
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

///|
test "basename/dirname" {
  inspect(Path::basename("a\\b"), content="b")
  inspect(Path::dirname("a\\b"), content="a")
  inspect(Path::basename("a\\b\\"))
  inspect(Path::dirname("a\\b\\"), content="a\\b")
}

///|
test "dirname/edge cases" {
  inspect(Path::dirname("\\"), content="\\")
  inspect(Path::dirname("\\\\"), content="\\\\")
  inspect(Path::dirname("\\\\\\"), content="\\\\\\")
  inspect(Path::dirname("\\\\\\\\"), content="\\\\\\\\")
  inspect(Path::dirname("a\\"), content="a")
  inspect(Path::dirname("a\\\\"), content="a")
  inspect(Path::dirname("a\\\\\\"), content="a")
  inspect(Path::dirname("a\\\\\\\\"), content="a")
  inspect(Path::dirname("\\a"), content="\\")
  inspect(Path::dirname("\\\\a"), content="\\\\")
  inspect(Path::dirname("\\\\\\a"), content="\\\\\\")
  inspect(Path::dirname("\\\\\\\\a"), content="\\\\\\\\")
}

///|
test "extname" {
  inspect(Path::extname("project\\main.mbt.md"), content=".md")
  inspect(Path::extname("project\\"), content="")
}

///|
test "is_absolute" {
  // absolute path cases
  //
  // verbatim symlink 
  json_inspect(Path::is_absolute("\\\\?\\GLOBALROOT\\file"), content=true)
  // verbatim unc 
  json_inspect(
    Path::is_absolute("\\\\?\\UNC\\server\\share\\file"),
    content=true,
  )
  // verbatim volume letter
  json_inspect(Path::is_absolute("\\\\?\\C:\\file"), content=true)
  // verbatim volume guid 
  json_inspect(
    Path::is_absolute(
      "\\\\?\\Volume{12345678-1234-1234-1234-1234567890ab}\\file",
    ),
    content=true,
  )
  // device namespace
  json_inspect(Path::is_absolute("\\\\.\\COM56"), content=true)
  // unc 
  json_inspect(Path::is_absolute("\\\\server\\share\\file"), content=true)
  // volume letter root 
  json_inspect(Path::is_absolute("C:\\"), content=true)
  // relative path cases
  // 
  // volume letter relative path
  json_inspect(Path::is_absolute("C:folder\\file"), content=false)
  json_inspect(Path::is_absolute("Users\\user"), content=false)
  json_inspect(Path::is_absolute(""), content=false)
}

///|
test "join" {
  inspect(Path::join("a", "\\b"), content="\\b")
  inspect(Path::join("a\\", "b"), content="a\\b")
  inspect(Path::join("a", "b"), content="a\\b")
}

///|
test "relative" {
  let from = "C:\\Users\\user_name"
  let to = "C:\\Users\\user_name\\proj_a"
  inspect(Path::relative(base=from, to), content="proj_a")
  let from = "C:\\Users\\user_name\\proj_a"
  let to = "C:\\Users\\user_name"
  inspect(Path::relative(base=from, to), content="..")
  let from = "C:\\Users\\user_name"
  let to = "C:\\Users\\user_name"
  inspect(Path::relative(base=from, to), content="")
  let from = "C:\\Users\\user_name\\proj_a"
  let to = "C:\\Users\\user_name\\proj_b"
  inspect(Path::relative(base=from, to), content="..\\proj_b")
}

///|
test "normalize" {
  inspect(Path::normalize("a\\.\\b\\..\\c\\"), content="a\\c")
  inspect(Path::normalize("\\a\\b\\..\\..\\c\\."), content="\\c")
  inspect(Path::normalize("a\\b\\c\\.."), content="a\\b")
}

///|
test "resolve" {
  inspect(Path::resolve("\\a\\..\\b\\c"), content="\\b\\c")
  inspect(Path::resolve("\\a\\b\\c\\..\\..\\.."), content="\\")
}

///|
test "basename edge cases" {
  // Empty path
  inspect(Path::basename(""), content="")

  // Root paths
  inspect(Path::basename("\\"), content="")
  inspect(Path::basename("\\\\"), content="")
  inspect(Path::basename("\\\\\\"), content="")
  inspect(Path::basename("C:\\"), content="")

  // Single character paths
  inspect(Path::basename("a"), content="a")
  inspect(Path::basename("\\a"), content="a")
  inspect(Path::basename("C:a"), content="a")
  inspect(Path::basename("C:\\a"), content="a")

  // Paths with trailing backslashes
  inspect(Path::basename("a\\"), content="")
  inspect(Path::basename("a\\b\\"), content="")
  inspect(Path::basename("C:\\a\\b\\"), content="")
  inspect(Path::basename("a\\b\\\\"), content="")

  // Hidden files (starting with dot)
  inspect(Path::basename("C:\\a\\.hidden"), content=".hidden")
  inspect(Path::basename("C:\\.hidden"), content=".hidden")
  inspect(Path::basename(".hidden"), content=".hidden")
  inspect(Path::basename("C:\\a\\."), content=".")
  inspect(Path::basename("C:\\a\\.."), content="..")

  // Complex paths
  inspect(Path::basename(".\\a\\b"), content="b")
  inspect(Path::basename("..\\a\\b"), content="b")
  inspect(Path::basename("a\\..\\b"), content="b")
  inspect(Path::basename("C:\\a\\..\\b"), content="b")

  // Paths with special characters and spaces
  inspect(Path::basename("C:\\a\\b c"), content="b c")
  inspect(Path::basename("C:\\a\\b-c"), content="b-c")
  inspect(Path::basename("C:\\a\\b_c"), content="b_c")
  inspect(Path::basename("C:\\a\\b.c.d"), content="b.c.d")
  inspect(Path::basename("C:\\Program Files\\app"), content="app")

  // UNC paths
  inspect(Path::basename("\\\\server\\share\\file"), content="file")
  inspect(Path::basename("\\\\server\\share\\folder\\file"), content="file")
  inspect(Path::basename("\\\\server\\share\\"), content="")

  // Verbatim paths
  inspect(Path::basename("\\\\?\\C:\\temp\\file"), content="file")
  inspect(Path::basename("\\\\?\\UNC\\server\\share\\file"), content="file")

  // Long paths
  inspect(
    Path::basename("C:\\very\\long\\path\\with\\many\\components\\final"),
    content="final",
  )

  // Paths with multiple consecutive backslashes
  inspect(Path::basename("a\\\\b"), content="b")
  inspect(Path::basename("a\\\\\\b"), content="b")
  inspect(Path::basename("C:\\a\\\\b\\\\c"), content="c")
}

///|
test "dirname edge cases" {
  // Empty path
  inspect(Path::dirname(""), content="")

  // Root paths
  inspect(Path::dirname("\\"), content="\\")
  inspect(Path::dirname("\\\\"), content="\\\\")
  inspect(Path::dirname("\\\\\\"), content="\\\\\\")
  inspect(Path::dirname("C:\\"), content="C:\\")
  inspect(Path::dirname("C:"), content="C:")

  // Single character paths
  inspect(Path::dirname("a"), content="")
  inspect(Path::dirname("\\a"), content="\\")
  inspect(Path::dirname("C:a"), content="C:")
  inspect(Path::dirname("C:\\a"), content="C:\\")

  // Paths with trailing backslashes
  inspect(Path::dirname("a\\"), content="a")
  inspect(Path::dirname("C:\\a\\"), content="C:\\a")
  inspect(Path::dirname("a\\b\\"), content="a\\b")

  // Hidden files
  inspect(Path::dirname("C:\\.hidden"), content="C:\\")
  inspect(Path::dirname("C:\\a\\.hidden"), content="C:\\a")
  inspect(Path::dirname(".hidden"), content="")
  inspect(Path::dirname("C:\\a\\."), content="C:\\a")
  inspect(Path::dirname("C:\\a\\.."), content="C:\\a")

  // Complex paths
  inspect(Path::dirname(".\\a"), content=".")
  inspect(Path::dirname("..\\a"), content="..")
  inspect(Path::dirname("a\\..\\b"), content="a\\..")
  inspect(Path::dirname("C:\\a\\..\\b"), content="C:\\a\\..")

  // UNC paths
  inspect(
    Path::dirname("\\\\server\\share\\file"),
    content="\\\\server\\share\\",
  )
  inspect(
    Path::dirname("\\\\server\\share\\folder\\file"),
    content="\\\\server\\share\\folder",
  )
  inspect(Path::dirname("\\\\server\\share"), content="\\\\server\\share")

  // Verbatim paths
  inspect(Path::dirname("\\\\?\\C:\\temp\\file"), content="\\\\?\\C:\\temp")
  inspect(
    Path::dirname("\\\\?\\UNC\\server\\share\\file"),
    content="\\\\?\\UNC\\server\\share",
  )

  // Paths with multiple consecutive backslashes
  inspect(Path::dirname("a\\\\b"), content="a")
  inspect(Path::dirname("a\\\\\\b"), content="a")
  inspect(Path::dirname("C:\\a\\\\b\\\\c"), content="C:\\a\\\\b")

  // Long paths
  inspect(
    Path::dirname("C:\\very\\long\\path\\with\\many\\components"),
    content="C:\\very\\long\\path\\with\\many",
  )
}

///|
test "extname edge cases" {
  // Empty path and no extension
  inspect(Path::extname(""), content="")
  inspect(Path::extname("file"), content="")
  inspect(Path::extname("C:\\path\\file"), content="")

  // Hidden files
  inspect(Path::extname(".hidden"), content=".hidden")
  inspect(Path::extname(".hidden.txt"), content=".txt")
  inspect(Path::extname("C:\\path\\.hidden"), content=".hidden")
  inspect(Path::extname("C:\\path\\.hidden.txt"), content=".txt")

  // Dot files and directories
  inspect(Path::extname("."), content=".")
  inspect(Path::extname(".."), content=".")
  inspect(Path::extname(".\\file"), content="")
  inspect(Path::extname("..\\file"), content="")
  inspect(Path::extname("C:\\a\\."), content=".")
  inspect(Path::extname("C:\\a\\.."), content=".")

  // Multiple extensions
  inspect(Path::extname("file.tar.gz"), content=".gz")
  inspect(Path::extname("archive.tar.bz2"), content=".bz2")
  inspect(Path::extname("config.json.bak"), content=".bak")

  // Extensions with special characters
  inspect(Path::extname("file.c++"), content=".c++")
  inspect(Path::extname("file.123"), content=".123")
  inspect(Path::extname("file.-"), content=".-")

  // Paths with trailing backslashes
  inspect(Path::extname("file.txt\\"), content="")
  inspect(Path::extname("C:\\path\\file.txt\\"), content="")

  // Extensions at different positions
  inspect(Path::extname(".txt"), content=".txt")
  inspect(Path::extname("a.b.c"), content=".c")
  inspect(Path::extname("a."), content=".")
  inspect(Path::extname("C:\\path\\a."), content=".")

  // Windows specific extensions
  inspect(Path::extname("program.exe"), content=".exe")
  inspect(Path::extname("library.dll"), content=".dll")
  inspect(Path::extname("config.ini"), content=".ini")
  inspect(Path::extname("batch.bat"), content=".bat")
  inspect(Path::extname("command.cmd"), content=".cmd")

  // UNC paths with extensions
  inspect(Path::extname("\\\\server\\share\\file.txt"), content=".txt")
  inspect(Path::extname("\\\\?\\C:\\temp\\file.log"), content=".log")

  // Very long extensions
  inspect(Path::extname("file.verylongextension"), content=".verylongextension")
}

///|
test "is_absolute advanced edge cases" {
  // Empty and whitespace
  json_inspect(Path::is_absolute(""), content=false)
  json_inspect(Path::is_absolute(" "), content=false)
  json_inspect(Path::is_absolute("  \\"), content=false)

  // Basic absolute paths
  json_inspect(Path::is_absolute("\\"), content=true)
  json_inspect(Path::is_absolute("\\\\"), content=true) // This might be interpreted as start of UNC
  json_inspect(Path::is_absolute("\\\\\\"), content=true)
  json_inspect(Path::is_absolute("C:\\"), content=true)
  json_inspect(Path::is_absolute("D:\\temp"), content=true)

  // Relative paths with drive letters
  json_inspect(Path::is_absolute("C:"), content=false)
  json_inspect(Path::is_absolute("C:temp"), content=false)
  json_inspect(Path::is_absolute("C:temp\\file"), content=false)
  json_inspect(Path::is_absolute("Z:relative"), content=false)

  // Various UNC path formats
  json_inspect(Path::is_absolute("\\\\server\\share"), content=true)
  json_inspect(Path::is_absolute("\\\\server\\share\\"), content=true)
  json_inspect(Path::is_absolute("\\\\server\\share\\file"), content=true)
  json_inspect(Path::is_absolute("\\\\localhost\\c$\\temp"), content=true)
  json_inspect(Path::is_absolute("\\\\127.0.0.1\\share\\file"), content=true)

  // Verbatim path formats
  json_inspect(Path::is_absolute("\\\\?\\C:\\"), content=true)
  json_inspect(Path::is_absolute("\\\\?\\C:\\temp"), content=true)
  json_inspect(Path::is_absolute("\\\\?\\D:\\very\\long\\path"), content=true)

  // Verbatim UNC paths
  json_inspect(Path::is_absolute("\\\\?\\UNC\\server\\share"), content=true)
  json_inspect(
    Path::is_absolute("\\\\?\\UNC\\server\\share\\file"),
    content=true,
  )

  // Device namespace paths
  json_inspect(Path::is_absolute("\\\\.\\COM1"), content=true)
  json_inspect(Path::is_absolute("\\\\.\\LPT1"), content=true)
  json_inspect(Path::is_absolute("\\\\.\\PRN"), content=true)
  json_inspect(Path::is_absolute("\\\\.\\AUX"), content=true)
  json_inspect(Path::is_absolute("\\\\.\\NUL"), content=true)
  json_inspect(Path::is_absolute("\\\\.\\CON"), content=true)
  json_inspect(Path::is_absolute("\\\\.\\PhysicalDrive0"), content=true)

  // Volume GUID paths
  json_inspect(
    Path::is_absolute("\\\\?\\Volume{12345678-1234-1234-1234-1234567890ab}\\"),
    content=true,
  )
  json_inspect(
    Path::is_absolute(
      "\\\\?\\Volume{12345678-1234-1234-1234-1234567890ab}\\temp",
    ),
    content=true,
  )

  // Global root paths
  json_inspect(
    Path::is_absolute("\\\\?\\GLOBALROOT\\Device\\HarddiskVolume1\\"),
    content=true,
  )
  json_inspect(
    Path::is_absolute("\\\\?\\GLOBALROOT\\Device\\HarddiskVolume1\\temp"),
    content=true,
  )

  // Relative paths
  json_inspect(Path::is_absolute("temp"), content=false)
  json_inspect(Path::is_absolute("temp\\file"), content=false)
  json_inspect(Path::is_absolute(".\\temp"), content=false)
  json_inspect(Path::is_absolute("..\\temp"), content=false)
  json_inspect(Path::is_absolute("folder\\subfolder"), content=false)

  // Edge cases with dots
  json_inspect(Path::is_absolute("."), content=false)
  json_inspect(Path::is_absolute(".."), content=false)
  json_inspect(Path::is_absolute(".\\"), content=false)
  json_inspect(Path::is_absolute("..\\"), content=false)
  json_inspect(Path::is_absolute("\\."), content=true)
  json_inspect(Path::is_absolute("\\.."), content=true)

  // Case sensitivity tests (Windows is case-insensitive)
  json_inspect(Path::is_absolute("c:\\"), content=true)
  json_inspect(Path::is_absolute("C:\\"), content=true)
  json_inspect(Path::is_absolute("z:\\temp"), content=true)
  json_inspect(Path::is_absolute("Z:\\temp"), content=true)
}

///|
test "join edge cases" {
  // Empty strings
  inspect(Path::join("", ""), content="")
  inspect(Path::join("a", ""), content="a\\")
  inspect(Path::join("", "b"), content="b")

  // Both paths are absolute - second wins
  inspect(Path::join("C:\\a", "D:\\b"), content="D:\\b")
  inspect(Path::join("C:\\a\\c", "\\b"), content="\\b")
  inspect(
    Path::join("\\\\server1\\share1", "\\\\server2\\share2"),
    content="\\\\server2\\share2",
  )

  // Drive letter combinations
  inspect(Path::join("C:", "temp"), content="C:\\temp")
  inspect(Path::join("C:\\", "temp"), content="C:\\temp")
  inspect(Path::join("C:\\folder", "temp"), content="C:\\folder\\temp")
  inspect(Path::join("C:folder", "temp"), content="C:folder\\temp")

  // Various separator combinations
  inspect(Path::join("a", "b\\c"), content="a\\b\\c")
  inspect(Path::join("a\\", "b\\c"), content="a\\b\\c")
  inspect(Path::join("a\\\\", "b"), content="a\\\\b")
  inspect(Path::join("a\\\\\\", "b"), content="a\\\\\\b")

  // Relative path components
  inspect(Path::join("a", ".\\b"), content="a\\.\\b")
  inspect(Path::join("a", "..\\b"), content="a\\..\\b")
  inspect(Path::join(".\\a", "b"), content=".\\a\\b")
  inspect(Path::join("..\\a", "b"), content="..\\a\\b")

  // UNC paths
  inspect(
    Path::join("\\\\server\\share", "folder"),
    content="\\\\server\\share\\folder",
  )
  inspect(
    Path::join("\\\\server\\share\\", "folder"),
    content="\\\\server\\share\\folder",
  )

  // Verbatim paths
  inspect(
    Path::join("\\\\?\\C:\\temp", "file"),
    content="\\\\?\\C:\\temp\\file",
  )
  inspect(
    Path::join("\\\\?\\C:\\temp\\", "file"),
    content="\\\\?\\C:\\temp\\file",
  )

  // Root paths
  inspect(Path::join("\\", "a"), content="\\a")
  inspect(Path::join("\\", "\\a"), content="\\a")
  inspect(Path::join("\\\\", "a"), content="\\\\a")

  // Hidden files
  inspect(Path::join("a", ".hidden"), content="a\\.hidden")
  inspect(Path::join("C:\\a", ".hidden"), content="C:\\a\\.hidden")

  // Complex combinations
  inspect(Path::join("a\\b", "..\\c"), content="a\\b\\..\\c")
  inspect(Path::join("C:\\a\\b", "..\\c"), content="C:\\a\\b\\..\\c")
  inspect(Path::join("", "C:\\abs"), content="C:\\abs")

  // Multiple consecutive separators
  inspect(Path::join("a\\\\b", "c"), content="a\\\\b\\c")
  inspect(Path::join("a", "b\\\\c"), content="a\\b\\\\c")

  // Special Windows device names
  inspect(Path::join("C:\\temp", "CON"), content="C:\\temp\\CON")
  inspect(Path::join("C:\\temp", "PRN.txt"), content="C:\\temp\\PRN.txt")
}

///|
test "normalize edge cases" {
  // Empty path
  inspect(Path::normalize(""), content="")

  // Simple dot resolution
  inspect(Path::normalize("."), content="")
  inspect(Path::normalize(".\\"), content="")
  inspect(Path::normalize("a\\."), content="a")
  inspect(Path::normalize(".\\a"), content="a")
  inspect(Path::normalize(".\\a\\"), content="a")
  inspect(Path::normalize("a\\.\\b"), content="a\\b")

  // Double dot resolution
  inspect(Path::normalize(".."), content="..")
  inspect(Path::normalize("..\\"), content="..")
  inspect(Path::normalize("a\\.."), content="")
  inspect(Path::normalize("a\\..\\"), content="")
  inspect(Path::normalize("..\\a"), content="..\\a")
  inspect(Path::normalize("a\\..\\b"), content="b")
  inspect(Path::normalize("a\\b\\.."), content="a")
  inspect(Path::normalize("a\\b\\..\\"), content="a")
  inspect(Path::normalize("a\\b\\..\\c"), content="a\\c")

  // Multiple consecutive separators
  inspect(Path::normalize("a\\\\b"), content="a\\b")
  inspect(Path::normalize("a\\\\\\b"), content="a\\b")
  inspect(Path::normalize("\\\\a\\\\b"), content="\\\\a\\\\b")
  inspect(Path::normalize("\\\\\\a\\\\\\b"), content="\\\\\\a\\\\\\b")

  // Drive letter paths
  inspect(Path::normalize("C:\\.\\temp"), content="C:\\temp")
  inspect(Path::normalize("C:\\..\\temp"), content="C:\\temp")
  inspect(Path::normalize("C:\\folder\\..\\temp"), content="C:\\temp")
  inspect(Path::normalize("C:temp\\.\\file"), content="C:temp\\file")

  // UNC path normalization
  inspect(
    Path::normalize("\\\\server\\share\\.\\folder"),
    content="\\\\server\\share\\folder",
  )
  inspect(
    Path::normalize("\\\\server\\share\\folder\\..\\file"),
    content="\\\\server\\share\\file",
  )

  // Complex combinations
  inspect(Path::normalize("a\\.\\b\\..\\c"), content="a\\c")
  inspect(Path::normalize(".\\a\\..\\b\\.\\c"), content="b\\c")
  inspect(Path::normalize("a\\b\\..\\..\\c"), content="c")
  inspect(Path::normalize("a\\b\\..\\..\\..\\c"), content="..\\c")

  // Root paths with dots
  inspect(Path::normalize("\\."), content="\\")
  inspect(Path::normalize("\\./"), content="\\./")
  inspect(Path::normalize("\\.."), content="\\")
  inspect(Path::normalize("\\..\\"), content="\\")
  inspect(Path::normalize("\\a\\.."), content="\\")
  inspect(Path::normalize("\\a\\..\\"), content="\\")
  inspect(Path::normalize("\\a\\..\\b"), content="\\b")

  // Preserve trailing backslashes where appropriate
  inspect(Path::normalize("a\\b\\c\\"), content="a\\b\\c")
  inspect(Path::normalize("a\\.\\b\\..\\c\\"), content="a\\c")

  // Hidden files
  inspect(Path::normalize(".\\.hidden"), content=".hidden")
  inspect(Path::normalize("a\\.\\.hidden"), content="a\\.hidden")
  inspect(Path::normalize("a\\..\\..hidden"), content="..hidden")

  // Verbatim paths (should preserve structure)
  inspect(
    Path::normalize("\\\\?\\C:\\temp\\.\\file"),
    content="\\\\?\\C:\\temp\\file",
  )
  inspect(
    Path::normalize("\\\\?\\UNC\\server\\share\\folder\\..\\file"),
    content="\\\\?\\UNC\\server\\share\\file",
  )
}

///|
test "relative edge cases" {
  // Same paths
  let same1 = "C:\\Users\\user"
  let same2 = "C:\\Users\\user"
  inspect(Path::relative(base=same1, same2), content="")

  // Same paths with different normalization
  inspect(Path::relative(base="C:\\a\\b", "C:\\a\\.\\b"), content="")
  inspect(Path::relative(base="C:\\a\\b", "C:\\a\\c\\..\\b"), content="")

  // Different drives should return empty (different prefixes)
  inspect(Path::relative(base="C:\\temp", "D:\\temp"), content="")
  inspect(Path::relative(base="C:\\Users", "D:\\Files"), content="")

  // Root to various paths
  inspect(Path::relative(base="C:\\", "C:\\Users"), content="Users")
  inspect(Path::relative(base="C:\\Users", "C:\\"), content="..")

  // Deep nesting
  inspect(
    Path::relative(base="C:\\a\\b\\c\\d", "C:\\a\\b\\e\\f"),
    content="..\\..\\e\\f",
  )
  inspect(
    Path::relative(base="C:\\a\\b\\c\\d\\e", "C:\\a\\f"),
    content="..\\..\\..\\..\\f",
  )

  // Hidden files and directories
  inspect(
    Path::relative(base="C:\\Users\\.config", "C:\\Users\\.local"),
    content="..\\.local",
  )
  inspect(
    Path::relative(base="C:\\Users", "C:\\Users\\.hidden"),
    content=".hidden",
  )

  // Paths with dots
  inspect(
    Path::relative(base="C:\\Users\\user", "C:\\Users\\user\\."),
    content="",
  )
  inspect(
    Path::relative(base="C:\\Users\\user\\.", "C:\\Users\\user"),
    content="",
  )
  inspect(
    Path::relative(base="C:\\Users\\user", "C:\\Users\\user\\.."),
    content="..",
  )

  // UNC paths
  inspect(
    Path::relative(
      base="\\\\server\\share\\folder1",
      "\\\\server\\share\\folder2",
    ),
    content="..\\folder2",
  )
  inspect(
    Path::relative(base="\\\\server1\\share", "\\\\server2\\share"),
    content="",
  ) // Different prefixes

  // Complex relative navigation
  inspect(
    Path::relative(base="C:\\a\\b\\c", "C:\\a\\b\\c\\..\\d"),
    content="..\\d",
  )
  inspect(
    Path::relative(base="C:\\a\\b\\c\\..\\d", "C:\\a\\b\\c"),
    content="..\\c",
  )

  // Mixed case drives (Windows is case-insensitive, but let's test)
  inspect(Path::relative(base="c:\\temp", "C:\\temp\\file"), content="")
  inspect(Path::relative(base="C:\\TEMP", "c:\\temp\\FILE"), content="")
}

///|
test "resolve edge cases" {
  // Already absolute paths should be normalized
  inspect(Path::resolve("C:\\a\\.\\b"), content="C:\\a\\b")
  inspect(Path::resolve("C:\\a\\..\\b"), content="C:\\b")
  inspect(Path::resolve("C:\\a\\b\\..\\..\\c"), content="C:\\c")

  // UNC paths
  inspect(
    Path::resolve("\\\\server\\share\\.\\file"),
    content="\\\\server\\share\\file",
  )
  inspect(
    Path::resolve("\\\\server\\share\\folder\\..\\file"),
    content="\\\\server\\share\\file",
  )

  // Relative paths with current drive
  // Note: These tests assume we're running on Windows with a current drive
  // In cross-platform testing, these might need to be conditional

  // Verbatim paths
  inspect(
    Path::resolve("\\\\?\\C:\\temp\\.\\file"),
    content="\\\\?\\C:\\temp\\file",
  )
}

///|
test "constants and separators" {
  // Test delimiter constant (semicolon on Windows)
  inspect(delimiter, content=";")

  // Test separator constant (backslash on Windows)
  inspect(sep, content="\\")
}

///|
test "Windows-specific path behaviors" {
  // Test case insensitivity awareness (functions should handle but preserve case)
  inspect(Path::basename("C:\\Temp\\File.TXT"), content="File.TXT")
  inspect(Path::dirname("C:\\TEMP\\file.txt"), content="C:\\TEMP")
  inspect(Path::extname("FILE.EXE"), content=".EXE")

  // Test drive letter edge cases
  json_inspect(Path::is_absolute("A:"), content=false) // Relative to current dir on drive A
  json_inspect(Path::is_absolute("Z:\\"), content=true) // Root of drive Z
  inspect(Path::join("C:", "temp"), content="C:\\temp") // Relative path on drive C
  inspect(Path::join("C:\\", "temp"), content="C:\\temp") // Absolute path on drive C

  // Test UNC path components
  inspect(Path::basename("\\\\server\\share"), content="")
  inspect(Path::dirname("\\\\server\\share"), content="\\\\server\\share")
  inspect(Path::dirname("\\\\server"), content="\\\\")

  // Test verbatim path handling
  inspect(Path::basename("\\\\?\\C:\\very\\long\\path\\file"), content="file")
  inspect(
    Path::dirname("\\\\?\\C:\\very\\long\\path\\file"),
    content="\\\\?\\C:\\very\\long\\path",
  )

  // Test device namespace paths
  inspect(Path::basename("\\\\.\\COM1"), content="")
  inspect(Path::dirname("\\\\.\\COM1"), content="\\\\.\\COM1")
}

///|
test "join/windows relative path but specifies drive letter" {
  inspect(Path::join("lhs", "C:rhs"), content="C:rhs")
  inspect(Path::join("C:lhs", "C:rhs"), content="C:lhs\\rhs")
  inspect(Path::join("C:lhs", "D:rhs"), content="D:rhs")
}
