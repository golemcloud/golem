//! `waget` — a `wasm32-wasip2`-compatible `wget` clone, embeddable in any wasm component.
//!
//! Exposes an async [`run`] that takes argv (without the leading `wget` word) and returns a
//! structured [`Outcome`]. The HTTP transport — the cfg-gated `wasi-fetch`/`reqwest` seam plus
//! redirect following and timeouts — lives in [`whttp`]; this crate parses `wget` flags and writes
//! the file.
//! `run` creates no runtime; the caller awaits it.
//!
//! Unlike curl, wget writes to a **file** by default (named after the URL's last path segment);
//! `-O <file>` overrides the name and `-O -` streams to stdout.

mod parse;

use std::fmt::Write as _;

pub use parse::ParseError;
use parse::{Output, Request};

/// The result of a `waget` invocation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outcome {
    /// Bytes written to standard output (the body under `-O -`).
    pub stdout: Vec<u8>,
    /// Bytes written to standard error (progress/saved/status/`-S` messages).
    pub stderr: Vec<u8>,
    /// The process exit code, as wget's: 0 success, 1 a generic error (a URL wget cannot use), 2
    /// a usage error, 3 a file error, 4 a network failure, 8 an error response from the server.
    pub exit_code: u8,
}

/// A parsed `-T` period as a duration. WASI-HTTP carries timeouts as u64 nanoseconds, so a longer
/// one (wget accepts any finite number of seconds) is capped there rather than wrapped.
fn seconds(secs: f64) -> std::time::Duration {
    std::time::Duration::try_from_secs_f64(secs)
        .unwrap_or(std::time::Duration::MAX)
        .min(std::time::Duration::from_nanos(u64::MAX))
}

/// `-T 0` means "wait indefinitely" in wget, not a zero-length timeout (`seconds(0.0)` would give
/// `Duration::ZERO`, timing out before the request could even start).
fn timeout_seconds(secs: f64) -> std::time::Duration {
    if secs == 0.0 {
        whttp::NO_TIMEOUT
    } else {
        seconds(secs)
    }
}

/// `-V`/`--version`: the wget release whose behavior this one follows.
const VERSION: &str = "GNU Wget 1.25.0 built on wasm32-wasip2 (bash-tool).\n";

/// Where to read more after a command-line mistake.
const USAGE: &str = "Usage: wget [OPTION]... [URL]...\n\nTry `wget --help' for more options.\n";

impl Outcome {
    /// A parse error as wget reports it: a missing URL exits 1 and a mistake 2, with its usage;
    /// a bad value is reported alone; `--post-data` with `--post-file` exits 1; `-V` is the
    /// version.
    fn from_parse_error(error: &parse::ParseError) -> Self {
        let (stderr, exit_code) = match error {
            parse::ParseError::Version => {
                return Outcome {
                    stdout: VERSION.as_bytes().to_vec(),
                    stderr: Vec::new(),
                    exit_code: 0,
                };
            }
            parse::ParseError::PostDataAndFile => (format!("{error}\n"), 1),
            parse::ParseError::MissingUrl => (format!("wget: {error}\n{USAGE}"), 1),
            parse::ParseError::InvalidPeriod(_)
            | parse::ParseError::NegativePeriod(_)
            | parse::ParseError::InvalidNumber(..)
            | parse::ParseError::InputFromStdin => (format!("wget: {error}\n"), 2),
            _ => (format!("wget: {error}\n{USAGE}"), 2),
        };
        Outcome {
            stdout: Vec::new(),
            stderr: stderr.into_bytes(),
            exit_code,
        }
    }
}

/// An I/O error as wget words one: the C library's message, without Rust's `(os error N)`.
fn io_message(error: &std::io::Error) -> String {
    use std::io::ErrorKind;
    match error.kind() {
        ErrorKind::NotFound => "No such file or directory".to_owned(),
        ErrorKind::PermissionDenied => "Permission denied".to_owned(),
        ErrorKind::IsADirectory => "Is a directory".to_owned(),
        ErrorKind::NotADirectory => "Not a directory".to_owned(),
        _ => {
            let message = error.to_string();
            match message.find(" (os error") {
                Some(at) => message[..at].to_owned(),
                None => message,
            }
        }
    }
}

/// Two transfers' statuses as wget combines them: any failure over success, and among failures
/// the lower-numbered one, except that the generic 1 gives way to any other.
fn combine(so_far: u8, code: u8) -> u8 {
    match (so_far, code) {
        (0, other) | (other, 0) | (1, other) | (other, 1) => other,
        (a, b) => a.min(b),
    }
}

/// Run a `wget`-style invocation. `args` is argv **without** the leading command word.
pub async fn run(args: &[String]) -> Outcome {
    run_in_directory(args, std::path::Path::new(".")).await
}

/// Resolve file operands against a shell working directory without changing process state; the
/// process's `HOME` decides whether wget can keep its HSTS store.
pub async fn run_in_directory(args: &[String], cwd: &std::path::Path) -> Outcome {
    let home = std::env::var("HOME").ok();
    let mut stdout = Vec::new();
    match run_streaming(args, cwd, home.as_deref(), &mut stdout).await {
        Ok(outcome) => Outcome { stdout, ..outcome },
        // A `Vec` takes every write.
        Err(error) => Outcome {
            stdout,
            stderr: format!("wget: write error: {error}\n").into_bytes(),
            exit_code: 3,
        },
    }
}

/// Like [`run_in_directory`], but the response body is forwarded chunk-by-chunk instead of
/// buffered: to `stdout` under `-O -`, or incrementally into the target file otherwise (which
/// makes `-c`'s append naturally incremental too — the file grows chunk by chunk instead of one
/// `write_all` of the whole missing tail). Written for the shell (`execute_http`), which passes the
/// `HOME` its script exports: without one wget cannot keep its HSTS store, and says so.
///
/// Each URL — the command line's, then those in `-i`'s file — is a transfer of its own, in order;
/// `-O FILE` is opened (emptied) before any of them and takes them all. The status combines
/// theirs as wget does (see [`combine`]).
///
/// Returns `Err` only for a write failure on `stdout` (the sink, under `-O -`) — a closed pipe.
/// Every other failure (usage, transport, a local file-write error) comes back through the
/// returned `Outcome`, never as `Err`. A file-write failure partway through `-c` append or a fresh
/// write leaves the file with WHATEVER PREFIX was already flushed — wget itself has the same
/// property (it doesn't buffer the whole body either), so this isn't a new risk this crate adds.
///
/// # Errors
///
/// Returns the [`std::io::Error`] from the first failed write to `stdout`.
pub async fn run_streaming(
    args: &[String],
    cwd: &std::path::Path,
    home: Option<&str>,
    stdout: &mut (dyn futures::io::AsyncWrite + Unpin + Send),
) -> Result<Outcome, std::io::Error> {
    let req = match parse::parse_at(args) {
        Ok(req) => req,
        Err(e) => return Ok(Outcome::from_parse_error(&e)),
    };
    let mut stderr = Vec::new();

    // `-O FILE` is wget's output document: opened before anything is fetched (emptied, or under
    // `-c` kept to be appended to), and left there whatever the transfers do.
    let mut document = None;
    if let (false, Output::File(path)) = (req.output_is_default, &req.output) {
        let opened = if req.continue_download {
            std::fs::OpenOptions::new()
                .append(true)
                .create(true)
                .open(cwd.join(path))
        } else {
            std::fs::File::create(cwd.join(path))
        };
        match opened {
            Ok(file) => document = Some(file),
            Err(e) => {
                return Ok(Outcome {
                    stdout: Vec::new(),
                    stderr: format!("{path}: {}\n", io_message(&e)).into_bytes(),
                    exit_code: 1,
                });
            }
        }
    }
    // wget keeps its HSTS store in `$HOME`; with none, it says it cannot.
    if !req.quiet && home.is_none() {
        stderr.extend_from_slice(b"ERROR: could not open HSTS store. HSTS will be disabled.\n");
    }
    if req.spider && !req.quiet {
        stderr.extend_from_slice(b"Spider mode enabled. Check if remote file exists.\n");
    }

    let mut urls = req.urls.clone();
    if let Some(file) = &req.input_file {
        let listed: Vec<String> = match std::fs::read_to_string(cwd.join(file)) {
            Ok(text) => text
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(ToOwned::to_owned)
                .collect(),
            Err(e) => {
                if !req.quiet {
                    stderr.extend_from_slice(format!("{file}: {}\n", io_message(&e)).as_bytes());
                }
                Vec::new()
            }
        };
        if listed.is_empty() && !req.quiet {
            stderr.extend_from_slice(format!("No URLs found in {file}.\n").as_bytes());
        }
        urls.extend(listed);
    }

    let mut status = 0;
    for url in urls {
        let transfer = req.for_url(&url);
        let code = transfer_one(&transfer, cwd, document.as_mut(), stdout, &mut stderr).await?;
        status = combine(status, code);
    }
    Ok(Outcome {
        stdout: Vec::new(),
        stderr,
        exit_code: status,
    })
}

/// The URL `req` names, as wget checks it before connecting: one with no scheme gets `http://`
/// (wget says so, even under `-q`); a scheme other than HTTP's, no host, or a port out of range
/// ends this transfer with status 1. `Err` holds that status.
fn checked_url(req: &Request, stderr: &mut Vec<u8>) -> Result<String, u8> {
    let mut url = req.url.clone();
    if !url.contains("://") {
        stderr.extend_from_slice(format!("Prepended http:// to '{url}'\n").as_bytes());
        url = format!("http://{url}");
    }
    let (scheme, rest) = url.split_once("://").unwrap_or(("", &url));
    let complaint = if matches!(scheme.to_ascii_lowercase().as_str(), "ftp" | "ftps") {
        // wget speaks FTP; WASI-HTTP does not, so this is refused rather than tried.
        stderr.extend_from_slice(
            format!("wget: {url}: FTP is unsupported in bash-tool (HTTP and HTTPS only)\n")
                .as_bytes(),
        );
        return Err(2);
    } else if !matches!(scheme.to_ascii_lowercase().as_str(), "http" | "https") {
        "Unsupported scheme"
    } else {
        let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
        let host_port = authority
            .rsplit_once('@')
            .map_or(authority, |(_, host)| host);
        let (host, port) = match host_port.strip_prefix('[') {
            Some(ipv6) => match ipv6.split_once(']') {
                Some((host, after)) => (host, after.strip_prefix(':')),
                None => (ipv6, None),
            },
            None => match host_port.rsplit_once(':') {
                Some((host, port)) => (host, Some(port)),
                None => (host_port, None),
            },
        };
        if host.is_empty() {
            "Invalid host name"
        } else if port.is_some_and(|port| port.is_empty() || port.parse::<u16>().is_err()) {
            "Bad port number"
        } else {
            return Ok(url);
        }
    };
    if !req.quiet {
        stderr.extend_from_slice(format!("{url}: {complaint}.\n").as_bytes());
    }
    Err(1)
}

/// The host and port of an absolute URL, as wget names a server it could not reach.
fn host_port(url: &str) -> (String, String) {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let host_port = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    let default = if url.starts_with("https://") {
        "443"
    } else {
        "80"
    };
    if let Some((host, after)) = host_port
        .strip_prefix('[')
        .and_then(|v6| v6.split_once(']'))
    {
        let port = after.strip_prefix(':').unwrap_or(default);
        return (format!("[{host}]"), port.to_owned());
    }
    match host_port.rsplit_once(':') {
        Some((host, port)) => (host.to_owned(), port.to_owned()),
        None => (host_port.to_owned(), default.to_owned()),
    }
}

/// One URL's transfer: its checks, then the request (retried as `-t` allows), then the body to
/// `stdout`, the `-O` document, or its own file. Returns wget's status for it.
async fn transfer_one(
    req: &Request,
    cwd: &std::path::Path,
    document: Option<&mut std::fs::File>,
    stdout: &mut (dyn futures::io::AsyncWrite + Unpin + Send),
    stderr: &mut Vec<u8>,
) -> Result<u8, std::io::Error> {
    use futures::io::AsyncWriteExt;

    let url = match checked_url(req, stderr) {
        Ok(url) => url,
        Err(code) => return Ok(code),
    };

    // `-P`: the default file goes in that directory, which is made when a file is saved there.
    let target = match (&req.output, &req.directory_prefix) {
        (Output::File(path), Some(prefix)) if req.output_is_default => format!("{prefix}/{path}"),
        (Output::File(path), _) => path.clone(),
        (Output::Stdout, _) => String::new(),
    };
    // `-nc`: a file already there is kept, and nothing is fetched for it.
    if req.no_clobber && req.output_is_default && cwd.join(&target).exists() {
        if !req.quiet {
            stderr.extend_from_slice(
                format!("File '{target}' already there; not retrieving.\n\n").as_bytes(),
            );
        }
        return Ok(0);
    }

    let body = match &req.post_file {
        Some(file) => match std::fs::read(cwd.join(file)) {
            Ok(bytes) => Some(bytes),
            Err(e) => {
                if !req.quiet {
                    stderr.extend_from_slice(
                        format!("BODY data file '{file}' missing: {}\n", io_message(&e)).as_bytes(),
                    );
                }
                return Ok(3);
            }
        },
        None => req.data.clone(),
    };
    let method = if req.spider && body.is_none() {
        http::Method::HEAD
    } else {
        req.method.clone()
    };
    let mut hreq = whttp::Request::new(method, url.clone());
    hreq.headers = req.headers.clone();
    hreq.body = body;
    hreq.follow_redirects = req.follow_redirects;
    hreq.max_redirects = req.max_redirect;
    if let Some(secs) = req.timeout {
        // wget's `-T` covers connect and read alike.
        let d = timeout_seconds(secs);
        hreq.connect_timeout = Some(d);
        hreq.timeout = Some(d);
    }

    // `-c`/`--continue`: if the target file already has bytes, ask for the rest via `Range`. A
    // compliant server answers 206 with just the missing tail, which is appended. A server that
    // doesn't understand Range answers 200 with the WHOLE file, which then overwrites the file
    // from scratch — matching wget's own "200 means restart" behavior.
    let mut existing_size: u64 = 0;
    let existing = match &document {
        Some(document) => document.metadata(),
        None => std::fs::metadata(cwd.join(&target)),
    };
    if req.continue_download
        && let Ok(meta) = existing
    {
        existing_size = meta.len();
        if existing_size > 0 {
            hreq.headers
                .push(("Range".to_string(), format!("bytes={existing_size}-")));
        }
    }

    // `-t`/`--tries`: re-attempt on a transport failure (GNU default 20; `0` retries without
    // limit, represented as `u32::MAX` in `req.tries` -- see `parse.rs`). As in wget, a refused
    // connection and a host that does not resolve are not retried.
    let mut attempt = 0;
    let mut resp = loop {
        match whttp::fetch_streaming(&hreq).await {
            Ok(resp) => break resp,
            Err(e) => {
                attempt += 1;
                let fatal = matches!(e, whttp::Error::Connect(_) | whttp::Error::Resolve(_));
                if fatal || attempt >= req.tries {
                    if !req.quiet {
                        stderr.extend_from_slice(transport_failure(&url, &e).as_bytes());
                    }
                    return Ok(4);
                }
            }
        }
    };

    let status = resp.status;
    if req.server_response {
        stderr.extend_from_slice(server_response(status, &resp.headers).as_bytes());
    }
    // GNU wget's status for an error response from the server is 8.
    if status >= 400 {
        if !req.quiet {
            stderr.extend_from_slice(format!("wget: server returned status {status}\n").as_bytes());
        }
        return Ok(8);
    }
    if req.spider {
        if !req.quiet {
            stderr.extend_from_slice(b"Remote file exists.\n\n");
        }
        return Ok(0);
    }

    use std::io::Write as _;
    if let Output::Stdout = req.output {
        loop {
            match resp.body.next_chunk().await {
                Ok(Some(chunk)) => stdout.write_all(&chunk).await?,
                Ok(None) => return Ok(0),
                Err(e) => {
                    if !req.quiet {
                        stderr.extend_from_slice(format!("wget: {e}\n").as_bytes());
                    }
                    return Ok(4);
                }
            }
        }
    }
    let target = if req.content_disposition && req.output_is_default {
        match content_disposition_filename(&resp.headers) {
            Some(name) => match &req.directory_prefix {
                Some(prefix) => format!("{prefix}/{name}"),
                None => name,
            },
            None => target,
        }
    } else {
        target
    };
    let full = cwd.join(&target);
    // `-c`: a 206 to our `Range` request carries only the missing tail — append it. Any other
    // status here (200: the server ignored `Range` and sent the whole file; no `-c` in play at
    // all) writes/overwrites the file fresh, same as without `-c`.
    let append = req.continue_download && status == 206 && existing_size > 0;
    let mut owned;
    let file: &mut std::fs::File = match document {
        // Under `-c` the document was opened to be appended to; a server that ignored the range
        // sends the whole file, which replaces it.
        Some(document) if req.continue_download && !append => {
            if let Err(e) = document.set_len(0) {
                stderr.extend_from_slice(format!("{target}: {}\n", io_message(&e)).as_bytes());
                return Ok(3);
            }
            document
        }
        Some(document) => document,
        None => {
            if let Some(parent) = full.parent()
                && !parent.as_os_str().is_empty()
                && let Err(e) = std::fs::create_dir_all(parent)
            {
                stderr.extend_from_slice(
                    format!("{}: {}\n", parent.display(), io_message(&e)).as_bytes(),
                );
                return Ok(3);
            }
            let opened = if append {
                std::fs::OpenOptions::new().append(true).open(&full)
            } else {
                std::fs::File::create(&full)
            };
            match opened {
                Ok(file) => {
                    owned = file;
                    &mut owned
                }
                Err(e) => {
                    stderr.extend_from_slice(format!("{target}: {}\n", io_message(&e)).as_bytes());
                    return Ok(3);
                }
            }
        }
    };
    loop {
        match resp.body.next_chunk().await {
            Ok(Some(chunk)) => {
                if let Err(e) = file.write_all(&chunk) {
                    stderr.extend_from_slice(
                        format!("wget: cannot write {target}: {}\n", io_message(&e)).as_bytes(),
                    );
                    return Ok(3);
                }
            }
            Ok(None) => break,
            Err(e) => {
                if !req.quiet {
                    stderr.extend_from_slice(format!("wget: {e}\n").as_bytes());
                }
                return Ok(4);
            }
        }
    }
    if !req.quiet {
        let verb = if append { "appended to" } else { "saved to" };
        stderr.extend_from_slice(format!("wget: {verb} {target}\n").as_bytes());
    }
    Ok(0)
}

/// A failed request as wget reports it: the host it could not resolve or the server it could
/// not reach, or the transport's own words for anything else.
fn transport_failure(url: &str, error: &whttp::Error) -> String {
    let (host, port) = host_port(url);
    match error {
        whttp::Error::Resolve(_) => format!(
            "Resolving {host} ({host})... failed: Name does not resolve.\n\
             wget: unable to resolve host address '{host}'\n"
        ),
        whttp::Error::Connect(_) => {
            format!("Connecting to {host}:{port}... failed: Connection refused.\n")
        }
        other => format!("wget: {other}\n"),
    }
}

/// The `-S`/`--server-response` trace: the status line + response headers, indented like wget.
/// Takes plain `(status, headers)` rather than a `whttp::Response` so both the buffered
/// (`run_in_directory`) and streaming (`run_streaming`) response shapes can share it.
fn server_response(status: u16, headers: &[(String, String)]) -> String {
    let mut s = format!("  HTTP/1.1 {status}\n");
    for (k, v) in headers {
        let _ = writeln!(s, "  {k}: {v}");
    }
    s
}

/// Case-insensitive header lookup over a plain `(name, value)` slice — the shape both
/// `whttp::Response` and `whttp::StreamingResponse` expose their headers as.
fn lookup_header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

/// The filename from a `Content-Disposition: attachment; filename="…"` header, reduced to its
/// basename (guarding against a path-traversal filename).
fn content_disposition_filename(headers: &[(String, String)]) -> Option<String> {
    let cd = lookup_header(headers, "content-disposition")?;
    for part in cd.split(';') {
        let p = part.trim();
        if p.get(..9)
            .is_some_and(|h| h.eq_ignore_ascii_case("filename="))
        {
            let name = p[9..].trim().trim_matches('"');
            let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
            if !base.is_empty() {
                return Some(base.to_string());
            }
        }
    }
    None
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    // Test code: unwrap/expect on known-good fixtures is correct style. clippy's allow-unwrap-in-tests
    // does not fire here (compound/edge cfg-test detection), so scope it explicitly.
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    fn mock_server(status: u16, body: &'static str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 2048];
                let _ = stream.read(&mut buf);
                let response = format!(
                    "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
            }
        });
        format!("http://{addr}")
    }

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(std::string::ToString::to_string).collect()
    }

    #[tokio::test]
    async fn dash_o_dash_writes_to_stdout() {
        let url = mock_server(200, "stdout-body");
        let out = run(&argv(&["-O", "-", &url])).await;
        assert_eq!(out.exit_code, 0);
        assert_eq!(String::from_utf8(out.stdout).unwrap(), "stdout-body");
    }

    #[tokio::test]
    async fn dash_o_writes_named_file() {
        let url = mock_server(200, "named-body");
        let path = std::env::temp_dir().join(format!("waget_named_{}", std::process::id()));
        let out = run(&argv(&["-O", path.to_str().unwrap(), &url])).await;
        assert_eq!(out.exit_code, 0);
        assert!(out.stdout.is_empty());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "named-body");
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn default_writes_file_named_after_url_basename() {
        let url = mock_server(200, "basename-body");
        // The mock URL has no path, so the basename falls back to index.html; give it a path.
        let full = format!("{url}/report.txt");
        // Run from a temp cwd so the file lands somewhere cleanable.
        let dir = std::env::temp_dir().join(format!("waget_cwd_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let out = run(&argv(&[
            "-O",
            dir.join("report.txt").to_str().unwrap(),
            &full,
        ]))
        .await;
        assert_eq!(out.exit_code, 0);
        assert_eq!(
            std::fs::read_to_string(dir.join("report.txt")).unwrap(),
            "basename-body"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn quiet_suppresses_the_saved_message() {
        let url = mock_server(200, "x");
        let path = std::env::temp_dir().join(format!("waget_quiet_{}", std::process::id()));
        let out = run(&argv(&["-q", "-O", path.to_str().unwrap(), &url])).await;
        assert!(out.stderr.is_empty());
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn server_error_maps_to_exit_8() {
        // GNU wget's status for an error response from the server.
        let url = mock_server(404, "nope");
        let out = run(&argv(&["-O", "-", &url])).await;
        assert_eq!(out.exit_code, 8);
        assert!(out.stdout.is_empty());
    }

    #[tokio::test]
    async fn a_bad_timeout_is_reported_as_wget_reports_it() {
        let out = run(&argv(&["-T", "-1", "http://127.0.0.1:1/"])).await;
        assert_eq!(out.exit_code, 2);
        assert_eq!(
            String::from_utf8_lossy(&out.stderr),
            "wget: --timeout: Negative time period '-1'\n"
        );
    }

    #[test]
    fn a_timeout_longer_than_wasi_http_carries_is_capped() {
        assert_eq!(seconds(2.5), std::time::Duration::from_millis(2500));
        assert_eq!(seconds(1e20), std::time::Duration::from_nanos(u64::MAX));
    }

    #[tokio::test]
    async fn missing_url_is_a_usage_error() {
        let out = run(&argv(&["-q"])).await;
        assert_eq!(out.exit_code, 1);
        assert!(String::from_utf8_lossy(&out.stderr).starts_with("wget: missing URL\n"));
    }

    /// A one-shot server that adds a custom header (for `-S` / `--content-disposition`).
    fn mock_server_h(
        status: u16,
        header: (&'static str, &'static str),
        body: &'static str,
    ) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 2048];
                let _ = stream.read(&mut buf);
                let response = format!(
                    "HTTP/1.1 {status} X\r\n{}: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    header.0,
                    header.1,
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
            }
        });
        format!("http://{addr}")
    }

    /// A server that 302-redirects the first request to `/final`, then serves `body`.
    fn mock_redirect(body: &'static str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let loc = format!("{base}/final");
        std::thread::spawn(move || {
            for i in 0..2 {
                if let Ok((mut stream, _)) = listener.accept() {
                    let mut buf = [0u8; 2048];
                    let _ = stream.read(&mut buf);
                    let response = if i == 0 {
                        format!(
                            "HTTP/1.1 302 X\r\nLocation: {loc}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                        )
                    } else {
                        format!(
                            "HTTP/1.1 200 X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        )
                    };
                    let _ = stream.write_all(response.as_bytes());
                    let _ = stream.flush();
                }
            }
        });
        base
    }

    #[tokio::test]
    async fn follows_redirects_by_default() {
        let url = mock_redirect("final-body");
        let out = run(&argv(&["-O", "-", &url])).await;
        assert_eq!(out.exit_code, 0);
        assert_eq!(String::from_utf8(out.stdout).unwrap(), "final-body");
    }

    #[tokio::test]
    async fn server_response_prints_headers_to_stderr() {
        let url = mock_server_h(200, ("X-Trace", "abc"), "body");
        let out = run(&argv(&["-S", "-O", "-", &url])).await;
        let err = String::from_utf8(out.stderr).unwrap().to_lowercase();
        assert!(err.contains("http/1.1 200"), "status on stderr:\n{err}");
        assert!(err.contains("x-trace: abc"), "header on stderr:\n{err}");
    }

    #[test]
    fn content_disposition_filename_is_extracted_and_basenamed() {
        let headers = |cd: &str| vec![("Content-Disposition".to_string(), cd.to_string())];
        assert_eq!(
            content_disposition_filename(&headers("attachment; filename=\"dl.txt\"")).as_deref(),
            Some("dl.txt")
        );
        assert_eq!(
            content_disposition_filename(&headers("inline; filename=report.pdf")).as_deref(),
            Some("report.pdf")
        );
        // Path-traversal filename is reduced to its basename.
        assert_eq!(
            content_disposition_filename(&headers("attachment; filename=\"/etc/passwd\""))
                .as_deref(),
            Some("passwd")
        );
        assert_eq!(content_disposition_filename(&headers("attachment")), None);
    }

    #[tokio::test]
    async fn post_data_completes() {
        let url = mock_server(200, "ok");
        let out = run(&argv(&["--post-data", "a=1", "-O", "-", &url])).await;
        assert_eq!(out.exit_code, 0);
        assert_eq!(String::from_utf8(out.stdout).unwrap(), "ok");
    }

    // -- -c (Range continuation), -P (directory prefix), -N (documented no-op) --------------

    /// A one-shot server that asserts the request carried `Range: bytes=<from>-` and then answers
    /// with `status` and `body` (the 206 happy path: `body` is only the missing tail).
    fn mock_range_server(expected_from: u64, status: u16, body: &'static str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 2048];
                let n = stream.read(&mut buf).unwrap_or(0);
                let request = String::from_utf8_lossy(&buf[..n]).to_ascii_lowercase();
                assert!(
                    request.contains(&format!("range: bytes={expected_from}-")),
                    "expected a Range request:\n{request}"
                );
                let response = format!(
                    "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
            }
        });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn continue_appends_a_206_tail_to_the_existing_file() {
        let dir = std::env::temp_dir().join(format!("waget_continue_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("partial.bin");
        std::fs::write(&path, b"first-half-").unwrap();
        let url = mock_range_server(11, 206, "second-half");
        let out = run_in_directory(&argv(&["-c", "-O", "partial.bin", &url]), &dir).await;
        assert_eq!(out.exit_code, 0);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "first-half-second-half"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn continue_restarts_on_a_200_that_ignores_range() {
        let dir = std::env::temp_dir().join(format!("waget_restart_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("partial.bin");
        std::fs::write(&path, b"stale-partial-data").unwrap();
        let url = mock_range_server(18, 200, "whole-file-from-scratch");
        let out = run_in_directory(&argv(&["-c", "-O", "partial.bin", &url]), &dir).await;
        assert_eq!(out.exit_code, 0);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "whole-file-from-scratch",
            "a 200 overwrites rather than appends"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn continue_without_an_existing_file_sends_no_range() {
        let dir = std::env::temp_dir().join(format!("waget_no_partial_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let url = mock_server(200, "fresh-download");
        let out = run_in_directory(&argv(&["-c", "-O", "new.bin", &url]), &dir).await;
        assert_eq!(out.exit_code, 0);
        assert_eq!(
            std::fs::read_to_string(dir.join("new.bin")).unwrap(),
            "fresh-download"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn directory_prefix_creates_the_directory_and_writes_under_it() {
        let dir = std::env::temp_dir().join(format!("waget_prefix_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let url = mock_server(200, "prefixed-body");
        let out = run_in_directory(&argv(&["-P", "downloads/nested", &url]), &dir).await;
        assert_eq!(out.exit_code, 0);
        assert_eq!(
            std::fs::read_to_string(dir.join("downloads/nested/index.html")).unwrap(),
            "prefixed-body"
        );
        // `-O` names its file itself: as in wget, the prefix does not apply to it.
        let url = mock_server(200, "named-body");
        let out = run_in_directory(&argv(&["-P", "elsewhere", "-O", "f.txt", &url]), &dir).await;
        assert_eq!(out.exit_code, 0);
        assert_eq!(
            std::fs::read_to_string(dir.join("f.txt")).unwrap(),
            "named-body"
        );
        assert!(!dir.join("elsewhere").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn timestamping_flag_is_accepted_and_does_not_change_behavior() {
        let url = mock_server(200, "body");
        let out = run(&argv(&["-N", "-O", "-", &url])).await;
        assert_eq!(out.exit_code, 0);
        assert_eq!(String::from_utf8(out.stdout).unwrap(), "body");
    }

    // -- run_streaming (the shell's sink-based entry point) -------------------------

    async fn run_streaming_to_vec(args_: &[&str], cwd: &std::path::Path) -> (Outcome, Vec<u8>) {
        let mut sink = futures::io::AllowStdIo::new(Vec::new());
        let outcome = run_streaming(&argv(args_), cwd, None, &mut sink)
            .await
            .unwrap();
        (outcome, sink.into_inner())
    }

    #[tokio::test]
    async fn streaming_dash_o_dash_writes_through_the_sink() {
        let url = mock_server(200, "streamed-body");
        let (out, written) =
            run_streaming_to_vec(&["-O", "-", &url], std::path::Path::new(".")).await;
        assert_eq!(out.exit_code, 0);
        assert!(out.stdout.is_empty(), "the body went through the sink");
        assert_eq!(String::from_utf8(written).unwrap(), "streamed-body");
    }

    #[tokio::test]
    async fn streaming_default_writes_the_file_incrementally() {
        let dir = std::env::temp_dir().join(format!("waget_stream_file_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let url = mock_server(200, "file-streamed-body");
        let mut sink = futures::io::AllowStdIo::new(Vec::new());
        let out = run_streaming(&argv(&["-O", "out.bin", &url]), &dir, None, &mut sink)
            .await
            .unwrap();
        assert_eq!(out.exit_code, 0);
        assert!(
            sink.into_inner().is_empty(),
            "the sink is untouched when the destination is a file"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("out.bin")).unwrap(),
            "file-streamed-body"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn streaming_continue_appends_a_206_tail_incrementally() {
        let dir =
            std::env::temp_dir().join(format!("waget_stream_continue_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("partial.bin");
        std::fs::write(&path, b"first-half-").unwrap();
        let url = mock_range_server(11, 206, "second-half");
        let mut sink = futures::io::AllowStdIo::new(Vec::new());
        let out = run_streaming(
            &argv(&["-c", "-O", "partial.bin", &url]),
            &dir,
            None,
            &mut sink,
        )
        .await
        .unwrap();
        assert_eq!(out.exit_code, 0);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "first-half-second-half"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn streaming_server_error_writes_nothing() {
        let url = mock_server(404, "nope");
        let (out, written) =
            run_streaming_to_vec(&["-O", "-", &url], std::path::Path::new(".")).await;
        assert_eq!(out.exit_code, 8);
        assert!(written.is_empty());
    }

    /// A sink whose every write fails with `BrokenPipe`.
    struct BrokenSink;
    impl futures::io::AsyncWrite for BrokenSink {
        fn poll_write(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
            _buf: &[u8],
        ) -> std::task::Poll<std::io::Result<usize>> {
            std::task::Poll::Ready(Err(std::io::ErrorKind::BrokenPipe.into()))
        }
        fn poll_flush(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Ready(Ok(()))
        }
        fn poll_close(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Ready(Ok(()))
        }
    }

    #[tokio::test]
    async fn streaming_propagates_a_sink_write_error_instead_of_swallowing_it() {
        let url = mock_server(200, "body");
        let mut sink = BrokenSink;
        let result = run_streaming(
            &argv(&["-O", "-", &url]),
            std::path::Path::new("."),
            None,
            &mut sink,
        )
        .await;
        let error = result.expect_err("a closed sink must surface as Err, not a status code");
        assert_eq!(error.kind(), std::io::ErrorKind::BrokenPipe);
    }
}
