///|
/// Represents a discovered package within a MoonBit project.
pub(all) struct PackageInfo {
  rel_path : String
  dir : String
} derive(Show)

///|
/// Holds the scan results for a single package: its info and source files.
pub(all) struct PackageScanResult {
  pkg : PackageInfo
  files : Array[(String, String)]
} derive(Show)

///|
/// Directories to skip during recursive package discovery.
let skip_dirs : Array[String] = [
  "_build", ".mooncakes", "golem-temp", ".git", ".githooks",
]

///|
/// Generated files to exclude when scanning package source files.
let generated_files : Array[String] = [
  "golem_agents.mbt",
  "golem_reexports.mbt",
  "golem_derive.mbt",
  "golem_clients.mbt",
]

///|
/// Recursively discovers all packages (directories containing `moon.pkg`) in a
/// MoonBit project, starting from `project_root`.
pub fn discover_packages(project_root : String) -> Array[PackageInfo] raise {
  let root = if project_root.has_suffix("/") {
    project_root[:(project_root.length() - 1)].to_string()
  } else {
    project_root
  }
  let packages : Array[PackageInfo] = []
  let visited : Map[String, Bool] = {}
  walk_dir(root, root, packages, visited)
  packages
}

///|
/// Recursively walks directories to find packages.
/// Tracks visited directories to prevent infinite loops from symlink cycles.
fn walk_dir(
  root : String,
  dir : String,
  packages : Array[PackageInfo],
  visited : Map[String, Bool],
) -> Unit raise {
  if visited.get(dir) is Some(_) {
    return
  }
  visited[dir] = true
  if @fs.path_exists("\{dir}/moon.pkg") {
    let rel_path = if dir == root {
      ""
    } else {
      dir[(root.length() + 1):].to_string()
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
      walk_dir(root, full_path, packages, visited)
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
  project_root : String
) -> Array[PackageScanResult] raise {
  let packages = discover_packages(project_root)
  let results : Array[PackageScanResult] = []
  for pkg in packages {
    results.push(scan_package(pkg))
  }
  results
}
