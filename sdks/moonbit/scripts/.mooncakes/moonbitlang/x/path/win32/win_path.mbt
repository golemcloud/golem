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
/// [Microsoft Naming a Volume](https://learn.microsoft.com/en-us/windows/win32/fileio/naming-a-volume)
/// [Microsoft Naming Files, Paths, and Namespaces](https://learn.microsoft.com/en-us/windows/win32/fileio/naming-a-file)
/// [Microsoft Maximum Path Length Limitation](https://learn.microsoft.com/en-us/windows/win32/fileio/maximum-file-path-limitation)
priv struct WinPath {
  prefix : WinPrefix
  components : Array[StringView]
} derive(ToJson, Hash, Eq)

///|
/// 
/// #### Verbatim Prefix
/// 
/// refer to [Microsoft Win32 File Namespaces](https://learn.microsoft.com/en-us/windows/win32/fileio/naming-a-file#win32-file-namespaces)
/// > For file I/O,
/// > the "\\?\" prefix to a path string tells the Windows APIs to disable all string parsing and to send the string that follows it straight to the file system.
/// 
/// #### Windows Encoding 
/// 
/// refer to [Microsoft Path Unicode Normalization](https://learn.microsoft.com/en-us/windows/win32/fileio/maximum-file-path-limitation)
/// 
/// > There is no need to perform any Unicode normalization on path and file name strings for use by the Windows file I/O API functions 
/// > because the file system treats path and file names as an opaque sequence of WCHARs.
/// > Any normalization that your application requires should be performed with this in mind, 
/// > external of any calls to related Windows file I/O API functions.
/// 
/// #### Relative Paths Cannot use Verbatim Prefix
/// 
/// refer to [Microsoft Verbatim restriction](https://learn.microsoft.com/en-us/windows/win32/fileio/maximum-file-path-limitation)
/// 
/// > Because you cannot use the "\\?\" prefix with a relative path, 
/// > relative paths are always limited to a total of MAX_PATH characters.
/// 
priv enum WinPrefix {
  /// Syntax: `\`
  ///
  /// Example: `\Users\username\Documents\`
  Root
  /// Syntax: `\\?\<SYMLINK>\` 
  ///
  /// Example: `\\?\GLOBALROOT\`
  VerbatimSymlink(symlink~ : String)
  /// Syntax: `\\?\UNC\<SERVER>\<SHARE>\`
  ///
  /// Example: `\\?\UNC\server\share\`
  VerbatimUNC(hostname~ : String, shared_folder~ : String)
  /// Syntax: `\\?\<L>:\`
  ///
  /// Example: `\\?\C:\`
  VerbatimVolumeLetterRoot(Char)
  /// Syntax: `\\?\Volume{<GUID>}`
  ///
  /// Example: `\\?\Volume{26a21bda-a627-11d7-9931-806e6f6e6963}`
  ///
  /// Warning: this letter is case-insensitive.
  ///
  /// [Microsoft Enumerating Volume GUID Paths](https://learn.microsoft.com/en-us/windows/win32/fileio/enumerating-unique-volume-names)
  VerbatimVolumeGUID(String)
  /// Syntax: `\\.\<DEVICE>\`
  ///
  /// Example: `\\.\COM56\`
  ///
  /// [Microsoft Win32 Device Namespaces](https://learn.microsoft.com/en-us/windows/win32/fileio/naming-a-file#win32-device-namespaces)
  DeviceNS(device~ : String)
  /// Syntax: `\\<SERVER>\<SHARE>\`
  ///
  /// Example: `\\server\share\`
  UNC(hostname~ : String, shared_folder~ : String)
  /// Syntax: `<L>:\`
  ///
  /// Example: `C:\`
  ///
  /// Warning: this letter is case-insensitive.
  VolumeLetterRoot(Char)
  /// Syntax: `<L>:<rest_of_path>`
  ///
  /// Example: `C:main.mbt`
  ///
  /// 
  /// Warning: this letter is case-insensitive.
  VolumeLetter(Char)
  None
} derive(ToJson, Hash, Eq)

///|
let windows_reserved_names : FixedArray[String] = [
  ///
  "CON", "PRN", "AUX", "NUL",
  ///
   "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9",
  ///
   "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
  ///
   "COM\u00b9", "COM\u00b2", "COM\u00b3",
  ///
   "LPT\u00b9", "LPT\u00b2", "LPT\u00b3",
]

///|
let windows_reserved_names_set : Set[StringView] = Set::from_iter(
  windows_reserved_names.iter().map(x => x),
)

///|
fn is_windows_reserved_character(ch : Char) -> Bool {
  match ch {
    '<'
    | '>'
    | ':'
    | '\"'
    | '/'
    | '\\'
    | '|'
    | '?'
    | '*'
    | '\u0000'
    | '\u0001'..='\u001F' => true
    _ => false
  }
}

///|
fn windows_path_component_from(
  s : StringView,
  start~ : Int,
) -> StringView raise PathError {
  guard !s.is_empty() else {
    raise PathError::PathError(
      start~,
      message="Windows path: empty path component not allowed",
    )
  }
  guard !windows_reserved_names_set.contains(s) else {
    raise PathError::PathError(
      start~,
      message="Windows path: '\{s}' is a reserved name (CON, PRN, AUX, NUL, COM1-9, LPT1-9)",
    )
  }
  for i, ch in s {
    guard !is_windows_reserved_character(ch) else {
      raise PathError::PathError(
        start=start + i,
        message="Windows path: character '\{ch}' at position \{start+i} is not allowed (reserved characters: < > : \" / \\ | ? * and control characters)",
      )
    }
    guard ch != '\\' else {
      raise PathError::PathError(
        start=start + i,
        message="Windows path: backslash '\\' at position \{start+i} not allowed in path component",
      )
    }
  }
  s
}

///|
priv suberror PathError {
  PathError(start~ : Int, message~ : String)
} derive(Show)

///|
/// In order to make code more clean, 
/// so we have to define many string constants to avoid escape sequences 
/// and MoonBit directly use mutli-line string literals in callsite should wrap used ()

///|
const VerbatimPrefix : String =
  #|\\?\

///|
const VerbatimUncPrefix : String =
  #|\\?\UNC\

///|
const ColonBackslash : String =
  #|:\

///|
const VolumePrefix : String =
  #|\\?\Volume{

///|
const VolumeSuffix : String =
  #|}

///|
const DevicePrefix : String =
  #|\\.\

///|
const UncPrefix : String =
  #|\\

///|
const BackSlash : Char = '\\'

///|
impl Show for WinPrefix with output(self, logger) {
  match self {
    Root => logger.write_char(BackSlash)
    VerbatimSymlink(symlink~) => {
      logger.write_string(VerbatimPrefix)
      logger.write_string(symlink)
      logger.write_char(BackSlash)
    }
    VerbatimUNC(hostname~, shared_folder~) => {
      logger.write_string(VerbatimUncPrefix)
      logger.write_string(hostname)
      logger.write_char(BackSlash)
      logger.write_string(shared_folder)
      logger.write_char(BackSlash)
    }
    VerbatimVolumeLetterRoot(letter) => {
      logger.write_string(VerbatimPrefix)
      logger.write_char(letter)
      logger.write_string(ColonBackslash)
    }
    VerbatimVolumeGUID(guid) => {
      logger.write_string(VolumePrefix)
      logger.write_string(guid)
      logger.write_string(VolumeSuffix)
    }
    DeviceNS(device~) => {
      logger.write_string(DevicePrefix)
      logger.write_string(device)
      logger.write_char(BackSlash)
    }
    UNC(hostname~, shared_folder~) => {
      logger.write_string(UncPrefix)
      logger.write_string(hostname)
      logger.write_char(BackSlash)
      logger.write_string(shared_folder)
      logger.write_char(BackSlash)
    }
    VolumeLetterRoot(letter) => {
      logger.write_char(letter)
      logger.write_string(ColonBackslash)
    }
    VolumeLetter(letter) => {
      logger.write_char(letter)
      logger.write_string(":")
    }
    None => ()
  }
}

///|
fn WinPath::output(self : WinPath, logger : &Logger) -> Unit {
  let { prefix, components } = self
  logger.write_object(prefix)
  match components {
    [] => ()
    [x, .. xs] => {
      logger.write_string(x.to_string())
      for component in xs {
        logger.write_char(BackSlash)
        logger.write_string(component.to_string())
      }
    }
  }
}

///|
impl Show for WinPath with output(self, logger) {
  WinPath::output(self, logger)
}

///|
fn parse_windows_relative_path(
  s : StringView,
  start~ : Int,
  root_eliminate~ : Bool,
) -> Array[StringView] raise PathError {
  let mut rest = s
  let components : Array[StringView] = []
  let mut start = start
  for {
    lexmatch rest with longest {
      // pattern: <path_component><backslashes>
      // backslashes elimination
      ((("[^\\]+" as s) "(\\)+") as all, r) => {
        match s {
          "." => ()
          ".." =>
            match (components.pop(), root_eliminate) {
              (Some(".."), _) => {
                components.push("..")
                components.push("..")
              }
              (None, true) => ()
              (None, false) => components.push("..")
              (Some(_), _) => ()
            }
          _ => components.push(s)
        }
        start += all.char_length()
        rest = r
      }
      // pattern: <dot-dot>
      "\.\." => {
        match (components.pop(), root_eliminate) {
          (Some(".."), _) => {
            components.push("..")
            components.push("..")
          }
          (None, true) => ()
          (None, false) => components.push("..")
          (Some(_), _) => ()
        }
        break
      }
      // pattern: <dot>
      "\." => break
      ("[^\\]+" as filename) => {
        components.push(filename)
        break
      }
      "$" => break
      _ =>
        raise PathError(start~, message="Invalid Windows path in relative part")
    }
  }
  components
}

///|

///|
fn WinPath::parse(path : StringView) -> WinPath raise PathError {
  lexmatch path with longest {
    //-----------------------------------------------------------------------------
    // without <rest_of_path>
    //-----------------------------------------------------------------------------
    // this pattern exect match the whole path string
    // pattern: `\\?\<symlink>`
    ("\\\\\?\\" ("[^\\]+" as symlink)) =>
      WinPath::{
        prefix: VerbatimSymlink(symlink=symlink.to_string()),
        components: [],
      }
    // this pattern exect match the whole path string
    // pattern: `\\?\UNC\<hostname>\<shared_folder>`
    ("\\\\\?UNC\\" ("[^\\]+" as hostname) "\\" ("[^\\]+" as shared_folder)) =>
      WinPath::{
        prefix: VerbatimUNC(
          hostname=hostname.to_string(),
          shared_folder=shared_folder.to_string(),
        ),
        components: [],
      }
    // this pattern exect match the whole path string
    // pattern: `\\<hostname>\<shared_folder>`
    ("\\\\" ("[^\\]+" as hostname) "\\" ("[^\\]+" as shared_folder)) =>
      WinPath::{
        prefix: UNC(
          hostname=hostname.to_string(),
          shared_folder=shared_folder.to_string(),
        ),
        components: [],
      }
    // this pattern exect match the whole path string
    // pattern: `\\.\<device>`
    ("\\\\\.\\" ("[^\\]+" as device)) =>
      WinPath::{ prefix: DeviceNS(device=device.to_string()), components: [] }
    // this pattern exect match the whole path string
    // pattern: `\\?\<letter>:\`
    //
    // backslashes elimination
    ("\\\\\?\\" ("[a-zA-Z]" as letter) ":(\\)+") =>
      WinPath::{ prefix: VerbatimVolumeLetterRoot(letter), components: [] }
    // this pattern exect match the whole path string
    // pattern: `<letter>:\`
    // 
    // backslashes elimination
    (("[a-zA-Z]" as letter) ":(\\)+") =>
      WinPath::{ prefix: VolumeLetterRoot(letter), components: [] }
    // this pattern exect match the whole path string
    // pattern: `\\?\` followed by relative path components
    ("\\\\\?\\" "\\|\.\.|\.") =>
      raise PathError(
        start=0,
        message="Invalid Windows path: Verbatim prefix can use for relative path",
      )
    // this pattern exect match the whole path string
    // pattern: `\\?\Volume{<guid>}`
    ("\\\\\?\\Volume[{]" ("[^}]+" as guid) "[}]") =>
      WinPath::{ prefix: VerbatimVolumeGUID(guid.to_string()), components: [] }
    //-----------------------------------------------------------------------------
    // with <rest_of_path>
    //-----------------------------------------------------------------------------
    // pattern: `\\?\<symlink>\<rest_of_path>`
    //
    // backslashes elimination
    (("\\\\\?\\" ("[^\\]+" as symlink) "(\\)+") as prefix, rest) => {
      // Warning: won't check the symlink name here
      let components = parse_windows_relative_path(
        rest,
        start=prefix.char_length(),
        root_eliminate=true,
      )
      WinPath::{
        prefix: VerbatimSymlink(symlink=symlink.to_string()),
        components,
      }
    }
    // pattern: `\\?\UNC\<hostname>\<shared_folder>\<rest_of_path>`
    //
    // backslashes elimination
    (
      (
        "\\\\\?UNC\\"
        ("[^\\]+" as hostname)
        "\\"
        ("[^\\]+" as shared_folder)
        "(\\)+"
      ) as prefix,
      rest
    ) => {
      // Warning: won't check the hostname and shared_folder here
      let components = parse_windows_relative_path(
        rest,
        start=prefix.char_length(),
        root_eliminate=true,
      )
      WinPath::{
        prefix: VerbatimUNC(
          hostname=hostname.to_string(),
          shared_folder=shared_folder.to_string(),
        ),
        components,
      }
    }
    // pattern: `\\<hostname>\<shared_folder>\<rest_of_path>`
    //
    // backslashes elimination
    (
      ("\\\\" ("[^\\]+" as hostname) "\\" ("[^\\]+" as shared_folder) "(\\)+") as prefix,
      rest
    ) => {
      // Warning: won't check the hostname and shared_folder here
      let components = parse_windows_relative_path(
        rest,
        start=prefix.char_length(),
        root_eliminate=true,
      )
      WinPath::{
        prefix: UNC(
          hostname=hostname.to_string(),
          shared_folder=shared_folder.to_string(),
        ),
        components,
      }
    }
    // pattern: `\\?\Volume{<guid>}<rest_of_path>`
    (("\\\\\?\\Volume[{]" ("[^}]+" as guid) "[}]") as prefix, rest) => {
      let components = parse_windows_relative_path(
        rest,
        start=prefix.char_length(),
        root_eliminate=true,
      )
      WinPath::{ prefix: VerbatimVolumeGUID(guid.to_string()), components }
    }
    // pattern: `\\.\<device>\<rest_of_path>`
    //
    // backslashes elimination
    (("\\\\\.\\" ("[^\\]+" as device) "(\\)+") as prefix, rest) => {
      // check the device name here
      let components = parse_windows_relative_path(
        rest,
        start=prefix.char_length(),
        root_eliminate=true,
      )
      WinPath::{ prefix: DeviceNS(device=device.to_string()), components }
    }
    // pattern: `\\?\<letter>:\<rest_of_path>`
    //
    // backslashes elimination
    (("\\\\\?\\" ("[a-zA-Z]" as letter) ":(\\)+") as prefix, rest) => {
      let components = parse_windows_relative_path(
        rest,
        start=prefix.char_length(),
        root_eliminate=true,
      )
      WinPath::{ prefix: VerbatimVolumeLetterRoot(letter), components }
    }
    ("\\\\\?\\" "\\|\.\.|\.", _) =>
      // pattern: `\\?\` followed by relative path components
      raise PathError(
        start=0,
        message="Invalid Windows path: Verbatim prefix can use for relative path",
      )
    // root path but without derive letter 
    // syntax: `\<rest_of_path>`
    // Warning: here doesn't support backslash elimination, 
    //          because prefix backslashes elimination is conflicted with UNC path format
    ("\\", rest) => {
      let components = parse_windows_relative_path(
        rest,
        start=1,
        root_eliminate=true,
      )
      WinPath::{ prefix: Root, components }
    }

    // pattern: `<letter>:\<rest_of_path>`
    //
    // backslashes elimination
    ((("[a-zA-Z]" as letter) ":(\\)+") as prefix, rest) => {
      let components = parse_windows_relative_path(
        rest,
        start=prefix.char_length(),
        root_eliminate=true,
      )
      WinPath::{ prefix: VolumeLetterRoot(letter), components }
    }
    // pattern: `<letter>:<rest_of_path>`
    //
    // backslashes elimination
    ((("[a-zA-Z]" as letter) ":") as prefix, rest) => {
      let components = parse_windows_relative_path(
        rest,
        start=prefix.char_length(),
        root_eliminate=true,
      )
      WinPath::{ prefix: VolumeLetter(letter), components }
    }

    // syntax: `<path_component>\<rest_of_path>`
    // backslashes elimination
    ("[^a-zA-Z\\][^:][^\\]+(\\)+", _) => {
      let components = parse_windows_relative_path(
        path,
        start=0,
        root_eliminate=false,
      )
      WinPath::{ prefix: None, components }
    }
    // current path 
    // syntax: `.\<rest_of_path>`
    // backslashes elimination
    ("\.(\\)+" as prefix, rest) => {
      let components = parse_windows_relative_path(
        rest,
        start=prefix.char_length(),
        root_eliminate=false,
      )
      WinPath::{ prefix: None, components }
    }
    non_prefix_hint_path => {
      let components = parse_windows_relative_path(
        non_prefix_hint_path,
        start=0,
        root_eliminate=false,
      )
      WinPath::{ prefix: None, components }
    }
  }
}
