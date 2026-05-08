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
test "WinPrefix::VerbatimSymlink" {
  let path = WinPath::parse("\\\\?\\GLOBALROOT\\file")
  inspect(path, content="\\\\?\\GLOBALROOT\\file")
  let path = WinPath::parse("\\\\?\\GLOBALROOT\\file.txt")
  inspect(path, content="\\\\?\\GLOBALROOT\\file.txt")
  let path = WinPath::parse("\\\\?\\GLOBALROOT\\folder\\")
  inspect(path, content="\\\\?\\GLOBALROOT\\folder")
  let path = WinPath::parse("\\\\?\\GLOBALROOT")
  inspect(path, content="\\\\?\\GLOBALROOT\\")
}

///|
test "WinPrefix::VerbatimUNC" {
  let path = WinPath::parse("\\\\?\\UNC\\server\\share\\file")
  inspect(path, content="\\\\?\\UNC\\server\\share\\file")
  let path = WinPath::parse("\\\\?\\UNC\\server\\share\\file.log")
  inspect(path, content="\\\\?\\UNC\\server\\share\\file.log")
  let path = WinPath::parse("\\\\?\\UNC\\server\\share\\folder\\")
  inspect(path, content="\\\\?\\UNC\\server\\share\\folder")
  let path = WinPath::parse("\\\\?\\UNC\\server\\share")
  inspect(path, content="\\\\?\\UNC\\server\\share")
}

///|
test "WinPrefix::VerbatimVolumeLetter" {
  let path = WinPath::parse("\\\\?\\C:\\file")
  inspect(path, content="\\\\?\\C:\\file")
  let path = WinPath::parse("\\\\?\\C:\\file.md")
  inspect(path, content="\\\\?\\C:\\file.md")
  let path = WinPath::parse("\\\\?\\C:\\folder\\")
  inspect(path, content="\\\\?\\C:\\folder")
  let path = WinPath::parse("\\\\?\\C:")
  inspect(path, content="\\\\?\\C:\\")
}

///|
test "WinPrefix::VerbatimVolumeGUID" {
  let path = WinPath::parse(
    "\\\\?\\Volume{12345678-1234-1234-1234-1234567890AB}file",
  )
  inspect(
    path,
    content="\\\\?\\Volume{12345678-1234-1234-1234-1234567890AB}file\\",
  )
  let path = WinPath::parse(
    "\\\\?\\Volume{12345678-1234-1234-1234-1234567890AB}file.png",
  )
  inspect(
    path,
    content="\\\\?\\Volume{12345678-1234-1234-1234-1234567890AB}file.png\\",
  )
  let path = WinPath::parse(
    "\\\\?\\Volume{12345678-1234-1234-1234-1234567890AB}folder\\",
  )
  inspect(
    path,
    content="\\\\?\\Volume{12345678-1234-1234-1234-1234567890AB}folder\\",
  )
  let path = WinPath::parse(
    "\\\\?\\Volume{12345678-1234-1234-1234-1234567890AB}",
  )
  inspect(path, content="\\\\?\\Volume{12345678-1234-1234-1234-1234567890AB}\\")
}

///|
test "WinPrefix::DeviceNS" {
  let path = WinPath::parse("\\\\.\\COM56\\file")
  inspect(path, content="\\\\.\\COM56\\file")
  let path = WinPath::parse("\\\\.\\COM56\\file.bin")
  inspect(path, content="\\\\.\\COM56\\file.bin")
  let path = WinPath::parse("\\\\.\\COM56\\folder\\")
  inspect(path, content="\\\\.\\COM56\\folder")
  let path = WinPath::parse("\\\\.\\COM56")
  inspect(path, content="\\\\.\\COM56\\")
}

///|
test "WinPrefix::UNC" {
  let path = WinPath::parse("\\\\server\\share\\file")
  inspect(path, content="\\\\server\\share\\file")
  let path = WinPath::parse("\\\\server\\share\\file.log")
  inspect(path, content="\\\\server\\share\\file.log")
  let path = WinPath::parse("\\\\server\\share\\folder\\")
  inspect(path, content="\\\\server\\share\\folder")
  let path = WinPath::parse("\\\\server\\share")
  inspect(path, content="\\\\server\\share\\")
}

///|
test "WinPrefix::VolumeLetterRoot" {
  let path = WinPath::parse("C:\\")
  inspect(path, content="C:\\")
  let path = WinPath::parse("C:\\file")
  inspect(path, content="C:\\file")
  let path = WinPath::parse("C:\\file.txt")
  inspect(path, content="C:\\file.txt")
  let path = WinPath::parse("C:\\folder\\")
  inspect(path, content="C:\\folder")
}

///|
test "WinPrefix::VolumeLetter" {
  let path = WinPath::parse("C:main.mbt")
  inspect(path, content="C:main.mbt")
  let path = WinPath::parse("C:.\\..\\dir\\..\\main.mbt")
  inspect(path, content="C:main.mbt")
}

///|
test "Windows root path without specified drive letter" {
  let path = WinPath::parse("\\folder\\file")
  inspect(path, content="\\folder\\file")
  let path = WinPath::parse("\\..\\folder\\file")
  inspect(path, content="\\folder\\file")
}

///|
test "Windows path normalization" {
  let path = WinPath::parse(".\\a\\b\\.\\c\\..\\d\\..\\..")
  inspect(path, content="a")
  let path = WinPath::parse("\\a\\b\\..")
  inspect(path, content="\\a")
  let path = WinPath::parse("\\..\\..\\")
  inspect(path, content="\\")
  let path = WinPath::parse("\\..\\a\\b\\..\\c\\..\\..\\..")
  inspect(path, content="\\")
  let path = WinPath::parse("\\..\\a\\b\\..\\c\\..\\..")
  inspect(path, content="\\")
  let path = WinPath::parse("\\..\\a\\b\\..\\c")
  inspect(path, content="\\a\\c")
  let path = WinPath::parse("C:\\..\\..\\c\\..")
  inspect(path, content="C:\\")
}

///|
test "Windows relative path" {
  let path = WinPath::parse(".\\folder\\file")
  inspect(path, content="folder\\file")
  let path = WinPath::parse("containers\\finger_tree")
  inspect(path, content="containers\\finger_tree")
  let path = WinPath::parse("containers\\finger_tree\\finger_tree.mbt")
  inspect(path, content="containers\\finger_tree\\finger_tree.mbt")
}

///|
test "Windows inner path normalization for backslashes" {
  let path = WinPath::parse(".\\a\\\\b\\\\\\\\c")
  inspect(path, content="a\\b\\c")
}

///|
test "Windows backslashes normalization" {
  let path = WinPath::parse("C:\\\\\\\\a\\b")
  inspect(path, content="C:\\a\\b")
  let path = WinPath::parse(".\\\\\\a\\b\\c\\\\e")
  inspect(path, content="a\\b\\c\\e")
}

///|
test "Windows multiple tailing backslashes normalization" {
  let path = WinPath::parse("C:\\folder\\\\")
  inspect(path, content="C:\\folder")
  let path = WinPath::parse(".\\a\\b\\c\\\\\\")
  inspect(path, content="a\\b\\c")
  let path = WinPath::parse(".\\a\\b\\c\\\\\\")
  inspect(path, content="a\\b\\c")
}

///|
test "Windows edge cases" {
  let path = WinPath::parse("\\")
  inspect(path, content="\\")
  let path = WinPath::parse(".\\")
  inspect(path)
}
