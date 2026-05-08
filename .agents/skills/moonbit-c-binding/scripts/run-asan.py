#!/usr/bin/env python3
"""Run MoonBit native tests with AddressSanitizer.

Snapshots each specified package file (`moon.pkg` DSL or `moon.pkg.json`),
patches `link.native` with ASan flags, disables mimalloc, runs `moon test`,
and restores everything in a finally block.

The script uses two mechanisms:
  - Package config patching: adds ASan flags to cc-flags and stub-cc-flags
    (preserving existing flags like -I, -D). stub-cc-flags is patched on all
    packages; cc-flags is patched only on entry packages (is-main or test).
  - mimalloc disable: replaces libmoonbitrun.o with a dummy empty object so
    ASan can intercept all memory allocations

On macOS, MOON_CC/MOON_AR env vars are set to use Homebrew LLVM (Apple Clang
lacks LeakSanitizer). On other platforms the system compiler is used directly.

Both `moon.pkg` (DSL format) and `moon.pkg.json` (JSON format) are supported.
"""

import argparse
import json
import os
import platform
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Any


ASAN_COMPILE_FLAGS = "-g -fsanitize=address -fno-omit-frame-pointer"


def _find_brew_clang() -> str | None:
    """Find Homebrew LLVM clang, which supports both ASan and LSan."""
    brew = shutil.which("brew")
    if not brew:
        if Path("/opt/homebrew/bin/brew").exists():
            brew = "/opt/homebrew/bin/brew"
        else:
            return None
    llvm_opts = ["llvm", "llvm@18", "llvm@19", "llvm@15", "llvm@13"]
    for llvm in llvm_opts:
        try:
            llvm_prefix = subprocess.run(
                [brew, "--prefix", llvm], check=True, text=True, capture_output=True
            ).stdout.strip()
        except subprocess.CalledProcessError:
            continue
        clang_path = Path(llvm_prefix) / "bin" / "clang"
        if clang_path.exists():
            return str(clang_path)
    return None


def macos_flags() -> tuple[str, dict[str, str]]:
    """Try Homebrew LLVM first (supports LSan), fall back to system clang."""
    # Prefer Homebrew LLVM: supports both ASan and LSan (leak detection)
    brew_clang = _find_brew_clang()
    if brew_clang:
        return (brew_clang, {"cc-flags": ASAN_COMPILE_FLAGS, "detect_leaks": "1"})

    # Fall back to system clang (Xcode 15+ supports ASan but NOT LSan)
    system_cc = shutil.which("cc") or "/usr/bin/cc"
    result = subprocess.run(
        [system_cc, "-fsanitize=address", "-x", "c", "-", "-o", "/dev/null"],
        input="int main(){return 0;}",
        text=True,
        capture_output=True,
    )
    if result.returncode == 0:
        return (system_cc, {"cc-flags": ASAN_COMPILE_FLAGS, "detect_leaks": "0"})

    raise Exception(
        "No ASan-capable compiler found. Install Homebrew LLVM: brew install llvm"
    )


def linux_flags() -> tuple[str, dict[str, str]]:
    cc = shutil.which("cc") or "gcc"
    return (cc, {"cc-flags": ASAN_COMPILE_FLAGS, "detect_leaks": "1"})


def windows_flags() -> tuple[str, dict[str, str]]:
    return ("cl", {
        "cc-flags": "/Z7 /fsanitize=address",
        "stub-cc-flags": "/Z7 /fsanitize=address",
        "detect_leaks": "0",
    })


def get_flags() -> tuple[str, dict[str, str]]:
    """Return (cc_path, flags_dict). cc_path is used for mimalloc and macOS MOON_CC."""
    system = platform.system()
    if system == "Darwin":
        return macos_flags()
    elif system == "Linux":
        return linux_flags()
    elif system == "Windows":
        return windows_flags()
    raise Exception(f"Unsupported platform: {system}")


# ---------------------------------------------------------------------------
# mimalloc disable
# ---------------------------------------------------------------------------


def _find_libmoonbitrun() -> Path | None:
    """Find libmoonbitrun.o by deriving the path from the moon binary."""
    moon_bin = shutil.which("moon")
    if not moon_bin:
        return None
    lib_dir = Path(moon_bin).resolve().parent.parent / "lib"
    moonbitrun = lib_dir / "libmoonbitrun.o"
    if moonbitrun.exists():
        return moonbitrun
    # Fallback: $MOON_HOME/lib/ (default: ~/.moon/lib/)
    moon_home = os.environ.get("MOON_HOME", str(Path.home() / ".moon"))
    moonbitrun = Path(moon_home) / "lib" / "libmoonbitrun.o"
    if moonbitrun.exists():
        return moonbitrun
    return None


def disable_mimalloc(cc_path: str) -> tuple[Path, bytes] | None:
    """Replace libmoonbitrun.o with a dummy empty object to disable mimalloc.

    MoonBit bundles mimalloc as its allocator via libmoonbitrun.o. mimalloc
    intercepts malloc/free, which prevents ASan from tracking allocations.
    Replacing it with an empty object lets ASan's allocator take over.

    Returns (path, original_bytes) for restoration, or None if not found.
    """
    moonbitrun = _find_libmoonbitrun()
    if moonbitrun is None:
        print("Warning: libmoonbitrun.o not found, skipping mimalloc disable")
        return None

    original = moonbitrun.read_bytes()

    # Compile an empty C file as the replacement
    fd, dummy_c = tempfile.mkstemp(suffix=".c")
    os.close(fd)
    try:
        if platform.system() == "Windows":
            subprocess.run(
                ["cl.exe", dummy_c, "/c", f"/Fo:{moonbitrun}"],
                check=True,
                capture_output=True,
            )
        else:
            subprocess.run(
                [cc_path, "-c", dummy_c, "-o", str(moonbitrun)],
                check=True,
                capture_output=True,
            )
    finally:
        os.unlink(dummy_c)

    print(f"Disabled mimalloc: {moonbitrun}")
    return (moonbitrun, original)


# ---------------------------------------------------------------------------
# Utilities
# ---------------------------------------------------------------------------


def display_path(path: Path, repo_root: Path) -> str:
    try:
        return str(path.relative_to(repo_root))
    except ValueError:
        return str(path)


def resolve_pkg_path(repo_root: Path, pkg_arg: str) -> Path:
    requested = Path(pkg_arg)
    requested = requested if requested.is_absolute() else (repo_root / requested)

    # Build candidate list: try both formats regardless of which was specified
    candidates = []
    if requested.name == "moon.pkg":
        candidates.append(requested.with_name("moon.pkg.json"))
        candidates.append(requested)
    elif requested.name == "moon.pkg.json":
        candidates.append(requested)
        candidates.append(requested.with_name("moon.pkg"))
    else:
        candidates.append(requested)

    for candidate in candidates:
        if candidate.is_file():
            return candidate.resolve()

    searched = ", ".join(str(p) for p in candidates)
    sys.exit(f"Package file not found for --pkg {pkg_arg}. Tried: {searched}")


# ---------------------------------------------------------------------------
# JSON format patching (moon.pkg.json)
# ---------------------------------------------------------------------------


def patch_link_native_json(
    moon_pkg: dict[str, Any], flags: dict[str, str], pkg_path: Path,
    is_entry: bool,
) -> None:
    """Patch link.native in a parsed JSON dict with ASan flags.

    Always patches stub-cc-flags. Only patches cc-flags when is_entry is True.
    """
    link = moon_pkg.get("link")
    if link is None:
        link = {}
        moon_pkg["link"] = link
    elif not isinstance(link, dict):
        raise ValueError(f'Expected "link" object in {pkg_path}')

    native = link.get("native")
    if native is None:
        native = {}
    elif not isinstance(native, dict):
        raise ValueError(f'Expected "link.native" object in {pkg_path}')

    # cc-flags: set ASan compile flags for MoonBit-generated C (entry packages only)
    if is_entry and "cc-flags" in flags:
        native["cc-flags"] = flags["cc-flags"]

    # stub-cc-flags: append ASan flags to existing value (preserving -I, -D, etc.)
    existing_stub_flags = native.get("stub-cc-flags", "")
    if "stub-cc-flags" in flags:
        # Windows: override entirely
        native["stub-cc-flags"] = flags["stub-cc-flags"]
    elif existing_stub_flags:
        native["stub-cc-flags"] = existing_stub_flags + " " + ASAN_COMPILE_FLAGS
    else:
        native["stub-cc-flags"] = ASAN_COMPILE_FLAGS

    link["native"] = native


def patch_json_file(pkg_path: Path, flags: dict[str, str], is_entry: bool) -> str:
    """Patch a moon.pkg.json file. Returns the patched text."""
    text = pkg_path.read_text(encoding="utf-8")
    try:
        moon_pkg = json.loads(text)
    except json.JSONDecodeError as error:
        sys.exit(f"Failed to parse JSON in {pkg_path}: {error}")
    if not isinstance(moon_pkg, dict):
        sys.exit(f"Package file is not a JSON object: {pkg_path}")
    try:
        patch_link_native_json(moon_pkg, flags, pkg_path, is_entry)
    except ValueError as error:
        sys.exit(str(error))
    return json.dumps(moon_pkg, indent=2) + "\n"


# ---------------------------------------------------------------------------
# DSL format patching (moon.pkg)
# ---------------------------------------------------------------------------


def _find_object_block(
    text: str,
    key: str,
    search_start: int = 0,
    search_end: int | None = None,
) -> tuple[int, int] | None:
    """Find the start and end positions of a key: { ... } object block."""
    key_pattern = rf'(?:{re.escape(key)}|"{re.escape(key)}")'
    haystack = text[search_start:search_end]
    m = re.search(rf"{key_pattern}\s*:\s*\{{", haystack)
    if m is None:
        return None
    start = search_start + m.end()
    depth = 1
    pos = start
    limit = len(text) if search_end is None else search_end
    while pos < limit and depth > 0:
        if text[pos] == "{":
            depth += 1
        elif text[pos] == "}":
            depth -= 1
        pos += 1
    if depth != 0:
        return None
    return (start, pos)


def _find_root_block(text: str) -> tuple[int, int] | None:
    """Find the outermost { ... } block in the file."""
    start = text.find("{")
    if start < 0:
        return None
    depth = 1
    pos = start + 1
    while pos < len(text) and depth > 0:
        if text[pos] == "{":
            depth += 1
        elif text[pos] == "}":
            depth -= 1
        pos += 1
    if depth != 0:
        return None
    return (start + 1, pos)


def _find_options_block(text: str) -> tuple[int, int] | None:
    """Find the start and end positions of options( ... ) content."""
    m = re.search(r"\boptions\s*\(", text)
    if m is None:
        return None
    start = m.end()
    depth = 1
    pos = start
    while pos < len(text) and depth > 0:
        if text[pos] == "(":
            depth += 1
        elif text[pos] == ")":
            depth -= 1
        pos += 1
    if depth != 0:
        return None
    return (start, pos)


def _find_link_block(
    text: str,
    search_start: int = 0,
    search_end: int | None = None,
) -> tuple[int, int] | None:
    """Find the start and end positions of the link: { ... } block."""
    return _find_object_block(text, "link", search_start, search_end)


def _find_native_block(text: str) -> tuple[int, int] | None:
    """Find the start and end positions of the native: { ... } block."""
    return _find_object_block(text, "native")


def _closing_indent(text: str, closing_brace: int) -> str:
    """Return indentation before a closing brace on its current line."""
    line_start = text.rfind("\n", 0, closing_brace) + 1
    indent = text[line_start:closing_brace]
    return indent if indent.isspace() else ""


def _detect_entry_indent(text: str, content_start: int, content_end: int) -> str:
    """Detect indentation for key entries in a container."""
    m = re.search(r"\n(\s+)\S", text[content_start : content_end - 1])
    return m.group(1) if m else "      "


def _insert_entry_in_container(
    text: str,
    content_start: int,
    container_end: int,
    entry: str,
    entry_indent: str,
) -> str:
    """Insert an entry into a comma-separated container preserving syntax."""
    closing_brace = container_end - 1
    block_content = text[content_start:closing_brace]
    has_entries = bool(block_content.strip())
    multiline = "\n" in block_content

    if not has_entries:
        if multiline:
            insertion = f"\n{entry_indent}{entry},\n{_closing_indent(text, closing_brace)}"
        else:
            insertion = f" {entry} "
        return text[:content_start] + insertion + text[closing_brace:]

    last = closing_brace - 1
    while last >= content_start and text[last].isspace():
        last -= 1
    if last < content_start:
        return text

    needs_comma = text[last] != ","
    if multiline:
        separator = "," if needs_comma else ""
        insertion = (
            f"{separator}\n{entry_indent}{entry},\n{_closing_indent(text, closing_brace)}"
        )
    else:
        tail_ws = text[last + 1 : closing_brace]
        separator = ", " if needs_comma else " "
        insertion = f"{separator}{entry}{tail_ws}"
    return text[: last + 1] + insertion + text[closing_brace:]


def _insert_entry_in_block(
    text: str,
    content_start: int,
    block_end: int,
    entry: str,
    entry_indent: str,
) -> str:
    """Insert an entry into a { ... } block while preserving valid syntax."""
    return _insert_entry_in_container(text, content_start, block_end, entry, entry_indent)


def _ensure_native_block(text: str) -> str:
    """Ensure link.native exists by creating missing blocks when needed."""
    if _find_native_block(text) is not None:
        return text

    options_bounds = _find_options_block(text)
    if options_bounds is not None:
        options_start, options_end = options_bounds
        link_bounds = _find_link_block(text, options_start, options_end)
        if link_bounds is not None:
            content_start, block_end = link_bounds
            entry_indent = _detect_entry_indent(text, content_start, block_end)
            return _insert_entry_in_block(
                text, content_start, block_end, '"native": {}', entry_indent
            )

        entry_indent = _detect_entry_indent(text, options_start, options_end)
        return _insert_entry_in_container(
            text,
            options_start,
            options_end,
            'link: { "native": {} }',
            entry_indent,
        )

    link_bounds = _find_link_block(text)
    if link_bounds is not None:
        content_start, block_end = link_bounds
        entry_indent = _detect_entry_indent(text, content_start, block_end)
        return _insert_entry_in_block(
            text, content_start, block_end, '"native": {}', entry_indent
        )

    root_bounds = _find_root_block(text)
    if root_bounds is None:
        raise ValueError("Could not locate root object block in moon.pkg")
    content_start, block_end = root_bounds
    m = re.search(r"\n(\s+)\S", text[content_start : block_end - 1])
    entry_indent = m.group(1) if m else "  "
    return _insert_entry_in_block(
        text,
        content_start,
        block_end,
        '"link": { "native": {} }',
        entry_indent,
    )


def _find_string_value_in_native(text: str, key: str) -> str | None:
    """Find a string value for key inside native block."""
    bounds = _find_native_block(text)
    if bounds is None:
        return None
    content_start, block_end = bounds
    native_text = text[content_start : block_end - 1]
    pattern = re.compile(rf'(?:{re.escape(key)}|"{re.escape(key)}")\s*:\s*"([^"]*)"')
    m = pattern.search(native_text)
    return m.group(1) if m else None


def _replace_or_insert_in_native(text: str, key: str, value: str) -> str:
    """Replace an existing key's value or insert a new key-value pair in native."""
    bounds = _find_native_block(text)
    if bounds is None:
        raise ValueError('No "native" block found in moon.pkg')
    content_start, block_end = bounds
    native_text = text[content_start : block_end - 1]

    # Replace existing key only inside native block.
    pattern = re.compile(rf'((?:{re.escape(key)}|"{re.escape(key)}")\s*:\s*)"([^"]*)"')
    if pattern.search(native_text):
        replaced = pattern.sub(rf'\g<1>"{value}"', native_text, count=1)
        return text[:content_start] + replaced + text[block_end - 1 :]

    entry_indent = _detect_entry_indent(text, content_start, block_end)
    return _insert_entry_in_block(
        text,
        content_start,
        block_end,
        f'"{key}": "{value}"',
        entry_indent,
    )


def patch_dsl_file(pkg_path: Path, flags: dict[str, str], is_entry: bool) -> str:
    """Patch a moon.pkg DSL file using text manipulation. Returns the patched text.

    Always patches stub-cc-flags. Only patches cc-flags when is_entry is True.
    """
    text = pkg_path.read_text(encoding="utf-8")
    try:
        text = _ensure_native_block(text)
    except ValueError as error:
        sys.exit(f"Failed to patch {pkg_path}: {error}")

    # 1. cc-flags: set ASan compile flags for MoonBit-generated C (entry packages only)
    if is_entry and "cc-flags" in flags:
        text = _replace_or_insert_in_native(text, "cc-flags", flags["cc-flags"])

    # 2. stub-cc-flags: append ASan flags (or override on Windows)
    if "stub-cc-flags" in flags:
        text = _replace_or_insert_in_native(
            text, "stub-cc-flags", flags["stub-cc-flags"]
        )
    else:
        existing = _find_string_value_in_native(text, "stub-cc-flags")
        if existing is not None:
            new_value = f"{existing} {ASAN_COMPILE_FLAGS}"
            text = _replace_or_insert_in_native(text, "stub-cc-flags", new_value)
        else:
            text = _replace_or_insert_in_native(
                text, "stub-cc-flags", ASAN_COMPILE_FLAGS
            )

    return text


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------


def is_dsl_format(pkg_path: Path) -> bool:
    """Check if a package file uses moon.pkg DSL format (vs moon.pkg.json)."""
    return pkg_path.name == "moon.pkg"


def _is_entry_package(pkg_path: Path) -> bool:
    """Check if package is an entry package (is-main or has test files)."""
    text = pkg_path.read_text(encoding="utf-8")
    # Check is-main in config
    if is_dsl_format(pkg_path):
        if re.search(
            r'(?:is-main|is_main|"is-main"|"is_main")\s*:\s*true\b',
            text,
        ):
            return True
    else:
        data = json.loads(text)
        if data.get("is-main") or data.get("is_main"):
            return True
    # Heuristic: check for *_test.mbt files in the same directory.
    # Note: this won't detect test blocks inside non-_test.mbt files.
    pkg_dir = pkg_path.parent
    if list(pkg_dir.glob("*_test.mbt")):
        return True
    return False


def main():
    parser = argparse.ArgumentParser(
        description="Run MoonBit native tests with AddressSanitizer"
    )
    parser.add_argument(
        "--repo-root", required=True, type=Path, help="Project repository root"
    )
    parser.add_argument(
        "--pkg",
        action="append",
        default=[],
        metavar="PKG_FILE",
        help=(
            "Relative path to moon.pkg or moon.pkg.json (repeatable). "
            "Either format is auto-detected. "
            "Must include ALL packages with native-stub and all entry packages (is-main/test)."
        ),
    )
    parser.add_argument(
        "--no-disable-mimalloc",
        action="store_true",
        help="Skip disabling mimalloc (not recommended).",
    )
    args = parser.parse_args()

    repo_root = args.repo_root.resolve()
    if not repo_root.is_dir():
        sys.exit(f"--repo-root is not a directory: {repo_root}")
    if not (repo_root / "moon.mod.json").is_file():
        print(
            f"Warning: no moon.mod.json found in {repo_root}. "
            "Is --repo-root pointing at the right directory?",
            file=sys.stderr,
        )

    pkg_paths: list[Path] = []
    seen_pkg_paths: set[Path] = set()
    for pkg_arg in args.pkg:
        pkg_path = resolve_pkg_path(repo_root, pkg_arg)
        if pkg_path in seen_pkg_paths:
            continue
        seen_pkg_paths.add(pkg_path)
        pkg_paths.append(pkg_path)
        resolved_name = pkg_path.name
        requested_name = Path(pkg_arg).name
        if requested_name != resolved_name:
            print(
                f"Resolved --pkg {pkg_arg} -> {display_path(pkg_path, repo_root)}"
            )

    if not pkg_paths:
        sys.exit(
            "No --pkg arguments provided. Specify at least one moon.pkg or moon.pkg.json."
        )

    cc_path, flags = get_flags()
    detect_leaks = flags.pop("detect_leaks", "1")
    print(f"Platform: {platform.system()}")
    print(f"ASan compiler: {cc_path}")
    print(f"ASan compile flags: {flags['cc-flags']}")
    print(f"Leak detection: {'enabled' if detect_leaks == '1' else 'disabled'}")

    # Snapshot originals
    snapshots: dict[Path, str] = {}
    for pkg_path in pkg_paths:
        snapshots[pkg_path] = pkg_path.read_text(encoding="utf-8")

    # Disable mimalloc by replacing libmoonbitrun.o with an empty object.
    # MoonBit bundles mimalloc which intercepts malloc/free and prevents
    # ASan from tracking allocations properly.
    mimalloc_backup: tuple[Path, bytes] | None = None
    if not args.no_disable_mimalloc:
        mimalloc_backup = disable_mimalloc(cc_path)

    # Build environment
    env = os.environ.copy()
    # MOON_CC/MOON_AR only needed on macOS (Apple Clang lacks LSan)
    if platform.system() == "Darwin":
        env["MOON_CC"] = cc_path
        env["MOON_AR"] = "/usr/bin/ar"
    asan_opts = f"detect_leaks={detect_leaks}:fast_unwind_on_malloc=0"
    env["ASAN_OPTIONS"] = asan_opts
    lsan_suppressions = repo_root / ".lsan-suppressions"
    if lsan_suppressions.exists():
        env["LSAN_OPTIONS"] = f"suppressions={lsan_suppressions}"

    try:
        for pkg_path in pkg_paths:
            is_entry = _is_entry_package(pkg_path)
            if is_dsl_format(pkg_path):
                patched = patch_dsl_file(pkg_path, flags, is_entry)
            else:
                patched = patch_json_file(pkg_path, flags, is_entry)
            pkg_path.write_text(patched, encoding="utf-8")
            fmt = "DSL" if is_dsl_format(pkg_path) else "JSON"
            kind = "entry" if is_entry else "library"
            print(f"Patched ({fmt}, {kind}): {display_path(pkg_path, repo_root)}")

        result = subprocess.run(
            ["moon", "test", "--target", "native", "-v"],
            cwd=repo_root,
            env=env,
        )
        sys.exit(result.returncode)
    finally:
        for pkg_path, original in snapshots.items():
            pkg_path.write_text(original, encoding="utf-8")
            print(f"Restored: {display_path(pkg_path, repo_root)}")
        if mimalloc_backup is not None:
            moonbitrun_path, original_bytes = mimalloc_backup
            moonbitrun_path.write_bytes(original_bytes)
            print(f"Restored mimalloc: {moonbitrun_path}")


if __name__ == "__main__":
    main()
