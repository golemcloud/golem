///|
pub fn path_basename(path : String) -> String {
  if path.rev_find("/") is Some(lastSlashIndex) {
    try! path[lastSlashIndex + 1:].to_string()
    // Fallback to windows path separator
  } else if path.rev_find("\\") is Some(lastBackslashIndex) {
    return try! path[lastBackslashIndex + 1:].to_string()
  } else {
    return path
  }
}
