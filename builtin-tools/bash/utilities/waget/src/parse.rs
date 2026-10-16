//! `wget` argument parsing.
//!
//! Supports a practical subset of wget: output selection (`-O`), quiet (`-q`), redirect following
//! (on by default, capped by `--max-redirect`), response-header printing (`-S`), timeout (`-T`),
//! retries (`-t`), POST (`--post-data`/`--post-file`), extra headers (`--header`), user-agent
//! (`-U`), `--content-disposition` naming, resuming a partial download (`-c`), a directory prefix
//! (`-P`), leaving existing files alone (`-nc`), checking without saving (`--spider`), URLs from a
//! file (`-i`), several URLs, `-V`, and `--flag=value`. `-N` is accepted (see `Request::timestamping` doc) but is a
//! documented no-op — this crate has no local mtime/`If-Modified-Since` comparison to drive it. A
//! few other no-op-safe flags (`--no-check-certificate`, `-nv`, `-v`) are accepted so common
//! command lines don't error.

use http::Method;

/// Where a fetched body is written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Output {
    /// Stream to stdout (`-O -`).
    Stdout,
    /// Write to this path (explicit `-O <file>`, or the default basename).
    File(String),
}

/// A parsed `wget` invocation.
#[derive(Clone, Debug, PartialEq)]
// independent wget flag toggles, not a state enum
#[allow(clippy::struct_excessive_bools)]
pub(crate) struct Request {
    /// The first URL given (what a single fetch retrieves).
    pub url: String,
    /// Every URL given on the command line, in order; `-i`'s file adds more when it is read.
    pub urls: Vec<String>,
    /// `-i`/`--input-file`: a file of URLs, one per line, fetched after the command line's.
    pub input_file: Option<String>,
    pub output: Output,
    /// True when `output` is the URL-derived default (no explicit `-O`) — the only case
    /// `--content-disposition` may rename.
    pub output_is_default: bool,
    /// `-q`/`--quiet`: suppress the progress/saved/status messages on stderr.
    pub quiet: bool,
    pub method: Method,
    pub headers: Vec<(String, String)>,
    /// `--post-data` body.
    pub data: Option<Vec<u8>>,
    /// `--post-file`: the body's file, read when the request is made, as wget reads it.
    pub post_file: Option<String>,
    /// Follow 3xx redirects (wget does by default), capped by `max_redirect`.
    pub follow_redirects: bool,
    pub max_redirect: u32,
    /// `-S`/`--server-response`: print the response status line + headers to stderr.
    pub server_response: bool,
    /// `-T`/`--timeout` seconds: applied to both connect and read.
    pub timeout: Option<f64>,
    /// `-t`/`--tries`: total attempts on transport failure (GNU default 20; `0` or `inf` means
    /// retry without limit, represented here as [`u32::MAX`] since there is no literal infinity).
    pub tries: u32,
    /// `--content-disposition`: name the default output from a `Content-Disposition` header.
    pub content_disposition: bool,
    /// `-c`/`--continue`: resume a partial download with a `Range` request (200 restarts).
    pub continue_download: bool,
    /// `-P`/`--directory-prefix`: prepend this directory to the output path (created when a
    /// file is saved there).
    pub directory_prefix: Option<String>,
    /// `-N`/`--timestamping`: accepted, but a documented no-op — see the module doc.
    pub timestamping: bool,
    /// `-nc`/`--no-clobber`: leave a file that is already there alone, and fetch nothing for it.
    pub no_clobber: bool,
    /// `--spider`: check that each URL is there; save nothing.
    pub spider: bool,
}

/// A `wget` argument parsing error.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParseError {
    /// No URL argument was given.
    MissingUrl,
    /// A flag that requires a value was given none (holds the flag name).
    MissingValue(String),
    /// An unrecognized option was encountered (holds the offending flag).
    UnknownFlag(String),
    /// A negative count for an option wget reads a number for (holds the option's long name and
    /// the value as given).
    InvalidNumber(String, String),
    /// A `-T` value that is not a time period as wget reads one (holds the value as given).
    InvalidPeriod(String),
    /// A negative `-T` value (holds the value as given).
    NegativePeriod(String),
    /// Both `--post-data` and `--post-file`.
    PostDataAndFile,
    /// `-i -`: URLs from standard input, which this crate does not read.
    InputFromStdin,
    /// `-V`/`--version`: print the version and stop.
    Version,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            // As GNU wget words them (getopt's wording for options).
            ParseError::MissingUrl => write!(f, "missing URL"),
            ParseError::MissingValue(flag) => match flag.strip_prefix("--") {
                Some(_) => write!(f, "option '{flag}' requires an argument"),
                None => write!(
                    f,
                    "option requires an argument -- '{}'",
                    flag.trim_start_matches('-')
                ),
            },
            ParseError::UnknownFlag(flag) => match flag.strip_prefix("--") {
                Some(_) => write!(f, "unrecognized option '{flag}'"),
                None => write!(f, "invalid option -- '{}'", flag.trim_start_matches('-')),
            },
            ParseError::InvalidNumber(option, v) => write!(f, "{option}: Invalid number '{v}'."),
            ParseError::InvalidPeriod(v) => write!(f, "--timeout: Invalid time period '{v}'"),
            ParseError::NegativePeriod(v) => write!(f, "--timeout: Negative time period '{v}'"),
            ParseError::PostDataAndFile => {
                write!(f, "You cannot specify both --post-data and --post-file.")
            }
            ParseError::InputFromStdin => write!(
                f,
                "-i - (URLs from standard input) is unsupported in bash-tool; name a file"
            ),
            ParseError::Version => Ok(()),
        }
    }
}

/// Expand `--flag=value` into two tokens. (Unlike curl, wget short flags are matched whole — no
/// clustering — so `-nv`/`-nc` stay single options.)
fn expand_args(args: &[String]) -> Vec<String> {
    let mut out = Vec::with_capacity(args.len());
    for arg in args {
        match arg.strip_prefix("--").and_then(|l| l.split_once('=')) {
            Some((flag, value)) => {
                out.push(format!("--{flag}"));
                out.push(value.to_string());
            }
            None => out.push(arg.clone()),
        }
    }
    out
}

/// Parse argv (without the leading `wget` word) into a [`Request`].
#[cfg(test)]
pub(crate) fn parse(args: &[String]) -> Result<Request, ParseError> {
    parse_at(args)
}

pub(crate) fn parse_at(args: &[String]) -> Result<Request, ParseError> {
    let expanded = expand_args(args);

    let mut urls: Vec<String> = Vec::new();
    let mut input_file = None;
    let mut explicit_output: Option<Output> = None;
    let mut quiet = false;
    let mut headers = Vec::new();
    let mut data: Option<Vec<u8>> = None;
    let mut post_file = None;
    let mut server_response = false;
    let mut timeout = None;
    let mut tries = 20u32;
    let mut max_redirect = 20u32;
    let mut content_disposition = false;
    let mut continue_download = false;
    let mut directory_prefix = None;
    let mut timestamping = false;
    let mut no_clobber = false;
    let mut spider = false;

    let mut iter = expanded.into_iter();
    while let Some(arg) = iter.next() {
        let mut next = |flag: &str| {
            iter.next()
                .ok_or_else(|| ParseError::MissingValue(flag.into()))
        };
        match arg.as_str() {
            "-O" | "--output-document" => {
                let target = next("-O")?;
                explicit_output = Some(if target == "-" {
                    Output::Stdout
                } else {
                    Output::File(target)
                });
            }
            "-q" | "--quiet" => quiet = true,
            "-S" | "--server-response" => server_response = true,
            "-T" | "--timeout" => timeout = Some(parse_period(&next("-T")?)?),
            "-t" | "--tries" => {
                // `0` or `inf` means "retry without limit" in GNU wget, not "give up after zero
                // tries".
                let value = next("-t")?;
                tries = if value.eq_ignore_ascii_case("inf") {
                    u32::MAX
                } else {
                    match parse_count("--tries", &value)? {
                        0 => u32::MAX,
                        n => n,
                    }
                };
            }
            "--max-redirect" => {
                max_redirect = parse_count("--max-redirect", &next("--max-redirect")?)?;
            }
            "--content-disposition" => content_disposition = true,
            "-c" | "--continue" => continue_download = true,
            "-P" | "--directory-prefix" => directory_prefix = Some(next("-P")?),
            "-N" | "--timestamping" => timestamping = true,
            "-nc" | "--no-clobber" => no_clobber = true,
            "--spider" => spider = true,
            "-i" | "--input-file" => {
                let file = next("-i")?;
                if file == "-" {
                    return Err(ParseError::InputFromStdin);
                }
                input_file = Some(file);
            }
            "-V" | "--version" => return Err(ParseError::Version),
            "-U" | "--user-agent" => headers.push(("User-Agent".to_string(), next("-U")?)),
            "--header" => headers.push(split_header(&next("--header")?)),
            "--post-data" => data = Some(next("--post-data")?.into_bytes()),
            "--post-file" => post_file = Some(next("--post-file")?),
            // Accepted no-ops so common command lines don't error. `--no-check-certificate` does NOT
            // actually disable verification (WASI-HTTP doesn't expose that) — it is honored as "don't
            // fail parsing", the safer interpretation.
            "--no-check-certificate" | "-nv" | "--no-verbose" | "-v" | "--verbose" => {}
            other if other.starts_with('-') && other != "-" => {
                return Err(ParseError::UnknownFlag(other.to_string()));
            }
            _ => urls.push(arg),
        }
    }

    if data.is_some() && post_file.is_some() {
        return Err(ParseError::PostDataAndFile);
    }
    if urls.is_empty() && input_file.is_none() {
        return Err(ParseError::MissingUrl);
    }
    let url = urls.first().cloned().unwrap_or_default();
    let (output, output_is_default) = match explicit_output {
        Some(o) => (o, false),
        None => (Output::File(default_filename(&url)), true),
    };
    let method = if data.is_some() || post_file.is_some() {
        Method::POST
    } else {
        Method::GET
    };

    Ok(Request {
        url,
        urls,
        input_file,
        output,
        output_is_default,
        quiet,
        method,
        headers,
        data,
        post_file,
        follow_redirects: true,
        max_redirect,
        server_response,
        timeout,
        tries,
        content_disposition,
        continue_download,
        directory_prefix,
        timestamping,
        no_clobber,
        spider,
    })
}

impl Request {
    /// This request for `url`: its default output named after that URL, unless `-O` named one.
    pub(crate) fn for_url(&self, url: &str) -> Request {
        Request {
            url: url.to_owned(),
            output: if self.output_is_default {
                Output::File(default_filename(url))
            } else {
                self.output.clone()
            },
            ..self.clone()
        }
    }
}

/// A time period in seconds as wget reads one (`cmd_time` and `simple_atof` in wget's `init.c`):
/// trailing whitespace dropped, an optional unit (`s`, `m`, `h`, `d` or `w`, either case), then
/// digits with at most one `.`, optionally signed and surrounded by whitespace. No exponents.
fn parse_period(value: &str) -> Result<f64, ParseError> {
    let invalid = || ParseError::InvalidPeriod(value.to_string());
    let is_space = |c: char| matches!(c, ' ' | '\t' | '\n' | '\x0b' | '\x0c' | '\r');
    let trimmed = value.trim_end_matches(is_space);
    let unit = match trimmed.chars().last().map(|c| c.to_ascii_lowercase()) {
        Some('s') => Some(1.0),
        Some('m') => Some(60.0),
        Some('h') => Some(3_600.0),
        Some('d') => Some(86_400.0),
        Some('w') => Some(604_800.0),
        _ => None,
    };
    // The unit letters are ASCII, so dropping one byte drops the whole unit.
    let number = match unit {
        Some(_) => &trimmed[..trimmed.len() - 1],
        None => trimmed,
    };
    let number = number.trim_matches(is_space);
    let (negative, digits) = match number.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, number.strip_prefix('+').unwrap_or(number)),
    };
    if digits.matches('.').count() > 1
        || !digits.bytes().all(|b| b.is_ascii_digit() || b == b'.')
        || !digits.bytes().any(|b| b.is_ascii_digit())
    {
        return Err(invalid());
    }
    let magnitude = digits.parse::<f64>().map_err(|_| invalid())?;
    if negative && magnitude > 0.0 {
        return Err(ParseError::NegativePeriod(value.to_string()));
    }
    Ok(magnitude * unit.unwrap_or(1.0))
}

/// A count as wget reads one (`cmd_number`): `atoi`'s, so leading blanks and a sign, then the
/// digits there are, and anything after them ignored (`3x` is 3, `x` is 0); only a negative count
/// is refused. `option` is the long name wget's message gives it.
fn parse_count(option: &str, value: &str) -> Result<u32, ParseError> {
    let trimmed = value.trim_start_matches([' ', '\t', '\n', '\x0b', '\x0c', '\r']);
    let (negative, unsigned) = match trimmed.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, trimmed.strip_prefix('+').unwrap_or(trimmed)),
    };
    let digits = &unsigned[..unsigned.bytes().take_while(u8::is_ascii_digit).count()];
    let count = digits.bytes().fold(0u32, |count, digit| {
        count
            .saturating_mul(10)
            .saturating_add(u32::from(digit - b'0'))
    });
    if negative && count > 0 {
        return Err(ParseError::InvalidNumber(
            option.to_owned(),
            value.to_owned(),
        ));
    }
    Ok(count)
}

/// Split a `--header "Key: Value"` argument into `(key, value)`, trimming whitespace.
fn split_header(raw: &str) -> (String, String) {
    match raw.split_once(':') {
        Some((k, v)) => (k.trim().to_string(), v.trim().to_string()),
        None => (raw.trim().to_string(), String::new()),
    }
}

/// The default output filename for a URL: its last non-empty path segment (stripped of any query),
/// or `index.html` if there is none.
pub(crate) fn default_filename(url: &str) -> String {
    let without_scheme = url.split("://").nth(1).unwrap_or(url);
    let location = without_scheme.split(['?', '#']).next().unwrap_or("");
    // What precedes the first `/` is the host, never a file name; a path ending in `/` names a
    // directory, whose file wget calls `index.html`.
    let path = location.split_once('/').map_or("", |(_, path)| path);
    match path.rsplit('/').next() {
        Some(name) if !name.is_empty() => name.to_string(),
        _ => "index.html".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(std::string::ToString::to_string).collect()
    }

    #[test]
    fn timeout_follows_wgets_time_period_grammar() {
        // Checked against GNU wget in the conformance oracle.
        let accepted = [
            ("3", 3.0),
            ("2.5", 2.5),
            (".5", 0.5),
            ("5.", 5.0),
            ("+3", 3.0),
            ("-0", 0.0),
            (" 3", 3.0),
            ("3 ", 3.0),
            ("3s", 3.0),
            ("5m", 300.0),
            ("5M", 300.0),
            ("3 m", 180.0),
            ("1.5h", 5400.0),
            ("2d", 172_800.0),
            ("1w", 604_800.0),
        ];
        for flag in ["-T", "--timeout"] {
            for (value, seconds) in accepted {
                let r = parse(&argv(&[flag, value, "http://x"])).unwrap();
                assert_eq!(r.timeout, Some(seconds), "{flag} {value:?}");
            }
            for value in ["1e3", "nan", "inf", "1e300", "abc", "3x", ""] {
                let err = parse(&argv(&[flag, value, "http://x"])).unwrap_err();
                assert_eq!(
                    err.to_string(),
                    format!("--timeout: Invalid time period '{value}'")
                );
            }
            let err = parse(&argv(&[flag, "-1", "http://x"])).unwrap_err();
            assert_eq!(err.to_string(), "--timeout: Negative time period '-1'");
        }
    }

    #[test]
    fn plain_url_defaults_to_basename_file() {
        let r = parse(&argv(&["https://example.com/dir/report.txt"])).unwrap();
        assert_eq!(r.output, Output::File("report.txt".to_string()));
        assert!(r.output_is_default);
        assert!(r.follow_redirects, "wget follows redirects by default");
        assert_eq!(r.method, Method::GET);
        assert_eq!(r.tries, 20, "GNU wget's own default is 20, not 1");
    }

    #[test]
    fn tries_zero_means_unlimited() {
        let r = parse(&argv(&["-t", "0", "https://example.com"])).unwrap();
        assert_eq!(r.tries, u32::MAX);
    }

    #[test]
    fn a_url_with_no_file_name_saves_to_index_html() {
        for url in [
            "http://127.0.0.1:1/",
            "http://host",
            "http://host/?q=1",
            "http://h/d/",
        ] {
            assert_eq!(default_filename(url), "index.html", "{url}");
        }
        assert_eq!(default_filename("http://host/dir/f.txt?x"), "f.txt");
    }

    #[test]
    fn dash_o_is_explicit_not_default() {
        let r = parse(&argv(&["-O", "out.bin", "https://example.com/f"])).unwrap();
        assert_eq!(r.output, Output::File("out.bin".to_string()));
        assert!(!r.output_is_default);
    }

    #[test]
    fn post_data_sets_post_and_body() {
        let r = parse(&argv(&["--post-data", "a=1&b=2", "https://example.com"])).unwrap();
        assert_eq!(r.method, Method::POST);
        assert_eq!(r.data.as_deref(), Some(b"a=1&b=2".as_slice()));
    }

    #[test]
    fn post_file_is_a_post_whose_body_is_read_when_sent() {
        let r = parse(&argv(&["--post-file", "body.txt", "https://x"])).unwrap();
        assert_eq!(r.method, Method::POST);
        assert_eq!(r.post_file.as_deref(), Some("body.txt"));
        assert_eq!(
            parse(&argv(&[
                "--post-data",
                "a",
                "--post-file",
                "f",
                "https://x"
            ])),
            Err(ParseError::PostDataAndFile)
        );
    }

    #[test]
    fn counts_are_read_as_atoi_reads_them() {
        // Checked against GNU wget in the conformance oracle: `x` and `` are 0 (unlimited tries),
        // `3x` is 3, and only a negative count is refused.
        for (value, tries) in [("3x", 3), ("x", u32::MAX), ("", u32::MAX), ("-x", u32::MAX)] {
            assert_eq!(
                parse(&argv(&["-t", value, "http://x"])).unwrap().tries,
                tries
            );
        }
        assert_eq!(
            parse(&argv(&["--max-redirect=x", "http://x"]))
                .unwrap()
                .max_redirect,
            0
        );
        assert_eq!(
            parse(&argv(&["-t", "-2", "http://x"]))
                .unwrap_err()
                .to_string(),
            "--tries: Invalid number '-2'."
        );
    }

    #[test]
    fn header_and_user_agent() {
        let r = parse(&argv(&[
            "--header=Accept: text/plain",
            "-U",
            "bash-tool/1",
            "https://x",
        ]))
        .unwrap();
        assert!(
            r.headers
                .iter()
                .any(|(k, v)| k == "Accept" && v == "text/plain")
        );
        assert!(
            r.headers
                .iter()
                .any(|(k, v)| k == "User-Agent" && v == "bash-tool/1")
        );
    }

    #[test]
    fn timeout_tries_and_max_redirect() {
        let r = parse(&argv(&[
            "-T",
            "3",
            "-t",
            "5",
            "--max-redirect",
            "2",
            "https://x",
        ]))
        .unwrap();
        assert_eq!(r.timeout, Some(3.0));
        assert_eq!(r.tries, 5);
        assert_eq!(r.max_redirect, 2);
    }

    #[test]
    fn server_response_and_content_disposition_flags() {
        let r = parse(&argv(&["-S", "--content-disposition", "https://x/f"])).unwrap();
        assert!(r.server_response);
        assert!(r.content_disposition);
    }

    #[test]
    fn accepted_no_ops_stay_no_ops() {
        // Audit P3-6: assert the accepted no-op flags didn't error AND didn't flip real state — a
        // regression that made `-nv`/`--no-check-certificate` consume the URL or set quiet/
        // server_response would still have passed the old `.is_ok()`-only check.
        let r = parse(&argv(&["--no-check-certificate", "-nv", "https://x/f"])).unwrap();
        assert_eq!(r.url, "https://x/f");
        assert!(!r.quiet, "-nv must not set quiet (that is -q)");
        assert!(
            !r.server_response,
            "the no-ops must not set server_response (that is -S)"
        );
    }

    #[test]
    fn continue_directory_prefix_and_timestamping_flags() {
        let r = parse(&argv(&["-c", "-P", "downloads", "-N", "https://x/f"])).unwrap();
        assert!(r.continue_download);
        assert_eq!(r.directory_prefix.as_deref(), Some("downloads"));
        assert!(r.timestamping);
    }

    #[test]
    fn unknown_flag_errors() {
        assert_eq!(
            parse(&argv(&["--bogus", "https://example.com"])),
            Err(ParseError::UnknownFlag("--bogus".into()))
        );
    }

    #[test]
    fn missing_url_errors() {
        assert_eq!(parse(&argv(&["-q"])), Err(ParseError::MissingUrl));
    }
}
