//! `curl` argument parsing.
//!
//! Supports a practical subset of curl: method/data/headers, redirect following (`-L`), response-
//! header output (`-i`/`-I`), fail-on-error (`-f`, `--fail-with-body`), timeouts (`-m`/
//! `--connect-timeout`), write-out (`-w`), auth and identity (`-u`/`-A`/`-e`), data-from-file
//! (`-d @file`, `--data-binary`), `--data-urlencode`, `--json`, query-from-data (`-G`), verbose
//! (`-v`), multipart forms (`-F`), remote-naming (`-O`/`-J`, `--create-dirs`), retries (`--retry`/
//! `--retry-delay`), cookies (`-b`/`-c`), file upload (`-T`), byte ranges (`-r`), transparent
//! compression (`--compressed`), config files (`-K`), several URLs, and `-V`. Short flags cluster
//! (`-fsSL`) and long flags take `--flag=value`. Unknown flags are an error so a typo isn't
//! swallowed as a URL.

use http::Method;

/// One `-F` field, in the order given on the command line — `multipart/form-data` preserves field
/// order, and curl sends fields in the order `-F` was repeated.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum FormField {
    /// `-F name=value`.
    Text { name: String, value: String },
    /// `-F name=@path[;type=CONTENT_TYPE]`. The part's filename is `path`'s basename; curl's
    /// further `;filename=` override is not supported.
    File {
        name: String,
        path: String,
        content_type: Option<String>,
    },
}

/// A parsed `curl` invocation. (No `Eq`: the timeout fields are `f64`.)
#[derive(Clone, Debug, PartialEq)]
// Each bool is a distinct curl flag; folding them into an enum would obscure the 1:1 flag mapping.
#[allow(clippy::struct_excessive_bools)]
pub(crate) struct Request {
    /// The URL this transfer fetches: the first given, or (in one of [`Request::transfers`])
    /// that transfer's own.
    pub url: String,
    /// Every URL given, in order: curl fetches each in turn.
    pub urls: Vec<String>,
    /// Every `-o FILE` and `-O`, in order: the n-th is the n-th URL's.
    pub outputs: Vec<Destination>,
    pub method: Method,
    pub headers: Vec<(String, String)>,
    /// Request body (`-d`, `--data-binary`, `--json`), already resolved from `@file` if used.
    pub data: Option<Vec<u8>>,
    /// `-o <file>`: write the body here instead of stdout.
    pub output: Option<String>,
    /// `-s`/`--silent`: suppress the non-2xx status message and error messages on stderr.
    pub silent: bool,
    /// `-S`/`--show-error`: show error messages even with `-s`.
    pub show_error: bool,
    /// `-L`/`--location`: follow 3xx redirects.
    pub location: bool,
    /// `-i`/`--include`: prepend the response status line + headers to the output.
    pub include: bool,
    /// `-I`/`--head`: issue a HEAD request and print only the response headers.
    pub head: bool,
    /// `-f`/`--fail`: emit no body and exit non-zero on an HTTP error status (>= 400).
    pub fail: bool,
    /// `-m`/`--max-time` seconds: overall request time budget.
    pub max_time: Option<f64>,
    /// `--connect-timeout` seconds: connection time budget.
    pub connect_timeout: Option<f64>,
    /// `-w`/`--write-out` format string, expanded after the transfer.
    pub write_out: Option<String>,
    /// `-v`/`--verbose`: trace request/response headers to stderr.
    pub verbose: bool,
    /// `-G`/`--get`: send any `-d` data as the query string of a GET.
    pub get: bool,
    /// `-F`/`--form`: multipart/form-data fields. Non-empty implies `multipart/form-data` and (if
    /// `data` is unset) `POST`.
    pub forms: Vec<FormField>,
    /// `-O`/`--remote-name`: save to a file named after the URL's last path segment.
    pub remote_name: bool,
    /// `-J`/`--remote-header-name`: with `-O`, prefer a `Content-Disposition` filename.
    pub remote_header_name: bool,
    /// `--create-dirs`: create `-o`'s leading directories if they don't exist.
    pub create_dirs: bool,
    /// `--retry N`: additional attempts (beyond the first) on a transport failure or a retryable
    /// HTTP status (408, 429, 500, 502, 503, 504) — curl's own retryable set.
    pub retry: u32,
    /// `--retry-delay SECONDS`: fixed delay between retries (0 = none, curl's default).
    pub retry_delay: f64,
    /// `-b`/`--cookie`: a `name=value` string, or (if it contains neither `=` nor is a bare
    /// token typical of one — see `resolve_cookie_opt`) a Netscape-format cookie-jar file to load.
    pub cookie: Option<String>,
    /// `-c`/`--cookie-jar`: write the accumulated cookie jar (Netscape format) here when the
    /// transfer completes.
    pub cookie_jar: Option<String>,
    /// `-T`/`--upload-file`: PUT this file's contents as the body (unless `-X` overrides the
    /// method). A URL ending in `/` gets the file's basename appended, as curl does.
    pub upload_file: Option<String>,
    /// `--compressed`: ask for and transparently decode `gzip`/`deflate`.
    pub compressed: bool,
    /// `-k`/`--insecure`: skip certificate checks, which WASI-HTTP cannot; a certificate it
    /// refuses says so.
    pub insecure: bool,
    /// `--fail-with-body`: fail as `-f` does on an HTTP error, but still write the body.
    pub fail_with_body: bool,
}

/// Where a URL's body goes, from `-o FILE` or `-O`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Destination {
    /// `-o FILE`.
    File(String),
    /// `-O`: a file named after the URL.
    Remote,
}

/// A `curl` argument parsing error.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParseError {
    /// `curl` alone: curl says only where to read more.
    NoArguments,
    /// Options, but no URL.
    MissingUrl,
    /// A flag that requires a value was given none; holds the flag as typed.
    MissingValue(String),
    /// An unrecognized flag; holds the offending token.
    UnknownFlag(String),
    /// An empty argument where curl needs content: a URL (flag `""`) or an option's value; holds
    /// the flag as typed.
    Blank(String),
    /// A value curl cannot use for its option (`-F novalue`, a `-K` inside a config file); holds
    /// the flag as typed.
    BadlyUsed(String),
    /// The `-X` value was not a valid HTTP method; holds the given value.
    BadMethod(String),
    /// A numeric value curl would not accept; holds the flag as typed.
    BadNumber(String),
    /// A negative count; holds the flag as typed.
    Negative(String),
    /// A seconds value whose fractional digits overflow curl's 64-bit reader; holds the flag.
    TooLarge(String),
    /// A file an option names (`-d @file`, `--data-urlencode name@file`, `-K file`) could not be
    /// read: curl's exit 26. Holds the flag as typed.
    ReadFile(String),
    /// `-d @-`, which needs a standard input this parser does not have; holds the message.
    BadData(String),
    /// A `-K` file named an option curl does not know; holds the flag as typed.
    UnknownConfigOption(String),
    /// `-V`/`--version`: print the version and stop, as curl does when it meets the option.
    Version,
    /// An error curl reports after warnings it printed on the way (each a whole line, `-s` having
    /// already dropped those it hides).
    Warned(String, Box<ParseError>),
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseError::NoArguments | ParseError::Version => Ok(()),
            ParseError::MissingUrl => write!(f, "(2) no URL specified"),
            ParseError::MissingValue(flag) => write!(f, "option {flag}: requires parameter"),
            ParseError::UnknownFlag(flag) => write!(f, "option {flag}: is unknown"),
            ParseError::Blank(flag) => {
                write!(f, "option {flag}: blank argument where content is expected")
            }
            ParseError::BadlyUsed(flag) => write!(f, "option {flag}: is badly used here"),
            ParseError::BadMethod(m) => write!(f, "invalid method: {m}"),
            ParseError::BadNumber(flag) => {
                write!(f, "option {flag}: expected a proper numerical parameter")
            }
            ParseError::Negative(flag) => {
                write!(f, "option {flag}: expected a positive numerical parameter")
            }
            ParseError::TooLarge(flag) => write!(f, "option {flag}: too large number"),
            ParseError::ReadFile(flag) => {
                write!(f, "option {flag}: error encountered when reading a file")
            }
            ParseError::BadData(m) => write!(f, "{m}"),
            ParseError::UnknownConfigOption(flag) => {
                write!(f, "option {flag}: found an unknown config option")
            }
            ParseError::Warned(_, error) => write!(f, "{error}"),
        }
    }
}

/// Short flags that consume a value (`-o file`, and `-ofile` inside a cluster).
const VALUE_SHORTS: &[char] = &[
    'o', 'X', 'd', 'H', 'm', 'w', 'A', 'u', 'e', 'F', 'b', 'c', 'T', 'K', 'Q', 'r',
];

/// The long options this curl knows, each with whether it takes a value: what a `-K` file's
/// lines may name.
const LONGS: &[(&str, bool)] = &[
    ("compressed", false),
    ("config", true),
    ("connect-timeout", true),
    ("cookie", true),
    ("cookie-jar", true),
    ("create-dirs", false),
    ("data", true),
    ("data-ascii", true),
    ("data-binary", true),
    ("data-raw", true),
    ("data-urlencode", true),
    ("fail", false),
    ("fail-with-body", false),
    ("form", true),
    ("get", false),
    ("head", false),
    ("header", true),
    ("include", false),
    ("insecure", false),
    ("json", true),
    ("location", false),
    ("max-time", true),
    ("output", true),
    ("quote", true),
    ("range", true),
    ("referer", true),
    ("remote-header-name", false),
    ("remote-name", false),
    ("request", true),
    ("retry", true),
    ("retry-delay", true),
    ("show-error", false),
    ("silent", false),
    ("upload-file", true),
    ("url", true),
    ("user", true),
    ("user-agent", true),
    ("verbose", false),
    ("version", false),
    ("write-out", true),
];

/// Whether `flag` (`-x` or `--long`) takes a value, or `None` if curl here has no such option.
fn takes_value(flag: &str) -> Option<bool> {
    if let Some(long) = flag.strip_prefix("--") {
        return LONGS
            .iter()
            .find(|(name, _)| *name == long)
            .map(|(_, value)| *value);
    }
    let mut chars = flag.strip_prefix('-')?.chars();
    let (Some(short), None) = (chars.next(), chars.next()) else {
        return None;
    };
    const FLAG_SHORTS: &[char] = &['s', 'S', 'L', 'i', 'I', 'f', 'v', 'G', 'O', 'J', 'k', 'V'];
    if VALUE_SHORTS.contains(&short) {
        Some(true)
    } else {
        FLAG_SHORTS.contains(&short).then_some(false)
    }
}

/// Normalize argv into a flat flag/value stream: expand `--flag=value` into two tokens and split
/// short-flag clusters (`-fsSL` → `-f -s -S -L`), where a value-taking short consumes the rest of
/// its cluster as its value (`-ofile` → `-o file`) or, if nothing remains, the next argv token.
/// Everything after `--` is left as it is: those words are URLs.
fn expand_args(args: &[String]) -> Vec<String> {
    let mut out = Vec::with_capacity(args.len());
    let mut words = args.iter();
    for arg in words.by_ref() {
        if arg == "--" {
            out.push(arg.clone());
            break;
        }
        if let Some(long) = arg.strip_prefix("--") {
            match long.split_once('=') {
                Some((flag, value)) => {
                    out.push(format!("--{flag}"));
                    out.push(value.to_string());
                }
                None => out.push(arg.clone()),
            }
        } else if arg.len() > 2 && arg.starts_with('-') {
            // A short cluster like `-sL` or `-ofile`.
            let chars: Vec<char> = arg[1..].chars().collect();
            let mut i = 0;
            while i < chars.len() {
                let c = chars[i];
                out.push(format!("-{c}"));
                if VALUE_SHORTS.contains(&c) {
                    let rest: String = chars[i + 1..].iter().collect();
                    if !rest.is_empty() {
                        out.push(rest);
                    }
                    break;
                }
                i += 1;
            }
        } else {
            out.push(arg.clone());
        }
    }
    out.extend(words.cloned());
    out
}

/// How deep `-K` files may name further `-K` files, as in curl.
const MAX_CONFIG_DEPTH: usize = 5;

/// The words the `-K` file `path` stands for, read as curl reads one: a line per option, blank
/// lines and `#` lines skipped; the option's name (with or without its dashes) runs to a blank,
/// `=` or `:`, and its value follows, in double quotes (with `\\`, `\"`, `\t`, `\n`, `\r` and
/// `\v`) or up to the next blank. A `-K` line reads that file in its place, up to
/// [`MAX_CONFIG_DEPTH`] files deep. `flag` names the option in curl's diagnostics; `silent` is
/// whether a `-s` came first, which hides the warnings that name the bad line; `depth` is how
/// many files deep this one is.
fn config_words(
    path: &str,
    cwd: &std::path::Path,
    flag: &str,
    mut silent: bool,
    depth: usize,
) -> Result<Vec<String>, ParseError> {
    let warned = |silent: bool, warning: String, error: ParseError| {
        if silent {
            error
        } else {
            ParseError::Warned(warning, Box::new(error))
        }
    };
    let Ok(text) = std::fs::read_to_string(cwd.join(path)) else {
        let warning = format!("curl: cannot read config from '{path}'\n");
        return Err(warned(
            silent,
            warning,
            ParseError::ReadFile(flag.to_owned()),
        ));
    };
    let mut words = Vec::new();
    let mut number = 0;
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        number += 1;
        // curl reads the name from where the line starts, so an indented option has none.
        let end = line
            .find(|c: char| c.is_whitespace() || c == '=' || c == ':')
            .unwrap_or(line.len());
        let name = &line[..end];
        let mut rest = line[end..].trim_start();
        if let Some(after) = rest.strip_prefix(['=', ':']) {
            rest = after.trim_start();
        }
        let value = match rest.strip_prefix('"') {
            Some(quoted) => {
                let mut value = String::new();
                let mut chars = quoted.chars();
                while let Some(c) = chars.next() {
                    match c {
                        '"' => break,
                        '\\' => match chars.next() {
                            Some('t') => value.push('\t'),
                            Some('n') => value.push('\n'),
                            Some('r') => value.push('\r'),
                            Some('v') => value.push('\x0b'),
                            Some(other) => value.push(other),
                            None => break,
                        },
                        other => value.push(other),
                    }
                }
                value
            }
            None => rest
                .split(char::is_whitespace)
                .next()
                .unwrap_or_default()
                .to_owned(),
        };
        let option = if name.starts_with('-') {
            name.to_owned()
        } else {
            format!("--{name}")
        };
        let at = format!("curl: {path}:{number} config file option '{name}'");
        match takes_value(&option) {
            None => {
                return Err(warned(
                    silent,
                    format!("{at} is unknown\n"),
                    ParseError::UnknownConfigOption(flag.to_owned()),
                ));
            }
            Some(true) if value.is_empty() && !rest.starts_with('"') => {
                return Err(warned(
                    silent,
                    format!("{at} requires parameter\n"),
                    ParseError::MissingValue(flag.to_owned()),
                ));
            }
            Some(true) if matches!(option.as_str(), "-K" | "--config") => {
                // Each file the recursion passed through names its own `-K` line on the way out.
                let badly_used = format!("{at} is badly used here\n");
                let nested = if depth >= MAX_CONFIG_DEPTH {
                    let limit = format!(
                        "curl: Max config file recursion level reached ({MAX_CONFIG_DEPTH})\n"
                    );
                    Err(warned(
                        silent,
                        limit,
                        ParseError::BadlyUsed(flag.to_owned()),
                    ))
                } else {
                    config_words(&value, cwd, flag, silent, depth + 1)
                };
                match nested {
                    Ok(nested) => words.extend(nested),
                    Err(ParseError::Warned(warnings, error)) if !silent => {
                        return Err(ParseError::Warned(warnings + &badly_used, error));
                    }
                    Err(error) => return Err(error),
                }
            }
            Some(true) => words.extend([option, value]),
            Some(false) => {
                silent |= matches!(option.as_str(), "-s" | "--silent");
                words.push(option);
            }
        }
    }
    Ok(words)
}

/// Parse argv (without the leading `curl` word) into a [`Request`].
#[cfg(test)]
pub(crate) fn parse(args: &[String]) -> Result<Request, ParseError> {
    parse_at(args, std::path::Path::new("."))
}

pub(crate) fn parse_at(args: &[String], cwd: &std::path::Path) -> Result<Request, ParseError> {
    if args.is_empty() {
        return Err(ParseError::NoArguments);
    }
    let mut words: std::collections::VecDeque<String> = expand_args(args).into();

    let mut urls: Vec<String> = Vec::new();
    let mut outputs: Vec<Destination> = Vec::new();
    let mut method: Option<Method> = None;
    let mut headers = Vec::new();
    let mut data: Option<Vec<u8>> = None;
    let mut silent = false;
    let mut show_error = false;
    let mut location = false;
    let mut include = false;
    let mut head = false;
    let mut fail = false;
    let mut fail_with_body = false;
    let mut max_time = None;
    let mut connect_timeout = None;
    let mut write_out = None;
    let mut verbose = false;
    let mut get = false;
    let mut json = false;
    let mut forms = Vec::new();
    let mut remote_header_name = false;
    let mut create_dirs = false;
    let mut retry = 0u32;
    let mut retry_delay = 0.0f64;
    let mut cookie = None;
    let mut cookie_jar = None;
    let mut upload_file = None;
    let mut compressed = false;
    let mut insecure = false;
    let mut options_end = false;

    while let Some(arg) = words.pop_front() {
        if options_end || arg == "-" || !arg.starts_with('-') {
            if arg.is_empty() {
                return Err(ParseError::Blank(String::new()));
            }
            urls.push(arg);
            continue;
        }
        let mut next = || {
            words
                .pop_front()
                .ok_or_else(|| ParseError::MissingValue(arg.clone()))
        };
        // The warning before a failed file read, which a `-s` given earlier hides.
        let unreadable = |warning: String| {
            let error = ParseError::ReadFile(arg.clone());
            if silent {
                error
            } else {
                ParseError::Warned(warning, Box::new(error))
            }
        };
        match arg.as_str() {
            "--" => options_end = true,
            "-o" | "--output" => {
                let path = next()?;
                if path.is_empty() {
                    return Err(ParseError::Blank(arg));
                }
                outputs.push(Destination::File(path));
            }
            "-O" | "--remote-name" => outputs.push(Destination::Remote),
            "-s" | "--silent" => silent = true,
            "-L" | "--location" => location = true,
            "-i" | "--include" => include = true,
            "-I" | "--head" => head = true,
            "-f" | "--fail" => fail = true,
            "--fail-with-body" => fail_with_body = true,
            "-v" | "--verbose" => verbose = true,
            "-G" | "--get" => get = true,
            "-S" | "--show-error" => show_error = true,
            "-k" | "--insecure" => insecure = true,
            "-V" | "--version" => return Err(ParseError::Version),
            "-X" | "--request" => {
                let m = next()?;
                method = Some(
                    Method::from_bytes(m.to_uppercase().as_bytes())
                        .map_err(|_| ParseError::BadMethod(m))?,
                );
            }
            "-d" | "--data" | "--data-ascii" | "--data-binary" | "--data-raw" => {
                let value = next()?;
                let strip_newlines = arg != "--data-binary";
                let piece = resolve_data(&value, arg == "--data-raw", strip_newlines, cwd)
                    .map_err(|error| match error {
                        DataError::Stdin(message) => ParseError::BadData(message),
                        DataError::Unreadable(path) => {
                            unreadable(format!("curl: Failed to open {path}\n"))
                        }
                    })?;
                append_data(&mut data, piece);
            }
            "--data-urlencode" => {
                let value = next()?;
                let piece = urlencode_data(&value, cwd).map_err(|error| match error {
                    DataError::Stdin(message) => ParseError::BadData(message),
                    DataError::Unreadable(path) => {
                        unreadable(format!("curl: Failed to open {path}\n"))
                    }
                })?;
                append_data(&mut data, piece);
            }
            "--json" => {
                let value = next()?;
                data = Some(resolve_data(&value, false, false, cwd).map_err(
                    |error| match error {
                        DataError::Stdin(message) => ParseError::BadData(message),
                        DataError::Unreadable(path) => {
                            unreadable(format!("curl: Failed to open {path}\n"))
                        }
                    },
                )?);
                json = true;
            }
            "-H" | "--header" => headers.push(split_header(&next()?)),
            "-A" | "--user-agent" => headers.push(("User-Agent".to_string(), next()?)),
            "-e" | "--referer" => headers.push(("Referer".to_string(), next()?)),
            "-r" | "--range" => headers.push(("Range".to_string(), format!("bytes={}", next()?))),
            "-u" | "--user" => {
                let cred = next()?;
                headers.push((
                    "Authorization".to_string(),
                    format!("Basic {}", base64(cred.as_bytes())),
                ));
            }
            "-m" | "--max-time" => {
                let value = next()?;
                max_time = Some(parse_secs(&arg, &value)?);
            }
            "--connect-timeout" => {
                let value = next()?;
                connect_timeout = Some(parse_secs(&arg, &value)?);
            }
            "-w" | "--write-out" => write_out = Some(next()?),
            "--url" => {
                let url = next()?;
                if url.is_empty() {
                    return Err(ParseError::Blank(arg));
                }
                urls.push(url);
            }
            "-F" | "--form" => {
                let value = next()?;
                let Some(field) = parse_form_field(&value) else {
                    let error = ParseError::BadlyUsed(arg);
                    return Err(if silent {
                        error
                    } else {
                        let warning = "Warning: Illegally formatted input field\n".to_owned();
                        ParseError::Warned(warning, Box::new(error))
                    });
                };
                forms.push(field);
            }
            "-J" | "--remote-header-name" => remote_header_name = true,
            "--create-dirs" => create_dirs = true,
            "--retry" => {
                let value = next()?;
                retry = parse_count(&arg, &value)?;
            }
            "--retry-delay" => {
                let value = next()?;
                retry_delay = parse_secs(&arg, &value)?;
            }
            "-b" | "--cookie" => cookie = Some(next()?),
            "-c" | "--cookie-jar" => cookie_jar = Some(next()?),
            "-T" | "--upload-file" => upload_file = Some(next()?),
            "--compressed" => compressed = true,
            // FTP and SFTP commands to send around a transfer: nothing to send over HTTP.
            "-Q" | "--quote" => {
                next()?;
            }
            "-K" | "--config" => {
                let path = next()?;
                if path == "-" {
                    return Err(ParseError::BadData(format!(
                        "option {arg}: reading a config from standard input is unsupported in \
                         bash-tool"
                    )));
                }
                let config = config_words(&path, cwd, &arg, silent, 1)?;
                for word in config.into_iter().rev() {
                    words.push_front(word);
                }
            }
            other => return Err(ParseError::UnknownFlag(other.to_string())),
        }
    }

    // `--json` sets the content negotiation headers unless the caller already set them.
    if json {
        ensure_header(&mut headers, "Content-Type", "application/json");
        ensure_header(&mut headers, "Accept", "application/json");
    }

    // Method resolution: explicit `-X` wins; then `-I` (HEAD); then `-G` (GET); then `-T` (PUT);
    // then a body or `-F` implies POST; else GET. whttp skips reading the (absent) body of a HEAD
    // response — see `is_bodyless`.
    let method = method.unwrap_or_else(|| {
        if head {
            Method::HEAD
        } else if get {
            Method::GET
        } else if upload_file.is_some() {
            Method::PUT
        } else if data.is_some() || !forms.is_empty() {
            Method::POST
        } else {
            Method::GET
        }
    });

    let url = urls.first().cloned().ok_or(ParseError::MissingUrl)?;
    let destination = outputs.first().cloned();
    Ok(Request {
        url,
        urls,
        outputs,
        method,
        headers,
        data,
        output: match &destination {
            Some(Destination::File(path)) => Some(path.clone()),
            _ => None,
        },
        silent,
        show_error,
        location,
        include,
        head,
        fail: fail || fail_with_body,
        fail_with_body,
        max_time,
        connect_timeout,
        write_out,
        verbose,
        get,
        forms,
        remote_name: destination == Some(Destination::Remote),
        remote_header_name,
        create_dirs,
        retry,
        retry_delay,
        cookie,
        cookie_jar,
        upload_file,
        compressed,
        insecure,
    })
}

impl Request {
    /// One request per URL, in order, each with the output that pairs with it: the n-th `-o` or
    /// `-O`, or stdout once they run out.
    pub(crate) fn transfers(&self) -> Vec<Request> {
        self.urls
            .iter()
            .enumerate()
            .map(|(index, url)| {
                let destination = self.outputs.get(index);
                Request {
                    url: url.clone(),
                    output: match destination {
                        Some(Destination::File(path)) => Some(path.clone()),
                        _ => None,
                    },
                    remote_name: destination == Some(&Destination::Remote),
                    ..self.clone()
                }
            })
            .collect()
    }
}

/// Parse one `-F name=value` or `-F name=@path[;type=CONTENT_TYPE]` field; `None` when it has no
/// `=`, which curl calls badly used.
fn parse_form_field(raw: &str) -> Option<FormField> {
    let (name, rest) = raw.split_once('=')?;
    if let Some(spec) = rest.strip_prefix('@') {
        let (path, content_type) = match spec.split_once(";type=") {
            Some((path, ct)) => (path, Some(ct.to_string())),
            None => (spec, None),
        };
        if path.is_empty() {
            return None;
        }
        Some(FormField::File {
            name: name.to_string(),
            path: path.to_string(),
            content_type,
        })
    } else {
        Some(FormField::Text {
            name: name.to_string(),
            value: rest.to_string(),
        })
    }
}

/// A count (`--retry`) as curl reads one: digits only, and not negative.
fn parse_count(flag: &str, value: &str) -> Result<u32, ParseError> {
    if value.starts_with('-') && value.len() > 1 && value[1..].bytes().all(|b| b.is_ascii_digit()) {
        return Err(ParseError::Negative(flag.to_owned()));
    }
    if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
        return Err(ParseError::BadNumber(flag.to_owned()));
    }
    value
        .parse::<u32>()
        .map_err(|_| ParseError::TooLarge(flag.to_owned()))
}

/// Why a data option's value could not be resolved.
enum DataError {
    /// `@-`: standard input, which this parser does not have; holds the message.
    Stdin(String),
    /// The named file could not be read; holds the path as given.
    Unreadable(String),
}

/// Resolve a `-d`/`--data`/`--json` value: a leading `@` (unless `--data-raw`) reads a file. `@-`
/// would mean stdin in curl; we don't have a stdin handle here, so it's an error. `strip_newlines`
/// is curl's own behavior for `-d`/`--data`/`--data-raw` (but not `--data-binary` or `--json`): CR
/// and LF bytes read from an `@file` are removed, so a file ending (or containing) a newline still
/// posts as one unbroken line. It applies only to bytes read from a file, never to a value given
/// literally on the command line.
fn resolve_data(
    value: &str,
    raw: bool,
    strip_newlines: bool,
    cwd: &std::path::Path,
) -> Result<Vec<u8>, DataError> {
    if !raw && let Some(path) = value.strip_prefix('@') {
        let bytes = read_data_file(path, cwd)?;
        return Ok(if strip_newlines {
            bytes
                .into_iter()
                .filter(|b| *b != b'\r' && *b != b'\n')
                .collect()
        } else {
            bytes
        });
    }
    Ok(value.as_bytes().to_vec())
}

/// The file a data option's `@path` names; `@-` is standard input, which is not available here.
fn read_data_file(path: &str, cwd: &std::path::Path) -> Result<Vec<u8>, DataError> {
    if path == "-" {
        return Err(DataError::Stdin(
            "-d @-: stdin is not available here".into(),
        ));
    }
    std::fs::read(cwd.join(path)).map_err(|_| DataError::Unreadable(path.to_owned()))
}

/// A `--data-urlencode` value as curl sends it: `content`, `=content`, `name=content`, `@file`
/// or `name@file`, with the content (or the file's bytes, as they are) URL-encoded and `name=`
/// kept in front.
fn urlencode_data(value: &str, cwd: &std::path::Path) -> Result<Vec<u8>, DataError> {
    let (name, content) = match value.find(['=', '@']) {
        Some(at) if value.as_bytes()[at] == b'@' => {
            (&value[..at], read_data_file(&value[at + 1..], cwd)?)
        }
        Some(at) => (&value[..at], value.as_bytes()[at + 1..].to_vec()),
        None => ("", value.as_bytes().to_vec()),
    };
    let mut out = Vec::new();
    if !name.is_empty() {
        out.extend_from_slice(name.as_bytes());
        out.push(b'=');
    }
    for byte in content {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => out.push(byte),
            b' ' => out.push(b'+'),
            _ => out.extend_from_slice(format!("%{byte:02X}").as_bytes()),
        }
    }
    Ok(out)
}

/// Append a `-d`/`--data`/`--data-binary`/`--data-raw` piece to the accumulated request body:
/// curl joins repeated data arguments with `&`, exactly as if they had been given as one value.
fn append_data(data: &mut Option<Vec<u8>>, piece: Vec<u8>) {
    match data {
        Some(existing) => {
            existing.push(b'&');
            existing.extend_from_slice(&piece);
        }
        None => *data = Some(piece),
    }
}

/// Add `(name, value)` only if no header with that name (case-insensitive) is present.
fn ensure_header(headers: &mut Vec<(String, String)>, name: &str, value: &str) {
    if !headers.iter().any(|(k, _)| k.eq_ignore_ascii_case(name)) {
        headers.push((name.to_string(), value.to_string()));
    }
}

/// Parse a seconds value (curl accepts fractional seconds).
/// A seconds value as curl reads one (`secs2ms` in curl's `tool_paramhlp.c`): whole seconds up to
/// `LONG_MAX / 1000 - 1`, then optionally `.` and at least one digit, kept to the millisecond.
/// Whatever follows the number is ignored, so `1e300` is one second, as it is in curl.
fn parse_secs(flag: &str, value: &str) -> Result<f64, ParseError> {
    const MAX_SECS: u64 = i64::MAX as u64 / 1000 - 1;
    let bad = || ParseError::BadNumber(flag.to_string());
    let digits = |s: &str| s.bytes().take_while(u8::is_ascii_digit).count();
    let whole = &value[..digits(value)];
    let secs = whole
        .parse::<u64>()
        .ok()
        .filter(|secs| *secs <= MAX_SECS)
        .ok_or_else(bad)?;
    let mut millis = 0;
    if let Some(rest) = value[whole.len()..].strip_prefix('.') {
        // curl reports any fraction it cannot read, an empty one included, as too large.
        let fraction = &rest[..digits(rest)];
        if fraction.parse::<i64>().is_err() {
            return Err(ParseError::TooLarge(flag.to_string()));
        }
        // curl keeps the first three fractional digits, as milliseconds.
        let kept = &fraction[..fraction.len().min(3)];
        millis = kept.parse::<u64>().unwrap_or(0) * 10_u64.pow(3 - kept.len() as u32);
    }
    Ok(secs as f64 + millis as f64 / 1000.0)
}

/// Split a `-H "Key: Value"` argument into `(key, value)`, trimming whitespace. No colon → an
/// empty-valued header.
fn split_header(raw: &str) -> (String, String) {
    match raw.split_once(':') {
        Some((k, v)) => (k.trim().to_string(), v.trim().to_string()),
        None => (raw.trim().to_string(), String::new()),
    }
}

/// Standard base64 (with padding) — for `-u user:pass` → `Authorization: Basic …`. Hand-rolled to
/// avoid a dependency in this small wasm-facing crate.
fn base64(input: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0];
        let b1 = *chunk.get(1).unwrap_or(&0);
        let b2 = *chunk.get(2).unwrap_or(&0);
        let n = u32::from(b0) << 16 | u32::from(b1) << 8 | u32::from(b2);
        out.push(T[(n >> 18 & 63) as usize] as char);
        out.push(T[(n >> 12 & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            T[(n >> 6 & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            T[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(std::string::ToString::to_string).collect()
    }

    #[test]
    fn plain_url_is_a_get() {
        let r = parse(&argv(&["https://example.com"])).unwrap();
        assert_eq!(r.url, "https://example.com");
        assert_eq!(r.method, Method::GET);
        assert!(!r.location && !r.include && !r.head && !r.fail);
    }

    #[test]
    fn data_implies_post_and_is_bytes() {
        let r = parse(&argv(&["-d", "a=1", "https://example.com"])).unwrap();
        assert_eq!(r.method, Method::POST);
        assert_eq!(r.data.as_deref(), Some(b"a=1".as_slice()));
    }

    #[test]
    fn explicit_method_overrides_data_default() {
        let r = parse(&argv(&["-X", "PUT", "-d", "a=1", "https://example.com"])).unwrap();
        assert_eq!(r.method, Method::PUT);
    }

    #[test]
    fn head_flag_sets_head_method() {
        let r = parse(&argv(&["-I", "https://example.com"])).unwrap();
        assert_eq!(r.method, Method::HEAD);
        assert!(r.head);
    }

    #[test]
    fn clustered_short_flags_expand() {
        // The classic installer one-liner.
        let r = parse(&argv(&["-fsSL", "https://example.com"])).unwrap();
        assert!(r.fail && r.silent && r.location);
    }

    #[test]
    fn clustered_value_short_takes_the_remainder() {
        let r = parse(&argv(&["-sod", "https://example.com"])).unwrap();
        // -s, then -o with value "d".
        assert!(r.silent);
        assert_eq!(r.output.as_deref(), Some("d"));
    }

    #[test]
    fn long_flag_equals_value() {
        let r = parse(&argv(&["--request=DELETE", "https://example.com"])).unwrap();
        assert_eq!(r.method, Method::DELETE);
    }

    #[test]
    fn json_sets_content_headers_and_posts() {
        let r = parse(&argv(&["--json", r#"{"a":1}"#, "https://example.com"])).unwrap();
        assert_eq!(r.method, Method::POST);
        assert!(
            r.headers
                .iter()
                .any(|(k, v)| k == "Content-Type" && v == "application/json")
        );
        assert!(
            r.headers
                .iter()
                .any(|(k, v)| k == "Accept" && v == "application/json")
        );
    }

    #[test]
    fn basic_auth_encodes_credentials() {
        let r = parse(&argv(&["-u", "user:pass", "https://example.com"])).unwrap();
        // base64("user:pass") == "dXNlcjpwYXNz"
        assert!(
            r.headers
                .iter()
                .any(|(k, v)| k == "Authorization" && v == "Basic dXNlcjpwYXNz")
        );
    }

    #[test]
    fn user_agent_and_referer_become_headers() {
        let r = parse(&argv(&[
            "-A",
            "bash-tool/1",
            "-e",
            "https://ref",
            "https://example.com",
        ]))
        .unwrap();
        assert!(
            r.headers
                .iter()
                .any(|(k, v)| k == "User-Agent" && v == "bash-tool/1")
        );
        assert!(
            r.headers
                .iter()
                .any(|(k, v)| k == "Referer" && v == "https://ref")
        );
    }

    #[test]
    fn timeouts_parse_as_seconds() {
        let r = parse(&argv(&[
            "-m",
            "2.5",
            "--connect-timeout",
            "1",
            "https://example.com",
        ]))
        .unwrap();
        assert_eq!(r.max_time, Some(2.5));
        assert_eq!(r.connect_timeout, Some(1.0));
    }

    #[test]
    fn bad_timeout_is_an_error() {
        assert!(matches!(
            parse(&argv(&["-m", "soon", "https://x"])),
            Err(ParseError::BadNumber(flag)) if flag == "-m"
        ));
    }

    #[test]
    fn timeouts_follow_curls_seconds_grammar() {
        // Checked against curl 8 in the conformance oracle.
        let accepted = [
            ("1", 1.0),
            ("1.5", 1.5),
            ("2.25x", 2.25),
            ("0", 0.0),
            ("3 ", 3.0),
            ("1x", 1.0),
            ("1e300", 1.0),
            ("9223372036854774", 9_223_372_036_854_774.0),
        ];
        let rejected = [
            "-1",
            "-0",
            "+3",
            " 3",
            ".5",
            "nan",
            "inf",
            "abc",
            "",
            "9223372036854775",
            "99999999999999999999",
        ];
        for flag in ["-m", "--max-time", "--connect-timeout", "--retry-delay"] {
            for (value, seconds) in accepted {
                let r = parse(&argv(&[flag, value, "https://x"])).unwrap();
                let got = match flag {
                    "--connect-timeout" => r.connect_timeout,
                    "--retry-delay" => Some(r.retry_delay),
                    _ => r.max_time,
                };
                assert_eq!(got, Some(seconds), "{flag} {value:?}");
            }
            for value in rejected {
                let err = parse(&argv(&[flag, value, "https://x"])).unwrap_err();
                assert_eq!(
                    err.to_string(),
                    format!("option {flag}: expected a proper numerical parameter"),
                    "{flag} {value:?}"
                );
            }
            // curl reports any fraction it cannot read this way, an empty one included.
            for value in ["1.9999999999999999999", "5."] {
                let err = parse(&argv(&[flag, value, "https://x"])).unwrap_err();
                assert_eq!(err.to_string(), format!("option {flag}: too large number"));
            }
        }
    }

    #[test]
    fn data_from_file_is_read() {
        let path = std::env::temp_dir().join(format!("wcurl_data_{}", std::process::id()));
        std::fs::write(&path, b"file-payload").unwrap();
        let r = parse(&argv(&["-d", &format!("@{}", path.display()), "https://x"])).unwrap();
        assert_eq!(r.data.as_deref(), Some(b"file-payload".as_slice()));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn data_raw_keeps_the_at_sign() {
        let r = parse(&argv(&["--data-raw", "@notafile", "https://x"])).unwrap();
        assert_eq!(r.data.as_deref(), Some(b"@notafile".as_slice()));
    }

    // repeated `-d`/`--data`/`--data-binary`/`--data-raw` join with `&`, as curl does,
    // instead of the last one silently winning.
    #[test]
    fn repeated_data_joins_with_ampersand() {
        let r = parse(&argv(&["-d", "a=1", "-d", "b=2", "https://x"])).unwrap();
        assert_eq!(r.data.as_deref(), Some(b"a=1&b=2".as_slice()));
    }

    #[test]
    fn data_and_data_binary_join_with_ampersand() {
        let r = parse(&argv(&["-d", "a=1", "--data-binary", "b=2", "https://x"])).unwrap();
        assert_eq!(r.data.as_deref(), Some(b"a=1&b=2".as_slice()));
    }

    #[test]
    fn three_data_arguments_join_in_order() {
        let r = parse(&argv(&["-d", "a=1", "-d", "b=2", "-d", "c=3", "https://x"])).unwrap();
        assert_eq!(r.data.as_deref(), Some(b"a=1&b=2&c=3".as_slice()));
    }

    // `-d`/`--data`/`--data-raw` strip CR/LF bytes read from an `@file` (but not from a
    // literal value, and not for `--data-binary`).
    #[test]
    fn data_from_file_strips_newlines() {
        let path = std::env::temp_dir().join(format!("wcurl_data_nl_{}", std::process::id()));
        std::fs::write(&path, b"a=1\nb=2\n").unwrap();
        let r = parse(&argv(&["-d", &format!("@{}", path.display()), "https://x"])).unwrap();
        assert_eq!(r.data.as_deref(), Some(b"a=1b=2".as_slice()));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn data_binary_from_file_keeps_newlines() {
        let path = std::env::temp_dir().join(format!("wcurl_data_bin_{}", std::process::id()));
        std::fs::write(&path, b"a=1\nb=2\n").unwrap();
        let r = parse(&argv(&[
            "--data-binary",
            &format!("@{}", path.display()),
            "https://x",
        ]))
        .unwrap();
        assert_eq!(r.data.as_deref(), Some(b"a=1\nb=2\n".as_slice()));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn data_literal_value_keeps_newlines() {
        let r = parse(&argv(&["-d", "a=1\nb=2", "https://x"])).unwrap();
        assert_eq!(r.data.as_deref(), Some(b"a=1\nb=2".as_slice()));
    }

    #[test]
    fn write_out_and_url_flags() {
        let r = parse(&argv(&[
            "-w",
            "%{http_code}",
            "--url",
            "https://example.com",
        ]))
        .unwrap();
        assert_eq!(r.write_out.as_deref(), Some("%{http_code}"));
        assert_eq!(r.url, "https://example.com");
    }

    #[test]
    fn get_flag_records_intent() {
        let r = parse(&argv(&["-G", "-d", "q=1", "https://example.com"])).unwrap();
        assert!(r.get);
        assert_eq!(r.method, Method::GET);
    }

    #[test]
    fn missing_url_errors() {
        assert_eq!(parse(&argv(&["-s"])), Err(ParseError::MissingUrl));
    }

    #[test]
    fn unknown_flag_errors() {
        assert_eq!(
            parse(&argv(&["--bogus", "https://example.com"])),
            Err(ParseError::UnknownFlag("--bogus".into()))
        );
    }
}
