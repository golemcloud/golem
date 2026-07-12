///|
/// Represents a discovered package within a MoonBit project.
pub(all) struct PackageInfo {
  rel_path : String
  dir : String
} derive(Debug)

///|
/// Holds the scan results for a single package: its info and source files.
pub(all) struct PackageScanResult {
  pkg : PackageInfo
  files : Array[(String, String)]
} derive(Debug)

///|
/// Directories to skip during recursive package discovery.
let skip_dirs : Array[String] = [
  "_build", ".mooncakes", "golem-temp", ".git", ".githooks",
]

///|
/// Generated files to exclude when scanning package source files.
let generated_files : Array[String] = [
  "golem_agents.mbt", "golem_reexports.mbt", "golem_derive.mbt", "golem_clients.mbt",
  "golem_tools.mbt", "golem_tool_clients.mbt",
]

///|
/// Recursively discovers all packages (directories containing `moon.pkg`) in a
/// MoonBit project, starting from `project_root`.
///
/// Package `rel_path`s are computed relative to the **MoonBit module root**
/// (the nearest ancestor — including `project_root` itself — that contains a
/// `moon.mod.json`), NOT relative to the CLI argument. This keeps a package's
/// identity (and therefore the default `type_id()` prefix derived from it)
/// stable whether the tool is invoked at the module root (`agents <module>`)
/// or at a single package dir (`agents <module>/pkg`).
pub fn discover_packages(project_root : String) -> Array[PackageInfo] raise {
  let root = if project_root.has_suffix("/") {
    project_root[:project_root.length() - 1].to_string()
  } else {
    project_root
  }
  let rel_base = find_module_root(root)
  let packages : Array[PackageInfo] = []
  let visited : Map[String, Bool] = {}
  walk_dir(rel_base, root, packages, visited)
  packages
}

///|
/// Returns the parent directory of `dir`, or `dir` itself when it has no parent
/// (a single segment or the filesystem root).
fn parent_dir(dir : String) -> String {
  match dir.rev_find("/") {
    Some(0) => "/"
    Some(idx) => dir.unsafe_substring(start=0, end=idx)
    None => dir
  }
}

///|
/// Finds the MoonBit module root for `start`: the nearest ancestor directory
/// (including `start`) that contains a `moon.mod.json`. Falls back to `start`
/// when no module file is found up to the filesystem root.
fn find_module_root(start : String) -> String raise {
  let mut dir = start
  for ;; {
    if @fs.path_exists("\{dir}/moon.mod.json") {
      return dir
    }
    let parent = parent_dir(dir)
    if parent == dir {
      return start
    }
    dir = parent
  }
}

///|
/// Recursively walks directories to find packages.
/// `rel_base` is the directory that package `rel_path`s are computed relative to
/// (the MoonBit module root); `dir` is the directory currently being walked.
/// Tracks visited directories to prevent infinite loops from symlink cycles.
fn walk_dir(
  rel_base : String,
  dir : String,
  packages : Array[PackageInfo],
  visited : Map[String, Bool],
) -> Unit raise {
  if visited.get(dir) is Some(_) {
    return
  }
  visited[dir] = true
  if @fs.path_exists("\{dir}/moon.pkg") {
    let rel_path = if dir == rel_base {
      ""
    } else {
      dir[rel_base.length() + 1:].to_string()
    }
    packages.push(PackageInfo::{ rel_path, dir })
  }
  let entries = @fs.read_dir(dir)
  for entry in entries {
    if skip_dirs.contains(entry) {
      continue
    }
    let full_path = "\{dir}/\{entry}"
    if @fs.is_dir(full_path) {
      walk_dir(rel_base, full_path, packages, visited)
    }
  }
}

///|
/// Reads all non-generated `.mbt` source files in the given package directory.
pub fn scan_package(pkg : PackageInfo) -> PackageScanResult raise {
  let files : Array[(String, String)] = []
  let entries = @fs.read_dir(pkg.dir)
  for entry in entries {
    if entry.has_suffix(".mbt") && not(generated_files.contains(entry)) {
      let path = "\{pkg.dir}/\{entry}"
      let content = @fs.read_file_to_string(path)
      files.push((entry, content))
    }
  }
  PackageScanResult::{ pkg, files }
}

///|
/// Discovers all packages in a project and scans each one for source files.
pub fn discover_and_scan(
  project_root : String,
) -> Array[PackageScanResult] raise {
  let packages = discover_packages(project_root)
  let results : Array[PackageScanResult] = []
  for pkg in packages {
    results.push(scan_package(pkg))
  }
  results
}
