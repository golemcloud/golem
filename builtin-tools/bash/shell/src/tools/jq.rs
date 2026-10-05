//! `jq`: jq 1.8's command line over the jaq engine.
//!
//! [`parse`] and [`Program`] serve both drivers; a driver only decides how input bytes arrive.
//! jaq's own CLI (MIT) is the reference for flag handling, and jq 1.8.2 for output bytes and exit
//! statuses. Nothing here may exit the process: jaq's `halt` becomes an ordinary exit status.

use super::io_message;
use jaq_core::jq_utf8;
use jaq_core::load::{self, Arena, File, Loader};
use jaq_core::native::{self, v};
use jaq_core::{Ctx, DataT, Error, Lut, RunPtr, Vars, compile::Compiler, data::HasLut};
use jaq_json::jv_parse::{self, Next, Parser};
use jaq_json::{Val, write};
use std::cell::RefCell;
use std::fmt::Write as _;
use std::path::PathBuf;

const HELP: &str = "Usage:\tjq [OPTIONS] FILTER [FILES...]
\tjq [OPTIONS] --args FILTER [STRINGS...]
\tjq [OPTIONS] --jsonargs FILTER [JSON_VALUES...]

jq is a JSON processor (the jaq engine behind jq's command line).

Options:
  -n, --null-input          use `null` as the single input value;
  -R, --raw-input           read each line as string instead of JSON;
  -s, --slurp               read all inputs into an array and use it as
                            the single input value;
  -c, --compact-output      compact instead of pretty-printed output;
  -r, --raw-output          output strings without escapes and quotes;
      --raw-output0         implies -r and output NUL after each output;
  -j, --join-output         implies -r and output without newline after
                            each output;
  -a, --ascii-output        output strings by only ASCII characters
                            using escape sequences;
  -S, --sort-keys           sort keys of each object on output;
  -C, --color-output        colorize JSON output;
  -M, --monochrome-output   disable colored output;
      --tab                 use tabs for indentation;
      --indent n            use n spaces for indentation (max 7 spaces);
      --seq                 parse and output as application/json-seq;
  -f, --from-file           load the filter from the file named by the
                            first positional argument;
      --arg name value      set $name to the string value;
      --argjson name value  set $name to the JSON value;
      --slurpfile name file set $name to an array of JSON values read
                            from the file;
      --rawfile name file   set $name to string contents of file;
      --args                consume remaining arguments as positional
                            string values;
      --jsonargs            consume remaining arguments as positional
                            JSON values;
  -e, --exit-status         set exit status code based on the output;
  -V, --version             show the version;
  -h, --help                show the help;
  --                        terminates argument processing;

Not supported in bash-tool: --stream, --stream-errors, modules (import/include).
";

const UNKNOWN_HINT: &str = "Use jq --help for help with command-line options,\nor see the jq manpage, or online docs at https://jqlang.org\n";

/// A diagnostic already formatted for stderr, with the exit status jq uses for it.
#[derive(Debug)]
pub(crate) struct Failure {
    pub(crate) code: i32,
    pub(crate) message: String,
}

fn failure(code: i32, message: impl Into<String>) -> Failure {
    Failure {
        code,
        message: message.into(),
    }
}

enum FilterSource {
    Inline(String),
    File(String),
}

enum Named {
    Str(String),
    Json(String),
    SlurpFile(String),
    RawFile(String),
}

#[derive(Default)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "one switch per jq command-line option"
)]
pub(crate) struct Options {
    null_input: bool,
    raw_input: bool,
    slurp: bool,
    raw_output: bool,
    join_output: bool,
    nul_output: bool,
    ascii_output: bool,
    /// `None` is compact output; otherwise the indentation unit.
    indent: Option<String>,
    sort_keys: bool,
    /// `-C`: color the output (standard output is never a terminal here).
    color: bool,
    /// `-M`: never color it, which wins over `-C`, as in jq.
    monochrome: bool,
    seq: bool,
    exit_status: bool,
    from_file: bool,
    filter: Option<FilterSource>,
    /// Input file operands; empty means standard input.
    pub(crate) files: Vec<String>,
    named: Vec<(String, Named)>,
    /// Positional arguments after `--args` (`false`) or `--jsonargs` (`true`).
    positional: Vec<(String, bool)>,
    /// `-L` directories: jq's module search path, which `get_search_list` reports.
    library_paths: Vec<String>,
    /// The command name as invoked (`argv[0]`), whose directory `get_jq_origin` reports.
    command: String,
}

/// What the command line asks for.
pub(crate) enum Parsed {
    Run(Box<Options>),
    Help,
    Version,
}

#[derive(Clone, Copy)]
enum Mode {
    Files,
    Args,
    JsonArgs,
}

/// Parse jq's command line (`argv[0]` is the command name).
pub(crate) fn parse(argv: &[String]) -> Result<Parsed, Failure> {
    let mut options = Options {
        indent: Some("  ".into()),
        command: argv.first().cloned().unwrap_or_default(),
        ..Options::default()
    };
    let mut mode = Mode::Files;
    let mut done = false;
    let mut args = argv.iter().skip(1);
    let unknown = |text: &str| failure(2, format!("jq: Unknown option {text}\n{UNKNOWN_HINT}"));
    while let Some(arg) = args.next() {
        // jq's `isoptish`: `-` then a letter, or `--`; so `-5` and `-.5` are arguments.
        let optish = arg.strip_prefix('-').is_some_and(|rest| {
            rest.starts_with('-') || rest.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
        });
        if done || !optish {
            positional(&mut options, mode, arg);
            continue;
        }
        let Some(long) = arg.strip_prefix("--") else {
            for (at, flag) in arg.char_indices().skip(1) {
                match short(&mut options, flag) {
                    Some(Flag::Done) => (),
                    Some(Flag::Help) => return Ok(Parsed::Help),
                    Some(Flag::Version) => return Ok(Parsed::Version),
                    // `-Lpath` or `-L path`
                    None if flag == 'L' => {
                        let rest = &arg[at + 1..];
                        let path = if rest.is_empty() {
                            args.next().cloned().ok_or_else(|| failure(2, "jq: -L takes a parameter: (e.g. -L /search/path or -L/search/path)\n"))?
                        } else {
                            rest.to_owned()
                        };
                        options.library_paths.push(path);
                        break;
                    }
                    // jq names the first letter it does not know.
                    None => return Err(unknown(&format!("-{flag}"))),
                }
            }
            continue;
        };
        let mut pair = |option: &str| -> Result<(String, String), Failure> {
            match (args.next(), args.next()) {
                (Some(name), Some(value)) => Ok((name.clone(), value.clone())),
                _ => Err(failure(
                    2,
                    format!(
                        "jq: --{option} takes two parameters (e.g. --{option} varname value)\n"
                    ),
                )),
            }
        };
        match long {
            "" => done = true,
            "arg" => {
                let (name, value) = pair("arg")?;
                options.named.push((name, Named::Str(value)));
            }
            "argjson" => {
                let (name, value) = pair("argjson")?;
                options.named.push((name, Named::Json(value)));
            }
            "slurpfile" => {
                let (name, value) = pair("slurpfile")?;
                options.named.push((name, Named::SlurpFile(value)));
            }
            "rawfile" => {
                let (name, value) = pair("rawfile")?;
                options.named.push((name, Named::RawFile(value)));
            }
            "args" => mode = Mode::Args,
            "jsonargs" => mode = Mode::JsonArgs,
            "indent" => {
                let width = args
                    .next()
                    .and_then(|value| value.parse::<i64>().ok())
                    .ok_or_else(|| failure(2, "jq: --indent takes one parameter\n"))?;
                options.indent = match width {
                    -1 => Some("\t".into()),
                    0..=7 => Some(" ".repeat(usize::try_from(width).unwrap_or(0))),
                    _ => {
                        return Err(failure(
                            2,
                            format!("jq: --indent takes a number between -1 and 7\n{UNKNOWN_HINT}"),
                        ));
                    }
                };
            }
            "tab" => options.indent = Some("\t".into()),
            "seq" => options.seq = true,
            "raw-output0" => {
                options.raw_output = true;
                options.nul_output = true;
            }
            "library-path" => {
                let path = args
                    .next()
                    .ok_or_else(|| failure(2, "jq: --library-path takes a parameter\n"))?;
                options.library_paths.push(path.clone());
            }
            "unbuffered" | "binary" => (),
            "stream" | "stream-errors" => {
                return Err(failure(
                    2,
                    format!("jq: --{long} is unsupported in bash-tool\n"),
                ));
            }
            other => {
                let flag = match other {
                    "null-input" => 'n',
                    "raw-input" => 'R',
                    "slurp" => 's',
                    "compact-output" => 'c',
                    "raw-output" => 'r',
                    "join-output" => 'j',
                    "ascii-output" => 'a',
                    "sort-keys" => 'S',
                    "color-output" => 'C',
                    "monochrome-output" => 'M',
                    "from-file" => 'f',
                    "exit-status" => 'e',
                    "version" => 'V',
                    "help" => 'h',
                    _ => return Err(unknown(arg)),
                };
                match short(&mut options, flag) {
                    Some(Flag::Help) => return Ok(Parsed::Help),
                    Some(Flag::Version) => return Ok(Parsed::Version),
                    _ => (),
                }
            }
        }
    }
    Ok(Parsed::Run(Box::new(options)))
}

enum Flag {
    Done,
    Help,
    Version,
}

fn short(options: &mut Options, flag: char) -> Option<Flag> {
    match flag {
        'n' => options.null_input = true,
        'R' => options.raw_input = true,
        's' => options.slurp = true,
        'c' => options.indent = None,
        'r' => options.raw_output = true,
        'j' => {
            options.raw_output = true;
            options.join_output = true;
        }
        'a' => options.ascii_output = true,
        'S' => options.sort_keys = true,
        'C' => options.color = true,
        'M' => options.monochrome = true,
        'b' => (),
        'f' => options.from_file = true,
        'e' => options.exit_status = true,
        'h' => return Some(Flag::Help),
        'V' => return Some(Flag::Version),
        _ => return None,
    }
    Some(Flag::Done)
}

fn positional(options: &mut Options, mode: Mode, arg: &str) {
    if options.filter.is_none() {
        options.filter = Some(if options.from_file {
            FilterSource::File(arg.to_owned())
        } else {
            FilterSource::Inline(arg.to_owned())
        });
        return;
    }
    match mode {
        Mode::Files => options.files.push(arg.to_owned()),
        Mode::Args => options.positional.push((arg.to_owned(), false)),
        Mode::JsonArgs => options.positional.push((arg.to_owned(), true)),
    }
}

pub(crate) const fn help() -> &'static str {
    HELP
}

pub(crate) const fn version() -> &'static str {
    "jq-1.8.2 (jaq 3.1)\n"
}

/// jaq data for one run: the filter's look-up table plus the input reader that the main loop,
/// `input`, `input_line_number` and `input_filename` share.
pub(crate) struct Kind;

impl DataT for Kind {
    type V<'a> = Val;
    type Data<'a> = &'a Data<'a>;
}

pub(crate) struct Data<'a> {
    lut: &'a Lut<Kind>,
    input: &'a RefCell<Input>,
}

impl<'a> HasLut<'a, Kind> for &'a Data<'a> {
    fn lut(&self) -> &'a Lut<Kind> {
        self.lut
    }
}

thread_local! {
    /// Bytes that `debug`, `stderr` and `halt_error` write; the driver moves them to the
    /// command's stderr. jaq routes them to the `log` crate, which would drop them.
    static DIAGNOSTICS: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

fn compact(value: &Val) -> Vec<u8> {
    let mut bytes = Vec::new();
    let _ = write::write(&mut bytes, &write::Pp::default(), 0, value);
    bytes
}

/// Replace each byte that isn't part of a valid UTF-8 sequence with the Unicode replacement
/// character (U+FFFD), matching jq's own JSON reader (which substitutes one the moment it
/// decodes an invalid byte in a string literal, so it never reaches jq's output un-substituted)
/// rather than jaq's own choice to always preserve a value's exact bytes.
fn utf8_replace_invalid(bytes: &[u8]) -> std::borrow::Cow<'_, [u8]> {
    match String::from_utf8_lossy(bytes) {
        std::borrow::Cow::Borrowed(_) => std::borrow::Cow::Borrowed(bytes),
        std::borrow::Cow::Owned(s) => std::borrow::Cow::Owned(s.into_bytes()),
    }
}

fn string_bytes(value: &Val) -> Option<&[u8]> {
    match value {
        Val::TStr(bytes) | Val::BStr(bytes) => Some(bytes),
        _ => None,
    }
}

fn diagnostic(bytes: &[u8]) {
    DIAGNOSTICS.with(|buffer| buffer.borrow_mut().extend_from_slice(bytes));
}

fn diagnostics_funs() -> Box<[native::Filter<RunPtr<Kind>>]> {
    Box::new([
        ("debug_empty", v(0), |cv| {
            let mut line = b"[\"DEBUG:\",".to_vec();
            line.extend(compact(&cv.1));
            line.extend_from_slice(b"]\n");
            diagnostic(&line);
            Box::new(core::iter::empty())
        }),
        ("stderr_empty", v(0), |cv| {
            match string_bytes(&cv.1) {
                Some(text) => diagnostic(text),
                None => diagnostic(&compact(&cv.1)),
            }
            Box::new(core::iter::empty())
        }),
        ("halt_error_empty", v(0), |cv| {
            match string_bytes(&cv.1) {
                Some(text) => diagnostic(text),
                None => {
                    let mut line = compact(&cv.1);
                    line.push(b'\n');
                    diagnostic(&line);
                }
            }
            Box::new(core::iter::empty())
        }),
    ])
}

/// Take what `debug`, `stderr` and `halt_error` wrote since the last call.
pub(crate) fn take_diagnostics() -> Vec<u8> {
    DIAGNOSTICS.with(|buffer| std::mem::take(&mut *buffer.borrow_mut()))
}

/// jq's status for an uncaught error, and for invalid input JSON (`JQ_ERROR_UNKNOWN`).
const ERROR_STATUS: i32 = 5;
/// `JQ_OK_NULL_KIND`: the last output was `false` or `null`.
const OK_NULL_KIND: i32 = -1;
/// `JQ_OK_NO_OUTPUT`: the filter produced nothing.
const OK_NO_OUTPUT: i32 = -4;

fn extra_defs() -> impl Iterator<Item = load::parse::Def<&'static str>> {
    use load::parse::{Def, Term};
    let var = |name: &'static str, var: &'static str| Def {
        name,
        args: Vec::new(),
        body: Term::Var(var),
    };
    // `jq_definitions_parse` guards this; a parse failure must not silently drop them.
    let jq = load::parse(JQ_DEFS, |parser| parser.defs()).unwrap_or_default();
    [
        var("env", "$ENV"),
        var("get_search_list", SEARCH_LIST),
        var("get_jq_origin", JQ_ORIGIN),
        var("get_prog_origin", PROG_ORIGIN),
    ]
    .into_iter()
    .chain(jq)
}

/// Why a program's `import` or `include` of `name` (a module, or with `extension` `json` data)
/// fails: jq's own error when jq would not find it on its search path, or else the refusal of a
/// module jq would load, since modules are unsupported here.
fn module_failure(
    options: &Options,
    home: Option<&str>,
    resolve: &dyn Fn(&str) -> PathBuf,
    name: &str,
    extension: &str,
) -> Failure {
    // `-L` replaces the default search path; its `~/.jq` needs a home directory, and there is
    // no jq binary whose `$ORIGIN` the rest name.
    let (directories, note): (Vec<PathBuf>, &str) = if options.library_paths.is_empty() {
        match home {
            Some(home) => (vec![PathBuf::from(home).join(".jq")], ""),
            None => (
                Vec::new(),
                " (Could not expand ~/.jq. (Could not find home directory.))",
            ),
        }
    } else {
        let paths = options
            .library_paths
            .iter()
            .map(|path| match (path.strip_prefix("~/"), home) {
                (Some(rest), Some(home)) => PathBuf::from(home).join(rest),
                _ => resolve(path),
            })
            .collect();
        (paths, "")
    };
    let base = name.rsplit('/').next().unwrap_or(name);
    let found = directories.iter().any(|directory| {
        directory.join(format!("{name}.{extension}")).is_file()
            || directory
                .join(name)
                .join(format!("{base}.{extension}"))
                .is_file()
    });
    if found {
        return failure(2, "jq: module imports are unsupported in bash-tool\n");
    }
    compile_failure(vec![format!(
        "jq: error: module not found: {name}{note}\n\n"
    )])
}

/// jq's output styles, with those `JQ_COLORS` sets (for `null`, `false`, `true`, numbers,
/// strings, arrays, objects and object keys, in order) over its defaults, as jq's
/// `jq_set_colors` takes them: `Err` for a style that is too long or not digits and `;`.
fn jq_styles(colors: Option<&str>) -> Result<write::Styles, ()> {
    const DEFAULTS: [&str; 8] = [
        "0;90", "0;39", "0;39", "0;39", "0;32", "1;39", "1;39", "1;34",
    ];
    let mut styles = DEFAULTS.map(|style| format!("\x1b[{style}m"));
    let mut rest = colors.unwrap_or_default();
    for style in &mut styles {
        if rest.is_empty() {
            break;
        }
        let (field, next) = rest.split_once(':').unwrap_or((rest, ""));
        if field.len() > 12 || !field.bytes().all(|b| b.is_ascii_digit() || b == b';') {
            return Err(());
        }
        *style = format!("\x1b[{field}m");
        rest = next;
    }
    let [null, r#false, r#true, num, str, arr, obj, key] = styles;
    Ok(write::Styles {
        bstr: str.clone(),
        null,
        r#false,
        r#true,
        num,
        str,
        arr,
        obj,
        key,
        reset: "\x1b[0m".to_owned(),
    })
}

/// The formats jq 1.8 has (as the reference jq is built: without `@base32` and `@base32d`).
const JQ_FORMATS: [&str; 10] = [
    "text", "json", "html", "uri", "urid", "csv", "tsv", "sh", "base64", "base64d",
];

/// Definitions for the formats `code` names that jq does not have (jaq's `@htmld` among them),
/// each failing as jq's `format` does when it formats something: `NAME is not a valid format`.
/// A compiled program keeps its definitions' text for good, so each name's is made once.
fn unknown_format_defs(code: &str) -> Vec<load::parse::Def<&'static str>> {
    thread_local! {
        static DEFS: RefCell<std::collections::HashMap<String, &'static str>> =
            RefCell::new(std::collections::HashMap::new());
    }
    unknown_formats(code)
        .into_iter()
        .flat_map(|name| {
            let text = DEFS.with(|defs| {
                *defs.borrow_mut().entry(name.clone()).or_insert_with(|| {
                    let text = format!("def @{name}: error(\"{name} is not a valid format\");");
                    Box::leak(text.into_boxed_str())
                })
            });
            load::parse(text, |parser| parser.defs()).unwrap_or_default()
        })
        .collect()
}

/// The formats `code` names that jq does not have.
fn unknown_formats(code: &str) -> Vec<String> {
    let bytes = code.as_bytes();
    let mut names = std::collections::BTreeSet::new();
    for (at, byte) in bytes.iter().enumerate() {
        if *byte != b'@' {
            continue;
        }
        let len = bytes[at + 1..]
            .iter()
            .take_while(|b| b.is_ascii_alphanumeric() || **b == b'_')
            .count();
        let name = &code[at + 1..at + 1 + len];
        if len > 0 && !JQ_FORMATS.contains(&name) {
            names.insert(name);
        }
    }
    names.into_iter().map(str::to_owned).collect()
}

/// The global holding `get_jq_origin`'s value: the directory part of the command as invoked
/// (jq's `dirname(argv[0])`, so `.` for a plain `jq`).
const JQ_ORIGIN: &str = "$!jq_origin";

/// The global holding `get_prog_origin`'s value: the real path of the program file's directory,
/// or of the working directory for a program on the command line.
const PROG_ORIGIN: &str = "$!prog_origin";

/// POSIX `dirname`: the path without its last component and trailing slashes.
fn dirname(path: &str) -> &str {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        return if path.is_empty() { "." } else { "/" };
    }
    match trimmed.rfind('/') {
        None => ".",
        Some(slash) => match trimmed[..slash].trim_end_matches('/') {
            "" => "/",
            parent => parent,
        },
    }
}

/// jq's `jq_realpath`: the real path of `path` (against the working directory), or `path`
/// itself when it cannot be resolved.
fn realpath(path: &str, resolve: &dyn Fn(&str) -> PathBuf) -> String {
    std::fs::canonicalize(resolve(path)).map_or_else(
        |_| path.to_owned(),
        |real| real.to_string_lossy().into_owned(),
    )
}

/// The global holding `get_search_list`'s value, under a name no program can write.
const SEARCH_LIST: &str = "$!search_list";

/// jq 1.8's default module search path.
const DEFAULT_SEARCH_LIST: [&str; 3] = ["~/.jq", "$ORIGIN/../lib/jq", "$ORIGIN/../lib"];

/// Definitions where jq 1.8 differs from jaq's standard library, or that jaq keeps elsewhere.
const JQ_DEFS: &str = r#"
def halt_error($exit_code): halt_error_empty, halt($exit_code);
def halt_error: halt_error(5);
def join($x): reduce .[] as $i (null;
    (if . == null then "" else . + $x end) +
    ($i | if . == null then "" elif type == "boolean" or type == "number" then tojson else . end)
  ) // "";
def @csv:
  if type != "array" then error("\(type) (\(tojson)) cannot be csv-formatted, only array")
  else map(
    if type == "string" then "\"" + (split("\"") | join("\"\"")) + "\""
    elif type == "number" or type == "boolean" then tojson
    elif . == null then ""
    else error("\(type) (\(tojson)) is not valid in a csv row") end
  ) | join(",") end;
def @tsv:
  if type != "array" then error("\(type) (\(tojson)) cannot be tsv-formatted, only array")
  else map(
    if type == "string" then
      split("\\") | join("\\\\") | split("\t") | join("\\t")
      | split("\n") | join("\\n") | split("\r") | join("\\r")
    elif type == "number" or type == "boolean" then tojson
    elif . == null then ""
    else error("\(type) (\(tojson)) is not valid in a csv row") end
  ) | join("\t") end;
def inputs: try repeat(input) catch if . == "break" then empty else error end;
def builtins: [
  "have_literal_numbers/0", "have_decnum/0", "input_line_number/0", "input_filename/0", "now/0",
  "localtime/0", "gmtime/0", "mktime/0", "strflocaltime/1", "strftime/1", "strptime/1",
  "stderr/0", "debug/0", "input/0", "modulemeta/0", "get_jq_origin/0", "get_prog_origin/0",
  "get_search_list/0", "halt_error/1", "halt/0", "env/0", "format/1", "error/0", "max/0",
  "min/0", "bsearch/1", "unique/0", "sort/0", "nan/0", "infinite/0", "isnormal/0", "isnan/0",
  "isinfinite/0", "type/0", "utf8bytelength/0", "length/0", "contains/1", "has/1", "delpaths/1",
  "getpath/1", "setpath/2", "rtrim/0", "ltrim/0", "trim/0", "implode/0", "explode/0", "split/1",
  "endswith/1", "startswith/1", "keys_unsorted/0", "keys/0", "tostring/0", "toboolean/0",
  "tonumber/0", "fromjson/0", "tojson/0", "lgamma_r/0", "frexp/0", "modf/0", "ldexp/2",
  "trunc/0", "significand/0", "scalbln/2", "scalb/2", "round/0", "rint/0", "nexttoward/2",
  "nextafter/2", "nearbyint/0", "logb/0", "log1p/0", "lgamma/0", "gamma/0", "fmod/2", "fmin/2",
  "fmax/2", "fma/3", "fdim/2", "fabs/0", "expm1/0", "exp10/0", "erfc/0", "erf/0", "drem/2",
  "copysign/2", "ceil/0", "yn/2", "jn/2", "y1/0", "y0/0", "tgamma/0", "tanh/0", "tan/0",
  "sqrt/0", "sinh/0", "sin/0", "remainder/2", "pow/2", "log2/0", "log10/0", "log/0", "j1/0",
  "j0/0", "hypot/2", "floor/0", "exp2/0", "exp/0", "cosh/0", "cos/0", "cbrt/0", "atanh/0",
  "atan2/2", "atan/0", "asinh/0", "asin/0", "acosh/0", "acos/0", "empty/0", "not/0", "path/1",
  "last/1", "range/2", "halt_error/0", "error/1", "map/1", "select/1", "sort_by/1",
  "group_by/1", "unique_by/1", "max_by/1", "min_by/1", "add/1", "add/0", "del/1", "abs/0",
  "map_values/1", "recurse/1", "recurse/2", "recurse/0", "to_entries/0", "from_entries/0",
  "with_entries/1", "reverse/0", "indices/1", "index/1", "rindex/1", "paths/0", "paths/1",
  "isfinite/0", "arrays/0", "objects/0", "iterables/0", "booleans/0", "numbers/0", "normals/0",
  "finites/0", "strings/0", "nulls/0", "values/0", "scalars/0", "join/1", "flatten/1",
  "flatten/0", "range/1", "fromdateiso8601/0", "todateiso8601/0", "fromdate/0", "todate/0",
  "ltrimstr/1", "rtrimstr/1", "trimstr/1", "match/2", "match/1", "test/2", "test/1",
  "capture/2", "capture/1", "scan/2", "scan/1", "splits/2", "splits/1", "split/2", "sub/3",
  "sub/2", "gsub/3", "gsub/2", "while/2", "until/2", "limit/2", "skip/2", "range/3", "first/1",
  "isempty/1", "all/2", "any/2", "all/1", "any/1", "all/0", "any/0", "nth/2", "first/0",
  "last/0", "nth/1", "combinations/0", "combinations/1", "transpose/0", "in/1", "inside/1",
  "repeat/1", "inputs/0", "ascii_downcase/0", "ascii_upcase/0", "truncate_stream/1",
  "fromstream/1", "tostream/0", "walk/1", "pick/1", "debug/1", "INDEX/2", "INDEX/1", "JOIN/2",
  "JOIN/3", "JOIN/4", "IN/1", "IN/2", "builtins/0"
];
def format($f):
  if $f == "json" then tojson elif $f == "text" then tostring
  elif $f == "csv" then @csv elif $f == "tsv" then @tsv
  elif $f == "html" then @html elif $f == "uri" then @uri elif $f == "urid" then @urid
  elif $f == "sh" then @sh elif $f == "base64" then @base64 elif $f == "base64d" then @base64d
  elif ($f | type) == "string" then error("\($f) is not a valid format")
  else error("\($f | type) (\($f | tojson)) is not a valid format") end;
"#;

/// `input`, `input_line_number` and `input_filename`, over the run's [`Reader`].
fn input_funs() -> Box<[native::Filter<RunPtr<Kind>>]> {
    Box::new([
        ("input", v(0), |cv| {
            let next = cv.0.data().input.borrow_mut().next();
            Box::new(core::iter::once(match next {
                Some(Ok(value)) => Ok(value),
                // A parse error is an ordinary error here, and the end of input is jq's `break`.
                Some(Err(error)) => Err(Error::str(error).into()),
                None => Err(Error::str("break").into()),
            }))
        }),
        ("input_line_number", v(0), |cv| {
            let line = cv.0.data().input.borrow().reader.line;
            Box::new(core::iter::once(Ok(Val::from(line))))
        }),
        // Modules are unsupported, so there is no module to describe.
        ("modulemeta", v(0), |cv| {
            if string_bytes(&cv.1).is_none() {
                let message = "modulemeta input module name must be a string";
                return Box::new(core::iter::once(Err(Error::str(message).into())));
            }
            diagnostic(b"jq: modulemeta is unsupported in bash-tool\n");
            Box::new(core::iter::once(Err(jaq_core::Exn::halt(2))))
        }),
        ("input_filename", v(0), |cv| {
            let name = cv.0.data().input.borrow().reader.filename.clone();
            Box::new(core::iter::once(Ok(name.map_or(Val::Null, Val::from))))
        }),
    ])
}

fn funs() -> impl Iterator<Item = native::Fun<Kind>> {
    // Replaced below: `env` must see the shell's exported variables, not the process's.
    const REPLACED: [&str; 3] = ["env", "debug_empty", "stderr_empty"];
    let run = native::run::<Kind>;
    jaq_core::funs::<Kind>()
        .chain(jaq_std::funs::<Kind>().filter(|(name, ..)| !REPLACED.contains(name)))
        .chain(jaq_json::funs::<Kind>())
        .chain(input_funs().into_vec().into_iter().map(run))
        .chain(diagnostics_funs().into_vec().into_iter().map(run))
}

/// Where a compile error points, as jq prints it: `line L, column C:` plus the source line and
/// a caret under the offending text.
fn locate(code: &str, at: &str, width: usize) -> String {
    let offset = (at.as_ptr() as usize)
        .checked_sub(code.as_ptr() as usize)
        .filter(|offset| *offset <= code.len())
        .unwrap_or(code.len());
    let before = &code[..offset];
    let line = before.matches('\n').count() + 1;
    let start = before.rfind('\n').map_or(0, |index| index + 1);
    let column = offset - start + 1;
    let text = code[start..].lines().next().unwrap_or("");
    // jq underlines up to the end of the line, and at least one character.
    let width = width.min((start + text.len()).saturating_sub(offset).max(1));
    format!(
        "at <top-level>, line {line}, column {column}:\n    {text}\n    {}{}\n",
        " ".repeat(column - 1),
        "^".repeat(width.max(1))
    )
}

fn load_error(code: &str, error: &load::Error<&str>) -> Vec<String> {
    match error {
        load::Error::Io(errors) => errors
            .iter()
            .map(|(path, error)| format!("jq: error: {path}: {error}\n"))
            .collect(),
        load::Error::Lex(errors) => errors
            .iter()
            .map(|(expect, at)| match expect.message() {
                // jq's own words for a bad run of string escapes, under the whole run
                Some(message) => format!("jq: error: {message} {}", locate(code, at, at.len())),
                None => format!(
                    "jq: error: syntax error, expected {} {}",
                    expect.as_str(),
                    locate(code, at, 1)
                ),
            })
            .collect(),
        load::Error::Parse(errors) => errors
            .iter()
            .map(|(expect, at)| {
                let unexpected = if at.is_empty() {
                    "unexpected end of file".to_owned()
                } else {
                    format!("unexpected {at:?}")
                };
                format!(
                    "jq: error: syntax error, {unexpected}, expected {} {}",
                    expect.as_str(),
                    locate(code, at, at.len())
                )
            })
            .collect(),
    }
}

/// What each of `texts`, constant expressions as jq's parser takes them, folds to: `None` for a
/// fold that fails (`"a" + 1`), which jq leaves to run time.
fn fold_constants(texts: &[&str]) -> Vec<Option<Val>> {
    if texts.is_empty() {
        return Vec::new();
    }
    // One program folds them all: each value in a one-element array, `null` for a fold that
    // fails. Each text is written as in the program (its own lines, comments included).
    let folds: Vec<String> = texts
        .iter()
        .map(|text| format!("try [({text}\n)] catch null"))
        .collect();
    let folds = format!("[{}]", folds.join(",\n"));
    let arena = Arena::default();
    let loader = Loader::new(
        jaq_core::defs()
            .chain(jaq_std::defs())
            .chain(jaq_json::defs()),
    );
    let failed = || vec![None; texts.len()];
    let Ok(modules) = loader.load(
        &arena,
        File {
            code: &*folds,
            path: (),
        },
    ) else {
        return failed();
    };
    let Ok(filter) = Compiler::default().with_funs(funs()).compile(modules) else {
        return failed();
    };
    let input = RefCell::new(Input::new(
        Reader::new(Vec::new(), true, false, false),
        Vec::new(),
    ));
    let data = Data {
        lut: &filter.lut,
        input: &input,
    };
    let ctx = Ctx::<Kind>::new(&data, Vars::new([]));
    let Some(Ok(Val::Arr(values))) = filter.id.run((ctx, Val::Null)).next() else {
        return failed();
    };
    values
        .iter()
        .map(|value| match value {
            Val::Arr(value) => value.first().cloned(),
            _ => None,
        })
        .collect()
}

/// What `text` folds to when jq takes it for a constant, for jq's parser (`jq_syntax`).
fn fold_constant(text: &str) -> Option<super::jq_syntax::Folded> {
    use jaq_core::ValT as _;
    if !load::is_const_term(text) {
        return None;
    }
    let value = fold_constants(&[text]).pop().flatten()?;
    Some((value.kind_name(), value.dump_trunc()))
}

/// The `break $x` whose label `name` (a slice of `code`) is, or `name` if no `break` comes
/// just before it.
fn break_of<'a>(code: &'a str, name: &'a str) -> &'a str {
    let Some(offset) = (name.as_ptr() as usize).checked_sub(code.as_ptr() as usize) else {
        return name;
    };
    let Some(before) = code.get(..offset).map(str::trim_end) else {
        return name;
    };
    match before.strip_suffix("break") {
        Some(rest) => code.get(rest.len()..offset + name.len()).unwrap_or(name),
        None => name,
    }
}

fn compile_failure(messages: Vec<String>) -> Failure {
    let count = messages.len();
    let mut message = messages.concat();
    let _ = writeln!(
        message,
        "jq: {count} compile error{}",
        if count == 1 { "" } else { "s" }
    );
    failure(3, message)
}

/// jq's refusal of a program nested deeper than the stack left holds.
const NESTED_TOO_DEEPLY: &str =
    "jq: maximum nesting level exceeded: deeper nesting is unsupported in bash-tool\n";

/// The deepest program (`jq_syntax::Parsed::depth`) jq runs, however much stack is left.
const MAX_DEPTH: i64 = 256;

/// Wasmtime's native stack, which WASM code runs on and whose exhaustion traps the whole tool.
const NATIVE_STACK: i64 = 512 * 1024;

/// What is left of the shadow stack (see [`stack`]) when a command runs at the top level.
const SHADOW_AT_TOP: i64 = 4_130_000;

/// The fewest shadow-stack bytes the shell uses per byte of native stack (measured between 6.3
/// and 9.8 over recursive functions, `$( )`, `eval` and `source`), so that what the shell has used
/// of the native stack is estimated from what it has used of the shadow stack, never
/// underestimated.
const SHELL_SHADOW_PER_NATIVE: i64 = 6;

/// The native stack jq needs besides its program's nesting (reading options, loading and compiling
/// the standard library), with margin (measured under 20 KiB).
const JQ_BASE_NATIVE: i64 = 48 * 1024;

/// The native stack jaq needs per level of a program's nesting, with margin (measured at most
/// 760 bytes, for nested arrays and definitions).
const NATIVE_PER_LEVEL: i64 = 1024;

/// The fewest shadow-stack bytes jaq's evaluation uses per byte of native stack (measured between
/// 0.63, for `flatten` on a deep input, and 1.8, for a recursive definition), as a fraction: the
/// shadow stack jaq may use is its share of the native stack left, at this rate.
const JAQ_SHADOW_PER_NATIVE: (i64, i64) = (1, 2);

/// The native stack left for jq, given the shadow stack left when it starts: what the shell has
/// not used of it (estimated from the shadow stack the shell has used), less jq's own needs.
fn native_left(shadow_left: i64) -> i64 {
    let shell = (SHADOW_AT_TOP - shadow_left).max(0) / SHELL_SHADOW_PER_NATIVE;
    NATIVE_STACK - shell - JQ_BASE_NATIVE
}

/// How deeply a program may nest, given the shadow stack left when jq starts: what the native
/// stack still holds, in levels, at most [`MAX_DEPTH`]; negative when even a program that does not
/// nest could exhaust it.
fn allowed_depth(shadow_left: i64) -> i64 {
    (native_left(shadow_left) / NATIVE_PER_LEVEL).min(MAX_DEPTH)
}

/// Make evaluation that would recurse past the native stack left (a recursive definition, `walk`
/// on a deep value, an update through a long path) fail with jq's refusal instead of trapping:
/// jaq stops at a floor on the shadow stack, which it uses in proportion.
fn limit_evaluation() {
    let (num, den) = JAQ_SHADOW_PER_NATIVE;
    let shadow = native_left(stack::remaining()).max(0) * num / den;
    let here = jaq_core::depth::here();
    let floor = here.saturating_sub(usize::try_from(shadow).unwrap_or(usize::MAX));
    #[cfg(target_arch = "wasm32")]
    jaq_core::depth::set_floor(floor.max(1));
    #[cfg(not(target_arch = "wasm32"))]
    let _ = floor;
}

/// The shadow stack: Rust's WASM targets keep it first in linear memory, growing down towards
/// `__stack_low`. What is left of it tells how deep the shell has recursed.
mod stack {
    #[cfg(target_arch = "wasm32")]
    unsafe extern "C" {
        /// The lowest address of the stack region, defined by the linker.
        static __stack_low: u8;
    }

    /// The bytes left between the current stack pointer and the bottom of the stack.
    #[cfg(target_arch = "wasm32")]
    #[inline(never)]
    pub(super) fn remaining() -> i64 {
        let marker = 0_u8;
        let current = std::ptr::addr_of!(marker) as usize;
        // only the address of the linker-defined symbol is taken; it is never read
        let low = std::ptr::addr_of!(__stack_low) as usize;
        i64::try_from(current.saturating_sub(low)).unwrap_or(i64::MAX)
    }

    /// Native code has no WASM stack to run out of: as much as at the top level.
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) const fn remaining() -> i64 {
        super::SHADOW_AT_TOP
    }
}

/// A compiled filter plus everything a run needs besides its inputs.
pub(crate) struct Program {
    filter: jaq_core::Filter<Kind>,
    globals: Vec<Val>,
    pp: write::Pp,
    null_input: bool,
    raw_input: bool,
    slurp: bool,
    raw_output: bool,
    join_output: bool,
    nul_output: bool,
    ascii_output: bool,
    seq: bool,
    exit_status: bool,
    buffered: bool,
}

impl Program {
    /// Compile the filter and bind variables. `resolve` maps an operand to a path against the
    /// shell's working directory; `env` is the shell's exported variables.
    pub(crate) fn compile(
        options: &Options,
        env: Vec<(String, String)>,
        resolve: &dyn Fn(&str) -> PathBuf,
    ) -> Result<Self, Failure> {
        // `-C` colors with jq's styles, or `JQ_COLORS`' when jq takes them.
        let styles = if options.color && !options.monochrome {
            let colors = env
                .iter()
                .find(|(key, _)| key == "JQ_COLORS")
                .map(|(_, value)| value.as_str());
            jq_styles(colors).unwrap_or_else(|()| {
                diagnostic(b"Failed to set $JQ_COLORS\n");
                jq_styles(None).unwrap_or_default()
            })
        } else {
            write::Styles::default()
        };
        // A module jq looks for in `~/.jq` needs the home directory.
        let home = env
            .iter()
            .find(|(key, _)| key == "HOME")
            .map(|(_, value)| value.clone())
            .filter(|home| !home.is_empty());
        let code = match &options.filter {
            None => ".".to_owned(),
            Some(FilterSource::Inline(code)) => code.clone(),
            Some(FilterSource::File(path)) => super::read_file(&resolve(path))
                .map(jq_utf8::lossy)
                .map_err(|error| {
                    failure(
                        2,
                        format!("jq: Could not open {path}: {}\n", io_message(&error)),
                    )
                })?,
        };
        let mut names = Vec::new();
        let mut values = Vec::new();
        let mut named = Vec::new();
        for (name, value) in &options.named {
            let value = named_value(name, value, resolve)?;
            names.push(format!("${name}"));
            named.push((Val::from(name.clone()), value.clone()));
            values.push(value);
        }
        let positional = options
            .positional
            .iter()
            .map(|(text, json)| {
                if *json {
                    jv_parse::parse_sized(text.as_bytes()).map_err(|_| {
                        failure(
                            2,
                            format!("jq: invalid JSON text passed to --jsonargs\n{UNKNOWN_HINT}"),
                        )
                    })
                } else {
                    Ok(Val::from(text.clone()))
                }
            })
            .collect::<Result<Vec<_>, _>>()?;
        names.push("$ARGS".into());
        values.push(Val::obj(
            [
                (
                    Val::from("positional".to_owned()),
                    positional.into_iter().collect(),
                ),
                (
                    Val::from("named".to_owned()),
                    Val::obj(named.into_iter().collect()),
                ),
            ]
            .into_iter()
            .collect(),
        ));
        names.push(SEARCH_LIST.into());
        values.push(if options.library_paths.is_empty() {
            DEFAULT_SEARCH_LIST
                .iter()
                .map(|p| Val::from((*p).to_owned()))
                .collect()
        } else {
            options
                .library_paths
                .iter()
                .map(|p| Val::from(p.clone()))
                .collect()
        });
        names.push(JQ_ORIGIN.into());
        values.push(Val::from(dirname(&options.command).to_owned()));
        names.push(PROG_ORIGIN.into());
        values.push(Val::from(match &options.filter {
            Some(FilterSource::File(path)) => realpath(dirname(path), resolve),
            _ => realpath(".", resolve),
        }));
        names.push("$ENV".into());
        values.push(Val::obj(
            env.into_iter()
                .map(|(key, value)| (Val::from(key), Val::from(value)))
                .collect(),
        ));

        // jq parses the program before anything else: its syntax errors, and the checks its
        // grammar makes (constant object keys, module metadata), are jq's own.
        let parsed = super::jq_syntax::check(&code, &mut fold_constant);
        if !parsed.errors.is_empty() {
            return Err(compile_failure(parsed.errors));
        }
        if !parsed.main {
            return Err(compile_failure(vec![
                "jq: error: Top-level program not given (try \".\")\n".into(),
            ]));
        }
        // jaq parses, compiles and runs a program recursively, on the stack the shell is using.
        if i64::from(parsed.depth) > allowed_depth(stack::remaining()) {
            return Err(failure(2, NESTED_TOO_DEEPLY));
        }
        // A format jq does not have is an error when it formats something, not when compiling.
        let format_defs = unknown_format_defs(&code);
        let arena = Arena::default();
        let loader = Loader::new(
            jaq_core::defs()
                .chain(jaq_std::defs())
                .chain(jaq_json::defs())
                .chain(extra_defs())
                .chain(format_defs),
        );
        let modules = loader
            .load(
                &arena,
                File {
                    code: &*code,
                    path: (),
                },
            )
            .map_err(|errors| {
                // jaq's loader fails an `import` or `include` it cannot read. One jq would not
                // find is jq's error; one it would find is refused, as modules are unsupported.
                let module = errors.iter().find_map(|(_, error)| match error {
                    load::Error::Io(modules) => modules.first().map(|(name, _)| *name),
                    _ => None,
                });
                if let Some(name) = module {
                    return module_failure(options, home.as_deref(), resolve, name, "jq");
                }
                compile_failure(
                    errors
                        .iter()
                        .flat_map(|(_, error)| load_error(&code, error))
                        .collect(),
                )
            })?;
        let mut data = None;
        let modules = load::import(&modules, |import| {
            data.get_or_insert(import.path.to_owned());
            Err("module imports are unsupported in bash-tool".into())
        })
        .map(|()| modules)
        .map_err(|errors| {
            if let Some(name) = &data {
                return module_failure(options, home.as_deref(), resolve, name, "json");
            }
            compile_failure(
                errors
                    .iter()
                    .flat_map(|(_, error)| load_error(&code, error))
                    .collect(),
            )
        })?;
        let filter = Compiler::default()
            .with_funs(funs())
            .with_global_vars(names.iter().map(String::as_str))
            .compile(modules)
            .map_err(|errors| {
                compile_failure(
                    errors
                        .iter()
                        .flat_map(|(_, errors)| errors)
                        .map(|(name, undefined)| {
                            use jaq_core::compile::Undefined;
                            let (what, at) = match undefined {
                                Undefined::Filter(arity) => (format!("{name}/{arity}"), *name),
                                // jq names a label as it binds it, at the whole `break $x`.
                                Undefined::Label => (
                                    format!("$*label-{}", name.trim_start_matches('$')),
                                    break_of(&code, name),
                                ),
                                _ => ((*name).to_owned(), *name),
                            };
                            format!(
                                "jq: error: {what} is not defined {}",
                                locate(&code, at, at.len())
                            )
                        })
                        .collect(),
                )
            })?;
        // `input`/`inputs` pull further values mid-filter, so every value must be available.
        let pulls = code
            .split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '$'))
            .any(|word| word == "input" || word == "inputs");
        Ok(Self {
            filter,
            globals: values,
            pp: write::Pp {
                indent: options.indent.clone(),
                sort_keys: options.sort_keys,
                styles,
                sep_space: options.indent.is_some(),
            },
            null_input: options.null_input,
            raw_input: options.raw_input,
            slurp: options.slurp,
            raw_output: options.raw_output,
            join_output: options.join_output,
            nul_output: options.nul_output,
            ascii_output: options.ascii_output,
            seq: options.seq,
            exit_status: options.exit_status,
            buffered: options.slurp || pulls,
        })
    }

    /// Whether input is a JSON text sequence (`--seq`).
    #[cfg_attr(
        not(target_arch = "wasm32"),
        expect(dead_code, reason = "only the streaming WASM driver asks")
    )]
    pub(crate) const fn seq(&self) -> bool {
        self.seq
    }

    /// Whether the filter needs all input before it can start (`-s`, `input`, `inputs`).
    #[cfg_attr(
        not(target_arch = "wasm32"),
        expect(dead_code, reason = "only the streaming WASM driver asks")
    )]
    pub(crate) const fn buffered(&self) -> bool {
        self.buffered
    }

    /// Whether the program reads any input at all; `-n` without `input`/`inputs` does not.
    pub(crate) const fn reads_input(&self) -> bool {
        !self.null_input || self.buffered
    }

    /// The reader for this run's operands (standard input when there are none).
    pub(crate) fn reader(&self, files: &[String]) -> Reader {
        let operands = if files.is_empty() {
            vec!["-".to_owned()]
        } else {
            files.to_vec()
        };
        Reader::new(operands, !self.raw_input, self.slurp, self.seq)
    }

    /// Run the filter on one input, as jq's `process` does: every output, or the outputs up to
    /// the first uncaught error. Returns jq's `ret` for it.
    pub(crate) fn process(
        &self,
        value: Val,
        input: &RefCell<Input>,
        state: &mut RunState,
        streams: &mut Streams,
    ) -> i32 {
        let data = Data {
            lut: &self.filter.lut,
            input,
        };
        let ctx = Ctx::<Kind>::new(&data, Vars::new(self.globals.clone()));
        let mut ret = OK_NO_OUTPUT;
        for output in self.filter.id.run((ctx, value)) {
            input.borrow_mut().drain_messages();
            streams.err(&take_diagnostics());
            match output {
                Ok(value) => {
                    ret = if string_bytes(&value).is_some() && self.raw_output {
                        0
                    } else if matches!(value, Val::Null | Val::Bool(false)) {
                        OK_NULL_KIND
                    } else {
                        0
                    };
                    if let Err(error) = self.emit(&value, streams) {
                        let _ = writeln!(Diagnostic(streams), "jq: error: {error}");
                        state.write_failed = true;
                        // As with a filter error: no further outputs for this input.
                        break;
                    }
                }
                Err(exception) => match exception.get_err() {
                    Ok(error) => {
                        let value = error.into_val();
                        let at = input.borrow().reader.position();
                        let _ = match string_bytes(&value) {
                            Some(text) => writeln!(
                                Diagnostic(streams),
                                "jq: error (at {at}): {}",
                                String::from_utf8_lossy(text)
                            ),
                            None => writeln!(
                                Diagnostic(streams),
                                "jq: error (at {at}) (not a string): {}",
                                String::from_utf8_lossy(&compact(&value))
                            ),
                        };
                        // jq ends an input's outputs at its first uncaught error.
                        ret = ERROR_STATUS;
                        break;
                    }
                    Err(exception) => {
                        // Evaluation that would have exhausted the stack is refused, as a
                        // program nested too deeply is.
                        if exception.is_too_deep() {
                            streams.err(NESTED_TOO_DEEPLY.as_bytes());
                            state.halted = true;
                            ret = 2;
                            break;
                        }
                        if let Ok(code) = exception.get_halt() {
                            state.halted = true;
                            ret = code;
                            break;
                        }
                    }
                },
            }
        }
        input.borrow_mut().drain_messages();
        streams.err(&take_diagnostics());
        ret
    }

    /// jq's main loop after `process` returns for an input value.
    pub(crate) fn processed(&self, ret: i32, state: &mut RunState) {
        state.ret = ret;
        if ret <= 0 && ret != OK_NO_OUTPUT {
            state.last_result = i32::from(ret != OK_NULL_KIND);
        }
    }

    /// Emit one output value, however deeply it is nested (`jaq_json::write` does not recurse;
    /// as jq, it writes what lies more than 10,000 levels deep as `<skipped: too deep>`). `Err`
    /// means the write failed; the caller reports it and stops, rather than let `out` end up
    /// with the partial JSON text this may still have appended.
    fn emit(&self, value: &Val, streams: &mut Streams) -> Result<(), std::io::Error> {
        let mut out = Vec::new();
        self.emit_value(value, &mut out)?;
        streams.out(&out);
        // jq writes the separator on its own, as musl's line buffering sees it.
        if self.nul_output {
            streams.out(&[0]);
        } else if !self.join_output {
            streams.out(b"\n");
        }
        Ok(())
    }

    fn emit_value(&self, value: &Val, out: &mut Vec<u8>) -> Result<(), std::io::Error> {
        match string_bytes(value) {
            // jq quotes strings under `-a` even with `-r`, and prefixes only JSON texts for `--seq`.
            Some(text) if self.raw_output && !self.ascii_output => {
                out.extend_from_slice(&utf8_replace_invalid(text));
            }
            _ => {
                if self.seq {
                    out.push(0x1e);
                }
                let mut json = Vec::new();
                write::write(&mut json, &self.pp, 0, value)?;
                // jaq's own writer intentionally preserves an invalid byte as-is rather than
                // ever fail to print (its own documented design: "printing a value should
                // always succeed" — see jaq-json's `write::write` doc comment), but a bare
                // invalid byte is exactly what real jq never emits: its own JSON reader
                // replaces one with the Unicode replacement character the moment it decodes
                // the string, so it never reaches jq's output un-substituted. Redone here as a
                // single pass over the whole assembled buffer rather than a recursive walk of
                // `value` itself, both because it's simpler and because a naive recursive walk
                // would reintroduce the unbounded native recursion already removed from
                // `write::write` itself — every other byte this buffer holds is JSON
                // structural syntax, which is pure ASCII and so is never touched by a lossy
                // UTF-8 pass. Verified against the oracle.
                let json = utf8_replace_invalid(&json);
                if self.ascii_output {
                    escape_non_ascii(&json, out);
                } else {
                    out.extend_from_slice(&json);
                }
            }
        }
        Ok(())
    }

    /// jq's exit status once the run is over; `failures` counts operands jq could not read.
    pub(crate) fn exit_code(&self, state: &RunState, failures: usize) -> i32 {
        let mut ret = state.ret;
        if failures != 0 || state.write_failed {
            ret = 2;
        }
        if self.exit_status {
            if ret != OK_NO_OUTPUT {
                return ret.abs();
            }
            return match state.last_result {
                -1 => OK_NO_OUTPUT.abs(),
                0 => 1,
                _ => 0,
            };
        }
        ret.max(0)
    }
}

/// jq's `ret` and `last_result` across a run.
pub(crate) struct RunState {
    ret: i32,
    last_result: i32,
    /// `halt` or `halt_error` ran: nothing more runs.
    halted: bool,
    /// An output could not be written.
    write_failed: bool,
}

impl Default for RunState {
    fn default() -> Self {
        Self {
            ret: OK_NO_OUTPUT,
            last_result: -1,
            halted: false,
            write_failed: false,
        }
    }
}

impl RunState {
    pub(crate) const fn halted(&self) -> bool {
        self.halted
    }

    /// Invalid input JSON: jq reports it and stops, unless it reads a JSON text sequence
    /// (`--seq`), where it reports it and goes on. Returns whether to stop.
    pub(crate) fn parse_error(&mut self, error: &str, seq: bool, streams: &mut Streams) -> bool {
        if seq {
            let _ = writeln!(Diagnostic(streams), "jq: ignoring parse error: {error}");
            return false;
        }
        let _ = writeln!(Diagnostic(streams), "jq: parse error: {error}");
        self.ret = ERROR_STATUS;
        true
    }
}

/// `write!` into a byte buffer.
struct Diagnostic<'a, W: Diagnose>(&'a mut W);

/// Where a diagnostic goes: a message buffer, or jq's standard error.
trait Diagnose {
    fn diagnose(&mut self, text: &[u8]);
}

impl Diagnose for Vec<u8> {
    fn diagnose(&mut self, text: &[u8]) {
        self.extend_from_slice(text);
    }
}

impl Diagnose for Streams {
    fn diagnose(&mut self, text: &[u8]) {
        self.err(text);
    }
}

impl<W: Diagnose> std::fmt::Write for Diagnostic<'_, W> {
    fn write_str(&mut self, text: &str) -> std::fmt::Result {
        self.0.diagnose(text.as_bytes());
        Ok(())
    }
}

/// Standard output or standard error.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Stream {
    Out,
    Err,
}

/// jq's standard output and error as the C library it runs on (musl, as in the reference jq)
/// delivers them, in order: standard error unbuffered, standard output line-buffered until its
/// first line is written (when musl finds it is not a terminal) and fully buffered in 1,024
/// bytes after that. So `debug` and `stderr` land between outputs as they do from jq.
#[derive(Default)]
pub(crate) struct Streams {
    writes: Vec<(Stream, Vec<u8>)>,
    buffered: Vec<u8>,
    full_buffering: bool,
}

impl Streams {
    /// musl's `BUFSIZ`, the size of standard output's buffer.
    const BUFFER: usize = 1024;

    /// A write to standard output.
    pub(crate) fn out(&mut self, mut data: &[u8]) {
        if !self.full_buffering {
            if let Some(end) = data.iter().rposition(|byte| *byte == b'\n') {
                let mut written = std::mem::take(&mut self.buffered);
                written.extend_from_slice(&data[..=end]);
                self.write(Stream::Out, written);
                self.full_buffering = true;
                data = &data[end + 1..];
            }
            self.buffered.extend_from_slice(data);
        } else if data.len() > Self::BUFFER - self.buffered.len().min(Self::BUFFER) {
            let mut written = std::mem::take(&mut self.buffered);
            written.extend_from_slice(data);
            self.write(Stream::Out, written);
        } else {
            self.buffered.extend_from_slice(data);
        }
    }

    /// A write to standard error.
    pub(crate) fn err(&mut self, data: &[u8]) {
        if !data.is_empty() {
            self.write(Stream::Err, data.to_vec());
        }
    }

    fn write(&mut self, stream: Stream, bytes: Vec<u8>) {
        match self.writes.last_mut() {
            Some((last, written)) if *last == stream => written.extend(bytes),
            _ => self.writes.push((stream, bytes)),
        }
    }

    /// The writes made so far, leaving standard output's buffer for later.
    pub(crate) fn take(&mut self) -> Vec<(Stream, Vec<u8>)> {
        std::mem::take(&mut self.writes)
    }

    /// Every write, standard output's buffer flushed as jq exits.
    pub(crate) fn finish(&mut self) -> Vec<(Stream, Vec<u8>)> {
        let buffered = std::mem::take(&mut self.buffered);
        if !buffered.is_empty() {
            self.write(Stream::Out, buffered);
        }
        self.take()
    }
}

fn named_value(
    name: &str,
    value: &Named,
    resolve: &dyn Fn(&str) -> PathBuf,
) -> Result<Val, Failure> {
    let read_file = |option: &str, path: &str| {
        super::read_file(&resolve(path)).map_err(|error| {
            failure(
                2,
                format!(
                    "jq: Bad JSON in --{option} {name} {path}: Could not open {path}: {}\n",
                    io_message(&error)
                ),
            )
        })
    };
    match value {
        Named::Str(text) => Ok(Val::from(text.clone())),
        Named::Json(text) => jv_parse::parse_sized(text.as_bytes()).map_err(|_| {
            failure(
                2,
                format!("jq: invalid JSON text passed to --argjson\n{UNKNOWN_HINT}"),
            )
        }),
        Named::RawFile(path) => Ok(Val::from(jq_utf8::lossy(read_file("rawfile", path)?))),
        Named::SlurpFile(path) => {
            // jq's `jv_load_file`: every value in the file, or its first syntax error.
            let bytes = read_file("slurpfile", path)?;
            let mut parser = Parser::new();
            parser.set_buf(&bytes, false);
            let mut values = Vec::new();
            loop {
                match parser.next_value() {
                    Next::Value(value) => values.push(value),
                    Next::More => break Ok(values.into_iter().collect()),
                    Next::Error(error) => {
                        break Err(failure(
                            2,
                            format!("jq: Bad JSON in --slurpfile {name} {path}: {error}\n"),
                        ));
                    }
                }
            }
        }
    }
}

fn escape_non_ascii(json: &[u8], out: &mut Vec<u8>) {
    for character in String::from_utf8_lossy(json).chars() {
        if character.is_ascii() {
            out.push(character as u8);
        } else {
            let mut units = [0u16; 2];
            for unit in character.encode_utf16(&mut units) {
                out.extend(format!("\\u{unit:04x}").bytes());
            }
        }
    }
}

/// Everything a finished (or refused) invocation writes, and its exit status.
#[derive(Default)]
pub(crate) struct Outcome {
    pub(crate) code: i32,
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: Vec<u8>,
    /// Both, in the order jq writes them (see `Streams`); empty when only one is written.
    pub(crate) writes: Vec<(Stream, Vec<u8>)>,
}

impl Outcome {
    /// The writes to make, in order.
    pub(crate) fn writes(&self) -> Vec<(Stream, &[u8])> {
        if self.writes.is_empty() {
            vec![
                (Stream::Out, &self.stdout[..]),
                (Stream::Err, &self.stderr[..]),
            ]
        } else {
            self.writes
                .iter()
                .map(|(stream, bytes)| (*stream, &bytes[..]))
                .collect()
        }
    }

    fn from_streams(code: i32, mut streams: Streams) -> Self {
        let writes = streams.finish();
        let mut outcome = Self {
            code,
            ..Self::default()
        };
        for (stream, bytes) in &writes {
            match stream {
                Stream::Out => outcome.stdout.extend(bytes),
                Stream::Err => outcome.stderr.extend(bytes),
            }
        }
        outcome.writes = writes;
        outcome
    }
}

/// Parse and compile, or produce the complete outcome of `--help`, `--version` or a refusal.
/// Whether `argv` names standard input (`/dev/stdin`, `/dev/fd/0`) for a file jq reads while it
/// compiles: `-f`'s program file, or a `--rawfile` or `--slurpfile` value.
#[cfg_attr(
    not(target_arch = "wasm32"),
    allow(
        dead_code,
        reason = "only the wasm driver stages standard input for the compile"
    )
)]
pub(crate) fn reads_stdin_file(argv: &[String]) -> bool {
    let Ok(Parsed::Run(options)) = parse(argv) else {
        return false;
    };
    let stdin = |path: &str| {
        super::devices::classify(std::path::Path::new(path))
            == Some(super::devices::Device::Stream(0))
    };
    matches!(&options.filter, Some(FilterSource::File(path)) if stdin(path))
        || options.named.iter().any(|(_, value)| {
            matches!(value, Named::RawFile(path) | Named::SlurpFile(path) if stdin(path))
        })
}

pub(crate) fn prepare(
    argv: &[String],
    env: Vec<(String, String)>,
    resolve: &dyn Fn(&str) -> PathBuf,
) -> Result<(Box<Options>, Program), Outcome> {
    let refuse = |failure: Failure| Outcome {
        code: failure.code,
        stderr: failure.message.into_bytes(),
        ..Outcome::default()
    };
    limit_evaluation();
    let options = match parse(argv) {
        Ok(Parsed::Run(options)) => options,
        Ok(Parsed::Help) => {
            return Err(Outcome {
                stdout: HELP.as_bytes().to_vec(),
                ..Outcome::default()
            });
        }
        Ok(Parsed::Version) => {
            return Err(Outcome {
                stdout: version().as_bytes().to_vec(),
                ..Outcome::default()
            });
        }
        Err(failure) => return Err(refuse(failure)),
    };
    let program = Program::compile(&options, env, resolve).map_err(refuse)?;
    Ok((options, program))
}

/// jq reads input in `fgets` chunks of at most this many bytes, and a chunk ends after a
/// newline; its line numbers count chunks that hold one.
const CHUNK: usize = 4091;

/// What the reader needs from its driver before it can go on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Need {
    /// Open this operand and report it with [`Reader::opened`].
    Open(usize),
    /// Read more of the open operand and report it with [`Reader::feed`] or
    /// [`Reader::failed`].
    Bytes,
}

/// An operand being read.
#[derive(Default)]
struct Source {
    bytes: Vec<u8>,
    pos: usize,
    /// The driver has no more bytes to give.
    done: bool,
    /// A read failed with this message (jq's `ferror`).
    error: Option<String>,
    /// A read hit the end of the file (jq's `feof`).
    eof: bool,
}

enum Slurped {
    Values(Vec<Val>),
    Text(String),
}

/// jq's input reader (`jq_util_input_state`, util.c), independent of how bytes arrive: every
/// operand read in turn through one parser (so a value may span files), cut into `fgets`
/// chunks for line numbers, with jq's messages for operands it cannot read.
pub(crate) struct Reader {
    operands: Vec<String>,
    next_operand: usize,
    source: Option<Source>,
    /// The first half of `read_more` (closing and opening operands) is done.
    opened_next: bool,
    /// `None` reads raw lines (`-R`).
    parser: Option<Parser>,
    slurped: Option<Slurped>,
    /// A raw line read so far.
    line_text: Option<String>,
    /// `is_last` of the `next_input` call in progress.
    is_last: bool,
    /// `input_filename`: the operand being read, once one has been opened.
    pub(crate) filename: Option<String>,
    /// `input_line_number`: newline-ended chunks read from it.
    pub(crate) line: usize,
    /// Operands jq could not read; its main loop stops at the next value.
    pub(crate) failures: usize,
    /// Messages for standard error, in order.
    pub(crate) messages: Vec<u8>,
}

impl Reader {
    fn new(operands: Vec<String>, json: bool, slurp: bool, seq: bool) -> Self {
        let slurped = match (slurp, json) {
            (false, _) => None,
            (true, true) => Some(Slurped::Values(Vec::new())),
            (true, false) => Some(Slurped::Text(String::new())),
        };
        Self {
            operands,
            next_operand: 0,
            source: None,
            opened_next: false,
            parser: json.then(|| Parser::with_seq(seq)),
            slurped,
            line_text: None,
            is_last: false,
            filename: None,
            line: 0,
            failures: 0,
            messages: Vec::new(),
        }
    }

    #[cfg_attr(
        not(target_arch = "wasm32"),
        expect(
            dead_code,
            reason = "only the streaming WASM driver opens operands itself"
        )
    )]
    pub(crate) fn operand(&self, index: usize) -> &str {
        &self.operands[index]
    }

    /// Where a filter error happened, as jq reports it: `NAME:LINE`, or `<unknown>` before any
    /// input was read.
    fn position(&self) -> String {
        match &self.filename {
            Some(name) => format!("{name}:{}", self.line),
            None => "<unknown>".to_owned(),
        }
    }

    /// The operand asked for with [`Need::Open`] is open (`Ok`), or could not be opened.
    pub(crate) fn opened(&mut self, result: Result<(), String>) {
        let name = &self.operands[self.next_operand];
        self.next_operand += 1;
        self.line = 0;
        self.filename = Some(if name == "-" { "<stdin>" } else { name }.to_owned());
        match result {
            Ok(()) => self.source = Some(Source::default()),
            Err(message) => {
                let _ = writeln!(
                    Diagnostic(&mut self.messages),
                    "jq: error: Could not open file {name}: {message}"
                );
                self.failures += 1;
            }
        }
    }

    /// More bytes of the open operand; `done` when there are no more.
    pub(crate) fn feed(&mut self, bytes: &[u8], done: bool) {
        if let Some(source) = &mut self.source {
            source.bytes.drain(..source.pos);
            source.pos = 0;
            source.bytes.extend_from_slice(bytes);
            source.done |= done;
        }
    }

    /// Reading the open operand failed.
    pub(crate) fn failed(&mut self, message: String) {
        if let Some(source) = &mut self.source {
            source.error = Some(message);
            source.done = true;
        }
    }

    /// jq's `jq_util_input_read_more`: move on from a finished operand, then read one chunk.
    /// Returns the chunk and whether it is the last.
    fn read_more(&mut self) -> Result<(Vec<u8>, bool), Need> {
        if !self.opened_next {
            if self
                .source
                .as_ref()
                .is_none_or(|source| source.eof || source.error.is_some())
            {
                if let Some(Source {
                    error: Some(error), ..
                }) = self.source.take()
                {
                    let _ = writeln!(Diagnostic(&mut self.messages), "jq: error: {error}");
                }
                if self.next_operand < self.operands.len() {
                    self.opened_next = true;
                    return Err(Need::Open(self.next_operand));
                }
            }
            self.opened_next = true;
        }
        let chunk = match &mut self.source {
            None => Vec::new(),
            Some(source) => {
                let rest = &source.bytes[source.pos..];
                let newline = rest.iter().take(CHUNK).position(|b| *b == b'\n');
                let (mut len, mut eof) = match newline {
                    Some(index) => (index + 1, false),
                    None if rest.len() >= CHUNK => (CHUNK, false),
                    // At a read error `fgets` fails: jq counts it and closes the operand next.
                    None if source.done && rest.is_empty() && source.error.is_some() => {
                        self.failures += 1;
                        (0, false)
                    }
                    None if source.done => (rest.len(), source.error.is_none()),
                    None => return Err(Need::Bytes),
                };
                if len == CHUNK && newline.is_none() {
                    // jq reads on to the end of a character the chunk would split.
                    if let Some(missing) = jq_utf8::missing(&rest[..CHUNK]).filter(|m| *m > 0) {
                        let available = rest.len() - CHUNK;
                        if available < missing && !source.done {
                            return Err(Need::Bytes);
                        }
                        len += missing.min(available);
                        eof = available < missing && source.error.is_none();
                    }
                }
                source.eof |= eof;
                if newline.is_some() {
                    self.line += 1;
                }
                let chunk = rest[..len].to_vec();
                source.pos += len;
                chunk
            }
        };
        self.opened_next = false;
        let is_last = self.next_operand == self.operands.len() && self.source.is_none();
        Ok((chunk, is_last))
    }

    /// jq's `jq_util_input_next_input`: the next value (for `-s`, the one slurped value), a
    /// parse error, or `None` at the end; or what it needs first.
    pub(crate) fn next_input(&mut self) -> Result<Option<Result<Val, String>>, Need> {
        loop {
            if let Some(parser) = &mut self.parser {
                if parser.remaining() == 0 {
                    let (chunk, is_last) = self.read_more()?;
                    self.is_last = is_last;
                    if let Some(parser) = &mut self.parser {
                        parser.set_buf(&chunk, !is_last);
                    }
                }
                let next = match &mut self.parser {
                    Some(parser) => parser.next_value(),
                    None => Next::More,
                };
                match (next, &mut self.slurped) {
                    (Next::Value(value), Some(Slurped::Values(values))) => values.push(value),
                    (Next::Value(value), _) => return Ok(self.finish(Some(Ok(value)))),
                    (Next::Error(error), _) => return Ok(self.finish(Some(Err(error)))),
                    (Next::More, _) => (),
                }
            } else {
                let (chunk, is_last) = self.read_more()?;
                self.is_last = is_last;
                if !chunk.is_empty() {
                    if let Some(Slurped::Text(text)) = &mut self.slurped {
                        text.push_str(&jq_utf8::lossy(chunk));
                    } else if let Some(line) = chunk.strip_suffix(b"\n") {
                        let mut text = self.line_text.take().unwrap_or_default();
                        text.push_str(&jq_utf8::lossy(line.to_vec()));
                        return Ok(self.finish(Some(Ok(Val::from(text)))));
                    } else {
                        let text = self.line_text.get_or_insert_default();
                        text.push_str(&jq_utf8::lossy(chunk));
                    }
                }
            }
            if self.is_last {
                break;
            }
        }
        let value = match self.slurped.take() {
            Some(Slurped::Values(values)) => Some(Ok(values.into_iter().collect())),
            Some(Slurped::Text(text)) => Some(Ok(Val::from(text))),
            None => self.line_text.take().map(|text| Ok(Val::from(text))),
        };
        Ok(self.finish(value))
    }

    fn finish(&mut self, value: Option<Result<Val, String>>) -> Option<Result<Val, String>> {
        self.is_last = false;
        value
    }
}

/// Every operand's bytes, read before the run: for the native builtin, and for WASM filters
/// that need all input first (`-s`, `input`, `inputs`).
pub(crate) enum Preloaded {
    /// The operand's bytes, and the error that ended reading it early, if one did.
    Read(Vec<u8>, Option<String>),
    /// Why it could not be opened.
    Unopened(String),
}

/// A run's reader, with the preloaded operands that feed it when a filter pulls input itself.
pub(crate) struct Input {
    pub(crate) reader: Reader,
    preloaded: Vec<Option<Preloaded>>,
    /// Where [`Reader::messages`] go while the filter runs.
    to_diagnostics: bool,
}

impl Input {
    pub(crate) fn new(reader: Reader, preloaded: Vec<Preloaded>) -> Self {
        Self {
            reader,
            preloaded: preloaded.into_iter().map(Some).collect(),
            to_diagnostics: true,
        }
    }

    /// The next input, reading preloaded operands as the reader asks for them; with nothing
    /// preloaded (a streaming run, where no filter pulls input itself) the input is over.
    fn next(&mut self) -> Option<Result<Val, String>> {
        let next = loop {
            match self.reader.next_input() {
                Ok(next) => break next,
                Err(Need::Open(index)) => {
                    match self.preloaded.get_mut(index).and_then(Option::take) {
                        Some(Preloaded::Read(bytes, error)) => {
                            self.reader.opened(Ok(()));
                            self.reader.feed(&bytes, true);
                            if let Some(error) = error {
                                self.reader.failed(error);
                            }
                        }
                        Some(Preloaded::Unopened(error)) => self.reader.opened(Err(error)),
                        None => self.reader.opened(Ok(())),
                    }
                }
                Err(Need::Bytes) => self.reader.feed(&[], true),
            }
        };
        self.drain_messages();
        next
    }

    /// Move the reader's messages to the command's diagnostics while a filter runs.
    fn drain_messages(&mut self) {
        if self.to_diagnostics && !self.reader.messages.is_empty() {
            diagnostic(&std::mem::take(&mut self.reader.messages));
        }
    }
}

/// Run over preloaded operands: the native builtin, and WASM filters that need all input.
pub(crate) fn run_preloaded(
    program: &Program,
    files: &[String],
    preloaded: Vec<Preloaded>,
) -> Outcome {
    let input = RefCell::new(Input::new(program.reader(files), preloaded));
    let mut state = RunState::default();
    let mut streams = Streams::default();
    if program.null_input {
        let ret = program.process(Val::Null, &input, &mut state, &mut streams);
        program.processed(ret, &mut state);
    } else {
        // jq checks for unreadable operands before each value, not while reading one.
        while input.borrow().reader.failures == 0 {
            let next = input.borrow_mut().next();
            streams.err(&take_diagnostics());
            match next {
                Some(Ok(value)) => {
                    let ret = program.process(value, &input, &mut state, &mut streams);
                    program.processed(ret, &mut state);
                    if state.halted() {
                        break;
                    }
                }
                Some(Err(error)) => {
                    if state.parse_error(&error, program.seq, &mut streams) {
                        break;
                    }
                }
                None => break,
            }
        }
    }
    let failures = input.borrow().reader.failures;
    Outcome::from_streams(program.exit_code(&state, failures), streams)
}

/// Read one operand whole, telling an operand that cannot be opened from one that cannot be
/// read (a directory opens, then fails to read, as with jq's `fopen` and `fgets`).
pub(crate) fn preload(path: &std::path::Path) -> Preloaded {
    use std::io::Read as _;
    if path == std::path::Path::new("/dev/null") {
        return Preloaded::Read(Vec::new(), None);
    }
    match std::fs::File::open(path) {
        Err(error) => Preloaded::Unopened(io_message(&error)),
        Ok(mut file) => {
            let mut bytes = Vec::new();
            let error = file.read_to_end(&mut bytes).err();
            Preloaded::Read(bytes, error.map(|error| io_message(&error)))
        }
    }
}

/// The native builtin: every operand is read up front.
pub(crate) fn run_buffered(
    argv: &[String],
    env: Vec<(String, String)>,
    resolve: &dyn Fn(&str) -> PathBuf,
    stdin: &mut dyn std::io::Read,
    out: &mut dyn std::io::Write,
    err: &mut dyn std::io::Write,
) -> std::io::Result<i32> {
    let outcome = match prepare(argv, env, resolve) {
        Err(outcome) => outcome,
        Ok((options, program)) => {
            let mut read_stdin = || {
                let mut bytes = Vec::new();
                match stdin.read_to_end(&mut bytes) {
                    Ok(_) => Preloaded::Read(bytes, None),
                    Err(error) => Preloaded::Read(bytes, Some(io_message(&error))),
                }
            };
            let preloaded = if !program.reads_input() {
                Vec::new()
            } else if options.files.is_empty() {
                vec![read_stdin()]
            } else {
                options
                    .files
                    .iter()
                    .map(|file| {
                        if file == "-" {
                            read_stdin()
                        } else {
                            preload(&resolve(file))
                        }
                    })
                    .collect()
            };
            run_preloaded(&program, &options.files, preloaded)
        }
    };
    for (stream, bytes) in outcome.writes() {
        match stream {
            Stream::Out => out.write_all(bytes)?,
            Stream::Err => err.write_all(bytes)?,
        }
    }
    Ok(outcome.code)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jq(args: &[&str], stdin: &str) -> (i32, String, String) {
        let argv: Vec<String> = std::iter::once("jq")
            .chain(args.iter().copied())
            .map(str::to_owned)
            .collect();
        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = run_buffered(
            &argv,
            vec![("HOME".into(), "/home/agent".into())],
            &|path| PathBuf::from(path),
            &mut stdin.as_bytes(),
            &mut out,
            &mut err,
        )
        .unwrap();
        (
            code,
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
        )
    }

    #[test]
    fn output_modes_match_jq() {
        assert_eq!(
            jq(&["."], "{\"a\":[1,2]}").1,
            "{\n  \"a\": [\n    1,\n    2\n  ]\n}\n"
        );
        assert_eq!(jq(&["-c", "."], "{\"a\":[1,2]}").1, "{\"a\":[1,2]}\n");
        assert_eq!(jq(&["-r", ".[]"], "[\"x\",1]").1, "x\n1\n");
        assert_eq!(jq(&["-j", ".[]"], "[\"x\",1]").1, "x1");
        assert_eq!(
            jq(&["-S", "-c", "."], "{\"b\":1,\"a\":2}").1,
            "{\"a\":2,\"b\":1}\n"
        );
        assert_eq!(jq(&["--tab", "."], "{\"a\":1}").1, "{\n\t\"a\": 1\n}\n");
        assert_eq!(jq(&["-a", "-n", "\"é\""], "").1, "\"\\u00e9\"\n");
        assert_eq!(jq(&["--seq", "-n", "1"], "").1, "\u{1e}1\n");
        assert_eq!(jq(&["--raw-output0", "-n", "\"a\""], "").1, "a\0");
    }

    #[test]
    // input that isn't valid UTF-8 must come out the way real jq's own JSON reader
    // already rewrote it going in — one Unicode replacement character (U+FFFD) per invalid
    // byte — not the original raw byte passed straight through unchanged, which is jaq's own
    // (documented, intentional) choice, not jq's. Verified against the oracle for both `-r`
    // and the default (JSON-quoted) output.
    fn invalid_utf8_input_becomes_the_replacement_character() {
        // `\xff` is never valid UTF-8 on its own; built at runtime so rustc's
        // invalid-literal lint doesn't refuse a `b"...\xff..."` byte-string literal.
        let mut input = b"\"a".to_vec();
        input.push(0xff);
        input.extend_from_slice(b"b\"\n");
        let input = unsafe { core::str::from_utf8_unchecked(&input) };

        let (code, out, _) = jq(&["-r", "."], input);
        assert_eq!((code, out.as_str()), (0, "a\u{fffd}b\n"));
        let (code, out, _) = jq(&["-c", "."], input);
        assert_eq!((code, out.as_str()), (0, "\"a\u{fffd}b\"\n"));
    }

    #[test]
    fn an_uncaught_error_ends_that_inputs_outputs() {
        let (code, out, err) = jq(&["-n", "1, error(\"x\"), 2"], "");
        assert_eq!(
            (code, out.as_str(), err.as_str()),
            (5, "1\n", "jq: error (at <unknown>): x\n")
        );
        // The next input still runs, and the status is the last input's.
        let (code, out, err) = jq(&["if . == 2 then error(\"two\") else . end, 10"], "1 2 3");
        assert_eq!((code, out.as_str()), (0, "1\n10\n3\n10\n"));
        assert_eq!(err.matches("jq: error").count(), 1, "{err}");
    }

    #[test]
    fn a_failed_json_parse_errors_once() {
        for filter in ["\"x\" | tonumber", "\"x\" | fromjson"] {
            let (code, out, err) = jq(&["-n", filter], "");
            assert_eq!((code, out.as_str()), (5, ""), "{filter}");
            assert_eq!(err.matches("jq: error").count(), 1, "{filter}: {err}");
        }
        let (code, out, _) = jq(
            &[
                "-n",
                "-c",
                "[\"x\" | fromjson?], (try (\"x\" | tonumber) catch \"caught\")",
            ],
            "",
        );
        assert_eq!((code, out.as_str()), (0, "[]\n\"caught\"\n"));
    }

    #[test]
    fn inputs_slurp_and_raw_input() {
        assert_eq!(jq(&["-c", "-n", "[inputs]"], "1 2 3").1, "[1,2,3]\n");
        assert_eq!(jq(&["-c", "[., input]"], "1 2 3 4").1, "[1,2]\n[3,4]\n");
        assert_eq!(jq(&["-c", "-s", "."], "1 2").1, "[1,2]\n");
        assert_eq!(jq(&["-R", "."], "a\nb\n").1, "\"a\"\n\"b\"\n");
        assert_eq!(jq(&["-R", "-s", "."], "a\nb\n").1, "\"a\\nb\\n\"\n");
        assert_eq!(jq(&["-c", "-n", "[inputs]"], "").1, "[]\n");
    }

    #[test]
    fn variables_env_and_args() {
        let (_, out, _) = jq(
            &[
                "-c",
                "-n",
                "--arg",
                "x",
                "1",
                "--argjson",
                "y",
                "{\"z\":2}",
                "[$x,$y,$ARGS.named]",
            ],
            "",
        );
        assert_eq!(out, "[\"1\",{\"z\":2},{\"x\":\"1\",\"y\":{\"z\":2}}]\n");
        assert_eq!(
            jq(&["-n", "-r", "$ENV.HOME, env.HOME"], "").1,
            "/home/agent\n/home/agent\n"
        );
        assert_eq!(
            jq(&["-c", "-n", "$ARGS", "--args", "a", "b"], "").1,
            "{\"positional\":[\"a\",\"b\"],\"named\":{}}\n"
        );
        assert_eq!(
            jq(
                &["-c", "-n", "$ARGS.positional", "--jsonargs", "1", "{}"],
                ""
            )
            .1,
            "[1,{}]\n"
        );
    }

    #[test]
    fn exit_statuses_match_jq() {
        assert_eq!(jq(&["-e", "-n", "null"], "").0, 1);
        assert_eq!(jq(&["-e", "-n", "empty"], "").0, 4);
        assert_eq!(jq(&["-e", "-n", "1"], "").0, 0);
        assert_eq!(jq(&["-n", "halt"], "").0, 0);
        let (code, out, err) = jq(&["-n", "\"x\" | halt_error"], "");
        assert_eq!((code, out.as_str(), err.as_str()), (5, "", "x"));
        let (code, _, err) = jq(&["-n", "{a:1} | halt_error(3)"], "");
        assert_eq!((code, err.as_str()), (3, "{\"a\":1}\n"));
        // Like jq, only the last input's run decides the status.
        let (code, out, err) = jq(&[".a"], "1 {\"a\":2}");
        assert_eq!((code, out.as_str()), (0, "2\n"));
        assert_eq!(jq(&[".a"], "{\"a\":2} 1").0, 5);
        assert!(err.starts_with("jq: error (at <stdin>:0): "), "{err}");
        assert_eq!(
            jq(&["-n", "error(\"boom\")"], "").2,
            "jq: error (at <unknown>): boom\n"
        );
        assert_eq!(jq(&["--bogus"], "").0, 2);
        assert_eq!(jq(&["-n", ".["], "").0, 3);
        let (code, _, err) = jq(&["-n", "foo"], "");
        assert_eq!(code, 3);
        assert_eq!(
            err,
            "jq: error: foo/0 is not defined at <top-level>, line 1, column 1:\n    foo\n    ^^^\njq: 1 compile error\n"
        );
        assert_eq!(jq(&[".", "/nonexistent/file"], "").0, 2);
    }

    #[test]
    fn jq_definitions_parse() {
        let defs = load::parse(JQ_DEFS, |parser| parser.defs());
        assert!(defs.is_some_and(|defs| defs.len() == 8));
    }

    #[test]
    fn debug_and_stderr_reach_the_command_stderr() {
        let (code, out, err) = jq(&["-n", "1 | debug | . + 1"], "");
        assert_eq!(
            (code, out.as_str(), err.as_str()),
            (0, "2\n", "[\"DEBUG:\",1]\n")
        );
        assert_eq!(jq(&["-n", "\"s\" | stderr | empty"], "").2, "s");
    }

    #[test]
    fn arguments_that_look_like_numbers_are_not_options() {
        let (code, out, _) = jq(&["-cn", "$ARGS.positional", "--jsonargs", "-5", "-1e3"], "");
        assert_eq!((code, out.as_str()), (0, "[-5,-1E+3]\n"));
        let (code, out, _) = jq(&["-cn", "$ARGS.positional", "--args", "-5", "-.5"], "");
        assert_eq!((code, out.as_str()), (0, "[\"-5\",\"-.5\"]\n"));
        assert_eq!(jq(&["-n", "-0 | tostring"], "").1, "\"0\"\n");
    }

    #[test]
    fn parse_errors_are_jqs() {
        let (code, out, err) = jq(&["-c", "."], "1 2 x\n");
        assert_eq!((code, out.as_str()), (5, "1\n2\n"));
        assert_eq!(
            err,
            "jq: parse error: Invalid numeric literal at line 2, column 0\n"
        );
        // `-s` prints nothing before the error
        let (code, out, _) = jq(&["-s", "."], "1 2 x\n");
        assert_eq!((code, out.as_str()), (5, ""));
        let (_, out, err) = jq(&["-c", "."], "\u{feff}{\"a\":1}");
        assert_eq!((out.as_str(), err.as_str()), ("{\"a\":1}\n", ""));
    }

    #[test]
    fn errors_and_input_line_number_carry_jqs_positions() {
        let (_, _, err) = jq(&[".a"], "1\n2\n");
        assert_eq!(
            err,
            "jq: error (at <stdin>:1): Cannot index number with string (\"a\")\n\
             jq: error (at <stdin>:2): Cannot index number with string (\"a\")\n"
        );
        assert_eq!(
            jq(&["-c", "[., input_line_number]"], "[1,\n2]\n3\n\n4").1,
            "[[1,2],2]\n[3,3]\n[4,4]\n"
        );
        let (code, _, err) = jq(&["-n", "input"], "");
        assert_eq!(
            (code, err.as_str()),
            (5, "jq: error (at <stdin>:0): break\n")
        );
    }

    #[test]
    fn a_long_line_is_read_in_jqs_chunks() {
        // Line numbers count chunks that end a line; a value inside the first 4091 bytes of a
        // long line is read before the chunk holding its newline.
        let long = format!("1 {}\n", "2 ".repeat(3000));
        let out = jq(&["-c", "[., input_line_number]"], &long).1;
        assert!(out.starts_with("[1,0]\n"), "{out}");
        assert!(out.ends_with("[2,1]\n"), "{out}");
    }
}
