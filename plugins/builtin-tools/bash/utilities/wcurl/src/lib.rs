//! `wcurl` — a `wasm32-wasip2`-compatible `curl` clone, embeddable in any wasm component.
//!
//! Exposes an async [`run`] that takes argv (without the leading `curl` word) and returns a
//! structured [`Outcome`] (stdout bytes, stderr bytes, exit code). The HTTP transport — the
//! cfg-gated `wasi-fetch`/`reqwest` seam plus redirect following and timeouts — lives in [`whttp`];
//! this crate parses `curl` flags and formats the response.
//!
//! `run` is `async` and does NOT create its own runtime: the caller awaits it under whatever async
//! executor is running; the standalone binary provides its own executor.

mod cookies;
mod multipart;
mod parse;

use http::Method;
use std::fmt::Write as _;

pub use parse::ParseError;
use parse::Request;

/// The bindings whose wit-bindgen runtime delivers the completions of [`whttp`]'s WASI-HTTP
/// futures, for a driver that awaits [`run`] (or a shell embedding it) itself: it must do so in a
/// component-model async task, as an async export (`wasi:cli/run@0.3.0`, say) gives it. Under
/// another executor the first request panics, with no task to register its completion with.
#[cfg(target_arch = "wasm32")]
pub use wasip3;

/// Retryable HTTP statuses for `--retry` — curl's own set: request timeout, rate limited, and the
/// server-side 5xx codes usually transient (not 501 Not Implemented or 505 HTTP Version, which
/// curl also excludes since retrying changes nothing).
const RETRYABLE_STATUSES: &[u16] = &[408, 429, 500, 502, 503, 504];

/// A parsed seconds value (`-m`, `--connect-timeout`, `--retry-delay`) as a duration. WASI-HTTP
/// carries timeouts as u64 nanoseconds, so a longer one (curl accepts up to about 292 million
/// years) is capped there rather than wrapped; parsing has already refused negatives.
fn seconds(secs: f64) -> std::time::Duration {
    std::time::Duration::from_secs_f64(secs).min(std::time::Duration::from_nanos(u64::MAX))
}

/// Sleep for `duration`, on whichever target is running. Used only by `--retry-delay` — this
/// crate otherwise does no timed waiting.
async fn sleep(duration: std::time::Duration) {
    #[cfg(not(target_arch = "wasm32"))]
    {
        tokio::time::sleep(duration).await;
    }
    #[cfg(target_arch = "wasm32")]
    {
        // wasi:clocks/monotonic-clock's `wait-for`, re-exported by `wasip3` — the same async
        // runtime already driving whttp's `wasi-fetch` futures polls this one too.
        let nanos = u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX);
        wasip3::clocks::monotonic_clock::wait_for(nanos).await;
    }
}

/// Where curl sends a user after a command-line mistake.
const TRY_HELP: &str = "curl: try 'curl --help' or 'curl --manual' for more information\n";

/// `-V`/`--version`: the curl release whose behavior this one follows, and what it supports.
const VERSION: &str =
    "curl 8.22.0 (wasm32-wasip2) bash-tool\nProtocols: http https\nFeatures: libz\n";

/// The result of a `wcurl` invocation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outcome {
    /// Bytes written to standard output (the response body and any `-w` output).
    pub stdout: Vec<u8>,
    /// Bytes written to standard error (status/transport notes and any `-v` trace).
    pub stderr: Vec<u8>,
    /// The process exit code, as curl's: 0 ok (an HTTP error status included), 2 a usage error,
    /// 22 an HTTP error under `-f`, and curl's own code for each transport failure.
    pub exit_code: u8,
}

impl Outcome {
    /// A command-line mistake, as curl reports one: the problem, then where to read more.
    fn usage_error(msg: impl Into<String>) -> Self {
        Outcome {
            stdout: Vec::new(),
            stderr: format!("curl: {}\n{TRY_HELP}", msg.into()).into_bytes(),
            exit_code: 2,
        }
    }

    /// An unreadable `-d @file`/`-T`/`-F name=@file`: curl's own exit 26 (`CURLE_READ_ERROR`),
    /// not a usage error (exit 2) -- the command line was fine, reading the file failed.
    fn data_read_error(msg: impl Into<String>) -> Self {
        Outcome {
            stdout: Vec::new(),
            stderr: format!("curl: {}\n{TRY_HELP}", msg.into()).into_bytes(),
            exit_code: 26,
        }
    }

    fn transport_error(msg: impl Into<String>) -> Self {
        Outcome {
            stdout: Vec::new(),
            stderr: format!("curl: {}\n", msg.into()).into_bytes(),
            exit_code: 4,
        }
    }

    /// An error curl reports with its exit code: `curl: (CODE) message`.
    fn coded_error(code: u8, msg: impl std::fmt::Display) -> Self {
        Outcome {
            stdout: Vec::new(),
            stderr: format!("curl: ({code}) {msg}\n").into_bytes(),
            exit_code: code,
        }
    }

    /// A parse error as curl reports it: a file an option names that cannot be read is curl's
    /// own exit 26, like `-T`/`-F`'s unreadable files (see [`Self::data_read_error`]); `curl`
    /// alone only says where to read more; `-V` is the version; every other parse error is a
    /// genuine usage mistake, exit 2. Warnings curl printed on the way come first.
    fn from_parse_error(error: parse::ParseError) -> Self {
        match error {
            parse::ParseError::Warned(warnings, error) => {
                let mut outcome = Self::from_parse_error(*error);
                let mut stderr = warnings.into_bytes();
                stderr.append(&mut outcome.stderr);
                outcome.stderr = stderr;
                outcome
            }
            parse::ParseError::NoArguments => Outcome {
                stdout: Vec::new(),
                stderr: TRY_HELP.as_bytes().to_vec(),
                exit_code: 2,
            },
            parse::ParseError::Version => Outcome {
                stdout: VERSION.as_bytes().to_vec(),
                stderr: Vec::new(),
                exit_code: 0,
            },
            parse::ParseError::BadData(msg) => Self::data_read_error(msg),
            error @ parse::ParseError::ReadFile(_) => Outcome {
                exit_code: 26,
                ..Self::usage_error(error.to_string())
            },
            other => Self::usage_error(other.to_string()),
        }
    }

    /// This outcome with `warnings`, printed before it began, in front of its own diagnostics.
    fn after(mut self, mut warnings: Vec<u8>) -> Self {
        warnings.append(&mut self.stderr);
        self.stderr = warnings;
        self
    }

    /// Drops the error message, as `-s` without `-S` does.
    fn quiet(mut self, req: &parse::Request) -> Self {
        if req.silent && !req.show_error {
            self.stderr.clear();
        }
        self
    }
}

/// Checks a URL as curl does before connecting: a URL with no scheme is `http://`; any scheme
/// but HTTP's is unsupported (exit 1); a URL that does not parse is rejected (exit 3).
fn check_url(req: &mut parse::Request) -> Option<Outcome> {
    if !req.url.contains("://") {
        req.url = format!("http://{}", req.url);
    }
    let (scheme, _) = req.url.split_once("://")?;
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        let message = format!("Protocol \"{scheme}\" not supported");
        return Some(Outcome::coded_error(1, message).quiet(req));
    }
    if iri_string::types::UriStr::new(&req.url).is_err() {
        let message = "URL rejected: Malformed input to a URL function";
        return Some(Outcome::coded_error(3, message).quiet(req));
    }
    if url_port(&req.url).is_some_and(|port| port.parse::<u16>().is_err()) {
        let message = "URL rejected: Port number was not a decimal number between 0 and 65535";
        return Some(Outcome::coded_error(3, message).quiet(req));
    }
    None
}

/// The port an absolute URL names, as written (`None` when it names none).
fn url_port(url: &str) -> Option<&str> {
    let rest = url.split_once("://")?.1;
    let authority = rest.split(['/', '?', '#']).next()?;
    let host_port = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    let port = match host_port.strip_prefix('[') {
        Some(ipv6) => ipv6.split_once(']')?.1.strip_prefix(':')?,
        None => host_port.rsplit_once(':')?.1,
    };
    (!port.is_empty()).then_some(port)
}

/// Run a `curl`-style invocation. `args` is argv **without** the leading command word.
pub async fn run(args: &[String]) -> Outcome {
    run_in_directory(args, std::path::Path::new(".")).await
}

/// Resolve file operands against a shell working directory without changing process state.
/// Each URL is a transfer of its own, in order; the status is the last one's.
pub async fn run_in_directory(args: &[String], cwd: &std::path::Path) -> Outcome {
    let req = match parse::parse_at(args, cwd) {
        Ok(req) => req,
        Err(e) => return Outcome::from_parse_error(e),
    };
    let mut total = Outcome {
        stdout: Vec::new(),
        stderr: Vec::new(),
        exit_code: 0,
    };
    for transfer in req.transfers() {
        let mut outcome = transfer_buffered(transfer, cwd).await;
        total.stdout.append(&mut outcome.stdout);
        total.stderr.append(&mut outcome.stderr);
        total.exit_code = outcome.exit_code;
    }
    total
}

/// One URL's transfer, its body kept in the returned [`Outcome`].
async fn transfer_buffered(mut req: Request, cwd: &std::path::Path) -> Outcome {
    if let Some(outcome) = check_url(&mut req) {
        return outcome;
    }
    let Prepared {
        hreq,
        mut jar,
        upload_len,
        warnings,
    } = match prepare(&mut req, cwd) {
        Ok(p) => p,
        Err(outcome) => return outcome,
    };

    // `--retry`: additional attempts (beyond the first) on a transport failure or a retryable HTTP
    // status, each separated by `--retry-delay` (0 by default — curl doesn't wait unless asked).
    let started = std::time::Instant::now();
    let mut attempt = 0u32;
    let outcome = loop {
        match whttp::fetch(&hreq).await {
            Ok(resp) => {
                if attempt < req.retry && RETRYABLE_STATUSES.contains(&resp.status) {
                    attempt += 1;
                    if req.retry_delay > 0.0 {
                        sleep(seconds(req.retry_delay)).await;
                    }
                    continue;
                }
                break finish(&req, resp, cwd, &mut jar, upload_len, started.elapsed());
            }
            Err(e) => {
                if attempt < req.retry {
                    attempt += 1;
                    if req.retry_delay > 0.0 {
                        sleep(seconds(req.retry_delay)).await;
                    }
                    continue;
                }
                // The whole body is read inside `fetch`, so how much of it arrived is unknown.
                break transfer_failure(&req, &e, None, started.elapsed());
            }
        }
    };

    // `-c`/`--cookie-jar`: persist whatever ended up in the jar (loaded cookies plus anything the
    // response set), even on an HTTP error status — curl still writes the jar in that case.
    if let Some(path) = &req.cookie_jar {
        let _ = std::fs::write(cwd.join(path), cookies::write_jar(&jar));
    }
    outcome.after(warnings)
}

/// The `whttp::Request` plus everything else built while preparing it (`-b`'s jar, `-d`/`-F`/`-T`'s
/// resolved body length) — shared by [`run_in_directory`] and [`run_streaming`] so the two can
/// never drift on how a request is built. Only HOW THE RESPONSE IS CONSUMED differs between them.
struct Prepared {
    hreq: whttp::Request,
    jar: Vec<cookies::Cookie>,
    upload_len: usize,
    /// Warnings curl prints while setting the transfer up, before any diagnostic of its own.
    warnings: Vec<u8>,
}

/// curl's message for a local file (`-T`, `-F name=@file`) it cannot read when a transfer starts.
const UNREADABLE: &str = "Failed to open/read local data from file/application";

/// The file `-O` writes when the URL names none.
const NO_REMOTE_NAME: &str = "curl_response";

/// Resolve `-T`/`-F`/`-G`/`-b` into a ready-to-send [`whttp::Request`]. `req.url`/`req.headers` may
/// be mutated in place (`-T` against a trailing-slash URL appends the filename; `-F` sets
/// `Content-Type`; `-b` sets `Cookie`). Returns `Err` for a usage error that must stop the request
/// before any network access (an unreadable `-T`/`-F` file).
fn prepare(req: &mut Request, cwd: &std::path::Path) -> Result<Prepared, Outcome> {
    // `-T`: upload this file's contents as the body. A URL ending in `/` gets the file's own
    // basename appended, exactly as curl does (`-T file http://host/dir/` uploads to
    // `http://host/dir/file`).
    let mut upload_body = None;
    if let Some(path) = req.upload_file.clone() {
        match std::fs::read(cwd.join(&path)) {
            Ok(bytes) => upload_body = Some(bytes),
            Err(_) => {
                // curl names the file whatever `-s` says, then reports the transfer's failure.
                let named = format!("curl: cannot open '{path}'\n{TRY_HELP}").into_bytes();
                let failed = Outcome::coded_error(26, UNREADABLE).quiet(req);
                return Err(failed.after(named));
            }
        }
        if req.url.ends_with('/') {
            let filename = path.rsplit(['/', '\\']).next().unwrap_or(&path);
            req.url = format!("{}{filename}", req.url);
        }
    }

    // `-F`: build the multipart body and Content-Type now, once, so a `--retry` loop below sends
    // an identical body on every attempt rather than re-reading `@file` fields from disk each time
    // (which could observe a different file if it changes mid-retry).
    let mut form_body = None;
    if !req.forms.is_empty() {
        match multipart::build(&req.forms, cwd) {
            Ok((bytes, content_type)) => {
                ensure_header(&mut req.headers, "Content-Type", &content_type);
                form_body = Some(bytes);
            }
            Err(_) => return Err(Outcome::coded_error(26, UNREADABLE).quiet(req)),
        }
    }

    // Where the body will go is settled before connecting: `--create-dirs` makes `-o`'s
    // directories even when the transfer then fails, and `-O` on a URL with no file name falls
    // back to curl's own name, with a warning.
    let mut warnings = Vec::new();
    if req.create_dirs
        && let Some(path) = &req.output
        && let Some(parent) = cwd.join(path).parent()
        && !parent.as_os_str().is_empty()
        && let Err(e) = std::fs::create_dir_all(parent)
    {
        return Err(Outcome::transport_error(format!(
            "--create-dirs: {}: {e}",
            parent.display()
        )));
    }
    if req.remote_name
        && !req.remote_header_name
        && req.output.is_none()
        && remote_filename(&req.url).is_none()
    {
        if !req.silent {
            warnings.extend_from_slice(
                format!("Warning: No remote filename, uses \"{NO_REMOTE_NAME}\"\n").as_bytes(),
            );
        }
        req.output = Some(NO_REMOTE_NAME.to_owned());
        req.remote_name = false;
    }

    // `-G`: fold any `-d` data into the query string of a GET and send no body.
    let (url, body) = if req.get {
        (request_url(req), None)
    } else if let Some(bytes) = upload_body {
        (req.url.clone(), Some(bytes))
    } else if let Some(bytes) = form_body {
        (req.url.clone(), Some(bytes))
    } else {
        (req.url.clone(), req.data.clone())
    };
    let upload_len = body.as_ref().map_or(0, Vec::len);

    // `-b`/`--cookie`: load the jar (or parse a literal `name=value[; ...]` string) and add
    // whatever in it matches this request's host/path/scheme as a `Cookie` header.
    // A literal string is only sent: as in curl, it never joins the jar `-c` writes.
    let (jar, sent) = match &req.cookie {
        Some(value) if cookies::is_literal(value) => {
            let literal = cookies::parse_literal(
                value,
                url_host(&url).unwrap_or_default().as_str(),
                url_path(&url).as_str(),
            );
            (Vec::new(), literal)
        }
        Some(path) => match std::fs::read_to_string(cwd.join(path)) {
            Ok(text) => {
                let loaded = cookies::parse_jar(&text);
                (loaded.clone(), loaded)
            }
            // A missing jar file is not an error — curl treats "nothing to load yet" as normal
            // (the very first `-b -c same.txt` run has no file to read).
            Err(_) => (Vec::new(), Vec::new()),
        },
        None => (Vec::new(), Vec::new()),
    };
    if req.cookie.is_some()
        && let Some(cookie_header) = cookies::header_value(&sent, &url)
    {
        ensure_header(&mut req.headers, "Cookie", &cookie_header);
    }

    let mut hreq = whttp::Request::new(req.method.clone(), url);
    hreq.headers = req.headers.clone();
    hreq.body = body;
    hreq.follow_redirects = req.location;
    // Pin curl's redirect cap explicitly rather than inheriting `whttp::Request::new`'s default —
    // otherwise `curl -L`'s bound is an invisible dependency on a constant in another crate, and it
    // silently diverges from waget (which sets 20). 50 matches curl's own default (audit P3-5).
    hreq.max_redirects = 50;
    hreq.connect_timeout = req.connect_timeout.map(seconds);
    // `-m 0` means no timeout, not a zero-length one (`seconds(0.0)` would give `Duration::ZERO`,
    // timing out before the request could even start).
    hreq.timeout = req.max_time.map(|t| {
        if t == 0.0 {
            whttp::NO_TIMEOUT
        } else {
            seconds(t)
        }
    });
    hreq.compressed = req.compressed;

    Ok(Prepared {
        hreq,
        jar,
        upload_len,
        warnings,
    })
}

/// Like [`run_in_directory`], but the response body is forwarded chunk-by-chunk instead of
/// buffered: to `stdout` (curl's default, `-o -` is not a curl spelling but `stdout` IS the
/// no-`-o`/no`-O` default) or, under `-o`/`-O`, incrementally into the target file. Written for
/// the shell (`execute_http`), which owns the command's real stdout; the standalone binary keeps
/// using the buffered [`run_in_directory`], which needs no sink.
///
/// Returns `Err` only for a WRITE failure on `stdout` (the sink) — a closed pipe. The caller
/// decides what that means (the cooperative shell turns an ignored SIGPIPE into curl's own exit
/// 23; the default disposition just lets the error propagate). Every other failure (a usage error,
/// a transport error, a local file-write error under `-o`) is reported through the returned
/// `Outcome` as usual, never as `Err`.
///
/// With several URLs, each but the last has its `-w` output follow its body through the sink,
/// before the next URL's body; the last one's comes back in the `Outcome`, for the caller to write
/// after the diagnostics, as curl's buffered stdout shows it after its unbuffered stderr.
///
/// `--compressed` cannot be decoded chunk-at-a-time (`whttp::fetch_streaming`'s doc explains why)
/// — that one case falls back to the fully buffered transfer and writes its single payload
/// through the sink in one call, still honoring the "forward through the sink, don't return it in
/// `Outcome`" contract.
///
/// # Errors
///
/// Returns the [`std::io::Error`] from the first failed write to `stdout`.
pub async fn run_streaming(
    args: &[String],
    cwd: &std::path::Path,
    stdout: &mut (dyn futures::io::AsyncWrite + Unpin + Send),
) -> Result<Outcome, std::io::Error> {
    use futures::io::AsyncWriteExt;

    let req = match parse::parse_at(args, cwd) {
        Ok(req) => req,
        Err(e) => return Ok(Outcome::from_parse_error(e)),
    };
    // Each URL is a transfer of its own, in order, its `-w` output after its body; the status is
    // the last one's.
    let mut total = Outcome {
        stdout: Vec::new(),
        stderr: Vec::new(),
        exit_code: 0,
    };
    for transfer in req.transfers() {
        stdout.write_all(&total.stdout).await?;
        let mut outcome = transfer_streaming(transfer, cwd, stdout).await?;
        total.stdout = outcome.stdout;
        total.stderr.append(&mut outcome.stderr);
        total.exit_code = outcome.exit_code;
    }
    Ok(total)
}

/// One URL's transfer, its body forwarded to `stdout` as it arrives (see [`run_streaming`]).
async fn transfer_streaming(
    mut req: Request,
    cwd: &std::path::Path,
    stdout: &mut (dyn futures::io::AsyncWrite + Unpin + Send),
) -> Result<Outcome, std::io::Error> {
    use futures::io::AsyncWriteExt;

    if let Some(outcome) = check_url(&mut req) {
        return Ok(outcome);
    }

    if req.compressed {
        let buffered = transfer_buffered(req, cwd).await;
        stdout.write_all(&buffered.stdout).await?;
        return Ok(Outcome {
            stdout: Vec::new(),
            stderr: buffered.stderr,
            exit_code: buffered.exit_code,
        });
    }

    let mut prepared = match prepare(&mut req, cwd) {
        Ok(p) => p,
        Err(outcome) => return Ok(outcome),
    };
    let warnings = std::mem::take(&mut prepared.warnings);
    Ok(stream_prepared(&req, prepared, cwd, stdout)
        .await?
        .after(warnings))
}

/// The prepared request sent, retried as `--retry` asks, and its response written out.
async fn stream_prepared(
    req: &Request,
    prepared: Prepared,
    cwd: &std::path::Path,
    stdout: &mut (dyn futures::io::AsyncWrite + Unpin + Send),
) -> Result<Outcome, std::io::Error> {
    use futures::io::AsyncWriteExt;

    let Prepared {
        hreq,
        mut jar,
        upload_len,
        ..
    } = prepared;
    let started = std::time::Instant::now();
    let mut attempt = 0u32;
    // Nothing is read from a `StreamingResponse`'s body until AFTER this loop decides not to
    // retry, so a retry here never re-sends (or drops) a body byte already forwarded — the "a
    // retry may only happen before any body byte has been written" rule.
    let mut resp = loop {
        match whttp::fetch_streaming(&hreq).await {
            Ok(resp) => {
                if attempt < req.retry && RETRYABLE_STATUSES.contains(&resp.status) {
                    attempt += 1;
                    if req.retry_delay > 0.0 {
                        sleep(seconds(req.retry_delay)).await;
                    }
                    continue;
                }
                break resp;
            }
            Err(e) => {
                if attempt < req.retry {
                    attempt += 1;
                    if req.retry_delay > 0.0 {
                        sleep(seconds(req.retry_delay)).await;
                    }
                    continue;
                }
                // curl writes the jar `-c` names even when the transfer failed.
                if let Some(path) = &req.cookie_jar {
                    let _ = std::fs::write(cwd.join(path), cookies::write_jar(&jar));
                }
                return Ok(transfer_failure(req, &e, Some(0), started.elapsed()));
            }
        }
    };

    // Cookie engine: same as the buffered path (see `finish`), just against `StreamingResponse`.
    if req.cookie.is_some() || req.cookie_jar.is_some() {
        let set_cookie: Vec<&str> = resp
            .headers
            .iter()
            .filter(|(k, _)| k.eq_ignore_ascii_case("set-cookie"))
            .map(|(_, v)| v.as_str())
            .collect();
        cookies::merge_set_cookie(&mut jar, &resp.final_url, &set_cookie);
    }
    if let Some(path) = &req.cookie_jar {
        let _ = std::fs::write(cwd.join(path), cookies::write_jar(&jar));
    }

    let status = resp.status;
    let elapsed = started.elapsed();
    let mut stderr = Vec::new();
    if req.verbose {
        stderr.extend_from_slice(
            verbose_trace(
                req.method.as_str(),
                &req.url,
                &req.headers,
                status,
                &resp.headers,
            )
            .as_bytes(),
        );
    }

    // -f/--fail: no body on an HTTP error; exit 22. The body is still drained (never written
    // anywhere) so the connection completes cleanly and -w's size_download stays meaningful.
    if req.fail && !req.fail_with_body && status >= 400 {
        let size_download = match drain_counting(&mut resp.body).await {
            Ok(size) => size,
            Err((error, received)) => {
                return Ok(transfer_failure(
                    req,
                    &error,
                    Some(received),
                    started.elapsed(),
                ));
            }
        };
        if !req.silent || req.show_error {
            stderr.extend_from_slice(
                format!("curl: (22) The requested URL returned error: {status}\n").as_bytes(),
            );
        }
        let mut stdout_extra = Vec::new();
        if let Some(fmt) = &req.write_out {
            let ctx = WriteOutContext {
                status,
                headers: &resp.headers,
                final_url: &resp.final_url,
                num_redirects: resp.num_redirects,
                size_download,
                exit_code: 22,
                method: &req.method,
                upload_len,
                elapsed,
                errormsg: "",
            };
            stdout_extra.extend_from_slice(write_out(fmt, &ctx, &mut stderr).as_bytes());
        }
        return Ok(Outcome {
            stdout: stdout_extra,
            stderr,
            exit_code: 22,
        });
    }

    // Where to write: an explicit `-o` wins; else `-O`/`--remote-name` derives a filename from the
    // URL (refined by `-J`'s `Content-Disposition`, if present); else stdout (the sink).
    let target = req.output.clone().or_else(|| {
        req.remote_name.then(|| {
            if req.remote_header_name {
                content_disposition_filename(&resp.headers)
                    .or_else(|| remote_filename(&req.url))
                    .unwrap_or_else(|| NO_REMOTE_NAME.to_owned())
            } else {
                remote_filename(&req.url).unwrap_or_else(|| NO_REMOTE_NAME.to_owned())
            }
        })
    });

    let header_bytes =
        (req.include || req.head).then(|| header_block(status, &resp.headers).into_bytes());
    let size_download = match &target {
        Some(path) => {
            let full = cwd.join(path);
            if req.create_dirs
                && let Some(parent) = full.parent()
                && !parent.as_os_str().is_empty()
                && let Err(e) = std::fs::create_dir_all(parent)
            {
                return Ok(Outcome::transport_error(format!(
                    "--create-dirs: {}: {e}",
                    parent.display()
                )));
            }
            match write_streamed_to_file(&full, header_bytes.as_deref(), req.head, &mut resp.body)
                .await
            {
                Ok(n) => n,
                Err(BodyError::Read(error, received)) => {
                    return Ok(transfer_failure(
                        req,
                        &error,
                        Some(received),
                        started.elapsed(),
                    ));
                }
                Err(BodyError::Write(e)) => {
                    return Ok(Outcome::transport_error(format!(
                        "cannot write {}: {e}",
                        path
                    )));
                }
            }
        }
        None => {
            if let Some(bytes) = &header_bytes {
                stdout.write_all(bytes).await?;
            }
            let mut total = 0usize;
            if !req.head {
                // A failed read ends the transfer as curl ends it (`-m` expiring mid-body is
                // exit 28); only a failed write to `stdout` is this function's `Err`.
                loop {
                    match resp.body.next_chunk().await {
                        Ok(Some(chunk)) => {
                            total += chunk.len();
                            stdout.write_all(&chunk).await?;
                        }
                        Ok(None) => break,
                        Err(error) => {
                            return Ok(transfer_failure(
                                req,
                                &error,
                                Some(total),
                                started.elapsed(),
                            ));
                        }
                    }
                }
            }
            total
        }
    };

    // An HTTP error status is a transfer that worked, as in curl: only `-f` and `--fail-with-body`
    // (which still writes the body) make it a failure.
    let exit_code = if req.fail_with_body && status >= 400 {
        22
    } else {
        0
    };
    let mut stdout_extra = Vec::new();
    if let Some(fmt) = &req.write_out {
        let ctx = WriteOutContext {
            status,
            headers: &resp.headers,
            final_url: &resp.final_url,
            num_redirects: resp.num_redirects,
            size_download,
            exit_code,
            method: &req.method,
            upload_len,
            elapsed,
            errormsg: "",
        };
        stdout_extra.extend_from_slice(write_out(fmt, &ctx, &mut stderr).as_bytes());
    }
    if req.fail_with_body && status >= 400 && (!req.silent || req.show_error) {
        stderr.extend_from_slice(
            format!("curl: (22) The requested URL returned error: {status}\n").as_bytes(),
        );
    }
    Ok(Outcome {
        stdout: stdout_extra,
        stderr,
        exit_code,
    })
}

/// Write an optional header block plus (unless `head_only`) the body stream to a local file,
/// returning the number of body bytes written. A plain, synchronous `std::fs::File` — local file
/// I/O is the same call on every target this crate builds for, so it needs no `AsyncWrite` sink.
async fn write_streamed_to_file(
    path: &std::path::Path,
    header_bytes: Option<&[u8]>,
    head_only: bool,
    body: &mut whttp::BodyStream,
) -> Result<usize, BodyError> {
    use std::io::Write as _;
    let mut file = std::fs::File::create(path).map_err(BodyError::Write)?;
    if let Some(bytes) = header_bytes {
        file.write_all(bytes).map_err(BodyError::Write)?;
    }
    let mut total = 0usize;
    if !head_only {
        loop {
            match body.next_chunk().await {
                Ok(Some(chunk)) => {
                    total += chunk.len();
                    file.write_all(&chunk).map_err(BodyError::Write)?;
                }
                Ok(None) => break,
                Err(error) => return Err(BodyError::Read(error, total)),
            }
        }
    }
    Ok(total)
}

/// Why writing a streamed body out stopped: reading it failed after this many bytes, or writing
/// it did.
enum BodyError {
    Read(whttp::Error, usize),
    Write(std::io::Error),
}

/// Read `body` to completion without keeping any of it, returning the total byte count — used by
/// `-f`'s error path, which reports `size_download` but writes no body anywhere. A failed read
/// comes back with the bytes read before it.
async fn drain_counting(body: &mut whttp::BodyStream) -> Result<usize, (whttp::Error, usize)> {
    let mut total = 0usize;
    loop {
        match body.next_chunk().await {
            Ok(Some(chunk)) => total += chunk.len(),
            Ok(None) => return Ok(total),
            Err(error) => return Err((error, total)),
        }
    }
}

/// A transfer that `error` ended, as curl reports it: the time budget running out is exit 28
/// (`Operation timed out after N milliseconds with M bytes received`, M left out where the
/// buffered path cannot know it), a body cut off while arriving is exit 56; anything else keeps
/// this crate's transport failure (exit 4). `-s` hides the message; `-w` still runs.
fn transfer_failure(
    req: &parse::Request,
    error: &whttp::Error,
    received: Option<usize>,
    elapsed: std::time::Duration,
) -> Outcome {
    let (code, message) = match error {
        whttp::Error::Timeout => {
            let received =
                received.map_or_else(String::new, |n| format!(" with {n} bytes received"));
            (
                28,
                format!(
                    "Operation timed out after {} milliseconds{received}",
                    elapsed.as_millis()
                ),
            )
        }
        whttp::Error::Transport(_) if received.is_some_and(|n| n > 0) => {
            (56, "Failure when receiving data from the peer".to_owned())
        }
        whttp::Error::Resolve(_) => (
            6,
            format!(
                "Could not resolve host: {}",
                url_host(&req.url).unwrap_or_default()
            ),
        ),
        whttp::Error::Connect(_) => (
            7,
            format!(
                "Failed to connect to {} after {} ms: Could not connect to server",
                url_host_port(&req.url),
                elapsed.as_millis()
            ),
        ),
        whttp::Error::Certificate(detail) => {
            let insecure = if req.insecure {
                "; --insecure is unsupported in bash-tool, which always verifies certificates"
            } else {
                ""
            };
            (60, format!("SSL certificate problem: {detail}{insecure}"))
        }
        other => (4, other.to_string()),
    };
    let mut outcome = if code == 4 {
        Outcome::transport_error(message.clone())
    } else {
        Outcome::coded_error(code, &message)
    }
    .quiet(req);
    // curl still runs -w on a transport failure (errormsg/exitcode set, everything
    // response-shaped empty).
    if let Some(fmt) = &req.write_out {
        let expanded = write_out_error(fmt, &request_url(req), &message, code, &mut outcome.stderr);
        outcome.stdout.extend_from_slice(expanded.as_bytes());
    }
    outcome
}

/// The URL a request goes to: under `-G`, with its data as the query.
fn request_url(req: &Request) -> String {
    match (&req.data, req.get) {
        (Some(data), true) => {
            let sep = if req.url.contains('?') { '&' } else { '?' };
            format!("{}{sep}{}", req.url, String::from_utf8_lossy(data))
        }
        _ => req.url.clone(),
    }
}

/// Add `(name, value)` only if no header with that name (case-insensitive) is already present —
/// an explicit `-H` always wins over one this crate would otherwise synthesize.
fn ensure_header(headers: &mut Vec<(String, String)>, name: &str, value: &str) {
    if !headers.iter().any(|(k, _)| k.eq_ignore_ascii_case(name)) {
        headers.push((name.to_string(), value.to_string()));
    }
}

/// The lowercased host of an absolute URL, for cookie scoping. `None` if it won't parse.
fn url_host(url: &str) -> Option<String> {
    use iri_string::types::UriAbsoluteStr;
    let uri = UriAbsoluteStr::new(url.split('#').next().unwrap_or(url)).ok()?;
    Some(uri.authority_components()?.host().to_ascii_lowercase())
}

/// `host:port` of an absolute URL as curl names a server it could not connect to: the port the
/// URL gives, or its scheme's.
fn url_host_port(url: &str) -> String {
    use iri_string::types::UriAbsoluteStr;
    let Ok(uri) = UriAbsoluteStr::new(url.split('#').next().unwrap_or(url)) else {
        return String::new();
    };
    let Some(authority) = uri.authority_components() else {
        return String::new();
    };
    let port = authority
        .port()
        .filter(|port| !port.is_empty())
        .map_or_else(
            || {
                if uri.scheme_str().eq_ignore_ascii_case("https") {
                    "443"
                } else {
                    "80"
                }
                .to_owned()
            },
            ToOwned::to_owned,
        );
    format!("{}:{port}", authority.host())
}

/// The path of an absolute URL (`/` if none), for cookie scoping.
fn url_path(url: &str) -> String {
    use iri_string::types::UriAbsoluteStr;
    UriAbsoluteStr::new(url.split('#').next().unwrap_or(url))
        .ok()
        .map(|uri| {
            let path = uri.path_str();
            if path.is_empty() {
                "/".to_string()
            } else {
                path.to_string()
            }
        })
        .unwrap_or_else(|| "/".to_string())
}

/// Format a fetch into an [`Outcome`], honoring the output-shaping flags. Without `-f`, a client/
/// server error status (>= 400) writes the body and exits 0, as curl does; `-f` suppresses the
/// body and yields curl's exit 22, and `--fail-with-body` writes it and yields 22. `jar` is updated from the response's `Set-Cookie` headers in place
/// (the caller writes it out under `-c` after this returns).
// resp is a private helper's owned response; taking it by value keeps the single call site simple.
#[allow(clippy::needless_pass_by_value)]
fn finish(
    req: &Request,
    resp: whttp::Response,
    cwd: &std::path::Path,
    jar: &mut Vec<cookies::Cookie>,
    upload_len: usize,
    elapsed: std::time::Duration,
) -> Outcome {
    let status = resp.status;
    let mut stdout: Vec<u8> = Vec::new();
    let mut stderr: Vec<u8> = Vec::new();

    // Cookie engine: merge whatever this response set, scoped to where it actually came from
    // (after redirects, `final_url`) — done whenever `-b` or `-c` turned the engine on, matching
    // curl (either flag alone still tracks cookies for the rest of the invocation).
    if req.cookie.is_some() || req.cookie_jar.is_some() {
        let set_cookie: Vec<&str> = resp
            .headers
            .iter()
            .filter(|(k, _)| k.eq_ignore_ascii_case("set-cookie"))
            .map(|(_, v)| v.as_str())
            .collect();
        cookies::merge_set_cookie(jar, &resp.final_url, &set_cookie);
    }

    if req.verbose {
        stderr.extend_from_slice(
            verbose_trace(
                req.method.as_str(),
                &req.url,
                &req.headers,
                status,
                &resp.headers,
            )
            .as_bytes(),
        );
    }

    // -f/--fail: no body on an HTTP error; exit 22.
    if req.fail && !req.fail_with_body && status >= 400 {
        if !req.silent || req.show_error {
            stderr.extend_from_slice(
                format!("curl: (22) The requested URL returned error: {status}\n").as_bytes(),
            );
        }
        if let Some(fmt) = &req.write_out {
            let ctx = WriteOutContext {
                status,
                headers: &resp.headers,
                final_url: &resp.final_url,
                num_redirects: resp.num_redirects,
                size_download: resp.body.len(),
                exit_code: 22,
                method: &req.method,
                upload_len,
                elapsed,
                errormsg: "",
            };
            stdout.extend_from_slice(write_out(fmt, &ctx, &mut stderr).as_bytes());
        }
        return Outcome {
            stdout,
            stderr,
            exit_code: 22,
        };
    }

    // The payload: optional header block (-i, or -I which is headers-only), then the body unless
    // this was a HEAD.
    let mut payload: Vec<u8> = Vec::new();
    if req.include || req.head {
        payload.extend_from_slice(header_block(status, &resp.headers).as_bytes());
    }
    if !req.head {
        payload.extend_from_slice(&resp.body);
    }

    // Where to write: an explicit `-o` wins; else `-O`/`--remote-name` derives a filename from the
    // URL (refined by `-J`'s `Content-Disposition`, if present); else stdout.
    let target = req.output.clone().or_else(|| {
        req.remote_name.then(|| {
            if req.remote_header_name {
                content_disposition_filename(&resp.headers)
                    .or_else(|| remote_filename(&req.url))
                    .unwrap_or_else(|| NO_REMOTE_NAME.to_owned())
            } else {
                remote_filename(&req.url).unwrap_or_else(|| NO_REMOTE_NAME.to_owned())
            }
        })
    });
    match target {
        Some(path) => {
            let full = cwd.join(&path);
            if req.create_dirs
                && let Some(parent) = full.parent()
                && !parent.as_os_str().is_empty()
                && let Err(e) = std::fs::create_dir_all(parent)
            {
                return Outcome::transport_error(format!(
                    "--create-dirs: {}: {e}",
                    parent.display()
                ));
            }
            if let Err(e) = std::fs::write(&full, &payload) {
                return Outcome::transport_error(format!("cannot write {path}: {e}"));
            }
        }
        None => stdout.extend_from_slice(&payload),
    }

    // An HTTP error status is a transfer that worked, as in curl: only `-f` and `--fail-with-body`
    // (which still writes the body) make it a failure.
    let exit_code = if req.fail_with_body && status >= 400 {
        22
    } else {
        0
    };

    // -w/--write-out is appended to stdout after the transfer.
    if let Some(fmt) = &req.write_out {
        let ctx = WriteOutContext {
            status,
            headers: &resp.headers,
            final_url: &resp.final_url,
            num_redirects: resp.num_redirects,
            size_download: resp.body.len(),
            exit_code,
            method: &req.method,
            upload_len,
            elapsed,
            errormsg: "",
        };
        stdout.extend_from_slice(write_out(fmt, &ctx, &mut stderr).as_bytes());
    }

    // The status is already visible under -i/-I, so only surface the stderr note otherwise.
    if req.fail_with_body && status >= 400 && (!req.silent || req.show_error) {
        stderr.extend_from_slice(
            format!("curl: (22) The requested URL returned error: {status}\n").as_bytes(),
        );
    }
    Outcome {
        stdout,
        stderr,
        exit_code,
    }
}

/// `-O`/`--remote-name`'s filename: the URL's last non-empty path segment, or `None` when its
/// path has none (curl then writes `curl_response`, see [`prepare`]).
fn remote_filename(url: &str) -> Option<String> {
    let without_scheme = url.split("://").nth(1).unwrap_or(url);
    let location = without_scheme.split(['?', '#']).next().unwrap_or("");
    // What precedes the first `/` is the host, never a file name.
    let path = location.split_once('/').map_or("", |(_, path)| path);
    path.rsplit('/')
        .find(|seg| !seg.is_empty())
        .map(ToOwned::to_owned)
}

/// Case-insensitive header lookup over a plain `(name, value)` slice — the shape both
/// `whttp::Response` and `whttp::StreamingResponse` expose their headers as, so this one helper
/// serves both without needing either response type by name.
fn lookup_header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

/// `-J`'s filename source: a `Content-Disposition: attachment; filename="…"` header, reduced to
/// its basename (guards against a path-traversal filename) — mirrors `waget`'s equivalent.
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

/// The status line + response headers, as curl prints them under `-i`/`-I` (CRLF, blank line
/// last). Takes plain `(status, headers)` rather than a `whttp::Response` so both the buffered and
/// streaming response shapes can share it.
fn header_block(status: u16, headers: &[(String, String)]) -> String {
    let mut s = format!("HTTP/1.1 {status}\r\n");
    for (k, v) in headers {
        let _ = write!(s, "{k}: {v}\r\n");
    }
    s.push_str("\r\n");
    s
}

/// A `-v` trace: request line + request headers (`>`), then response status + headers (`<`).
///
/// Credential header values are masked while their names remain visible in verbose output.
fn verbose_trace(
    method: &str,
    url: &str,
    request_headers: &[(String, String)],
    status: u16,
    response_headers: &[(String, String)],
) -> String {
    let mut s = format!("> {method} {url}\n");
    for (k, v) in request_headers {
        if whttp::is_credential_header(k) {
            let _ = writeln!(s, "> {k}: <redacted>");
        } else {
            let _ = writeln!(s, "> {k}: {v}");
        }
    }
    let _ = writeln!(s, "< HTTP/1.1 {status}");
    for (k, v) in response_headers {
        let _ = writeln!(s, "< {k}: {v}");
    }
    s
}

/// Everything a `-w`/`--write-out` format string can reference. Plain fields (not a
/// `whttp::Response`) so the buffered and streaming paths — which have genuinely different
/// response shapes — can build the same context.
struct WriteOutContext<'a> {
    status: u16,
    headers: &'a [(String, String)],
    final_url: &'a str,
    num_redirects: u32,
    /// Response body bytes — `resp.body.len()` on the buffered path, a running counter on the
    /// streaming path (which never holds the whole body at once).
    size_download: usize,
    /// The exit code THIS transfer will finish with — known before the caller returns it, since
    /// `-w` is curl's own last step, after the body/status decision but still part of one exit.
    exit_code: u8,
    method: &'a Method,
    /// Bytes sent as the request body (`-d`, `-F`, `-T`) — curl's `%{size_upload}`.
    upload_len: usize,
    /// Wall-clock time for the whole `whttp::fetch`/`fetch_streaming` call (all attempts/redirects
    /// included). Monotonic only — see the module doc; every `time_*` variable reports THIS value.
    elapsed: std::time::Duration,
    /// `%{errormsg}`: set only when this "response" stands in for a transport failure
    /// (`write_out_error`) — every other caller has no error to report.
    errormsg: &'a str,
}

/// Expand a `-w`/`--write-out` format string: `%{var}` variables (including `%{header{Name}}`),
/// `%%`, and `\n`/`\t`/`\r` escapes. An unknown variable expands to nothing, and curl's warning
/// about it goes to `stderr`, whatever `-s` says.
fn write_out(fmt: &str, ctx: &WriteOutContext<'_>, stderr: &mut Vec<u8>) -> String {
    let mut out = String::new();
    let mut chars = fmt.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('r') => out.push('\r'),
                Some(other) => out.push(other),
                None => out.push('\\'),
            },
            '%' if chars.peek() == Some(&'%') => {
                chars.next();
                out.push('%');
            }
            '%' if chars.peek() == Some(&'{') => {
                chars.next();
                // Brace-depth tracked (not "stop at the first `}`") so `%{header{Name}}` keeps its
                // inner `header{Name}` intact instead of truncating at the wrong `}`.
                let mut var = String::new();
                let mut depth = 1i32;
                for c in chars.by_ref() {
                    match c {
                        '{' => depth += 1,
                        '}' => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                    var.push(c);
                }
                match write_out_var(&var, ctx) {
                    Some(value) => out.push_str(&value),
                    None => stderr.extend_from_slice(
                        format!("curl: unknown --write-out variable: '{var}'\n").as_bytes(),
                    ),
                }
            }
            other => out.push(other),
        }
    }
    out
}

/// The curl `-w` variable set this crate implements. Every `time_*` variable reports the SAME
/// measured duration (`ctx.elapsed`) except `time_redirect`, which is 0 without redirects and
/// otherwise also `ctx.elapsed` — this crate has no per-phase (DNS/connect/TLS/first-byte) timing
/// hooks into either transport, so a single wall-clock measurement stands in for all of curl's
/// finer-grained phases. `%{json}` and `%output{...}` are not implemented (see the module doc).
fn write_out_var(var: &str, ctx: &WriteOutContext<'_>) -> Option<String> {
    if let Some(name) = var
        .strip_prefix("header{")
        .and_then(|s| s.strip_suffix('}'))
    {
        return Some(lookup_header(ctx.headers, name).unwrap_or("").to_string());
    }
    Some(match var {
        // Three digits, as curl prints it: `000` when there was no response.
        "http_code" | "response_code" => format!("{:03}", ctx.status),
        "size_download" => ctx.size_download.to_string(),
        "size_upload" => ctx.upload_len.to_string(),
        "size_header" => header_block(ctx.status, ctx.headers).len().to_string(),
        "url_effective" => ctx.final_url.to_string(),
        "content_type" => lookup_header(ctx.headers, "content-type")
            .unwrap_or("")
            .to_string(),
        "num_redirects" => ctx.num_redirects.to_string(),
        "redirect_url" => {
            if matches!(ctx.status, 301 | 302 | 303 | 307 | 308) {
                lookup_header(ctx.headers, "location")
                    .unwrap_or("")
                    .to_string()
            } else {
                String::new()
            }
        }
        "method" => ctx.method.to_string(),
        "scheme" => ctx
            .final_url
            .split_once("://")
            .map_or("", |(scheme, _)| scheme)
            .to_ascii_uppercase(),
        "exitcode" => ctx.exit_code.to_string(),
        "errormsg" => ctx.errormsg.to_string(),
        "time_total" | "time_namelookup" | "time_connect" | "time_appconnect"
        | "time_pretransfer" | "time_starttransfer" => format!("{:.6}", ctx.elapsed.as_secs_f64()),
        "time_redirect" => {
            if ctx.num_redirects > 0 {
                format!("{:.6}", ctx.elapsed.as_secs_f64())
            } else {
                "0.000000".to_string()
            }
        }
        _ => return None,
    })
}

/// A `-w` expansion for a transport-level failure (curl still runs `-w` in this case, with
/// `errormsg`/`exitcode` set and everything response-shaped empty/zero).
fn write_out_error(
    fmt: &str,
    url: &str,
    message: &str,
    exit_code: u8,
    stderr: &mut Vec<u8>,
) -> String {
    write_out(
        fmt,
        &WriteOutContext {
            status: 0,
            headers: &[],
            final_url: url,
            num_redirects: 0,
            size_download: 0,
            exit_code,
            method: &Method::GET,
            upload_len: 0,
            elapsed: std::time::Duration::ZERO,
            errormsg: message,
        },
        stderr,
    )
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    // Test code: unwrap/expect on known-good fixtures is correct style. clippy's allow-unwrap-in-tests
    // does not fire here (compound/edge cfg-test detection), so scope it explicitly.
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    #[test]
    fn a_timeout_longer_than_wasi_http_carries_is_capped() {
        assert_eq!(seconds(2.5), std::time::Duration::from_millis(2500));
        assert_eq!(
            seconds(9_223_372_036_854_774.0),
            std::time::Duration::from_nanos(u64::MAX)
        );
    }

    // `-m 0` means "no timeout" in curl, not a zero-length one that would time out before
    // the request could even start.
    #[test]
    fn max_time_zero_means_no_timeout_not_a_zero_length_one() {
        let mut req = crate::parse::parse(&[
            "-m".to_string(),
            "0".to_string(),
            "https://example.com".to_string(),
        ])
        .unwrap();
        let prepared = prepare(&mut req, std::path::Path::new(".")).unwrap();
        assert_eq!(prepared.hreq.timeout, Some(whttp::NO_TIMEOUT));
    }

    #[test]
    fn max_time_nonzero_is_unaffected() {
        let mut req = crate::parse::parse(&[
            "-m".to_string(),
            "5".to_string(),
            "https://example.com".to_string(),
        ])
        .unwrap();
        let prepared = prepare(&mut req, std::path::Path::new(".")).unwrap();
        assert_eq!(
            prepared.hreq.timeout,
            Some(std::time::Duration::from_secs(5))
        );
    }

    /// Spin a one-shot localhost HTTP/1.1 server on an ephemeral port that replies with `status` and
    /// `body` to the first request. Returns the bound `http://127.0.0.1:<port>` base URL. Hermetic:
    /// no real-internet dependency. The server thread exits after serving one request.
    fn mock_server(status: u16, body: &'static str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 2048];
                let _ = stream.read(&mut buf); // drain the request head (ignored)
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
    async fn get_writes_body_to_stdout() {
        let url = mock_server(200, "hello-body");
        let out = run(&argv(&[&url])).await;
        assert_eq!(out.exit_code, 0);
        assert_eq!(String::from_utf8(out.stdout).unwrap(), "hello-body");
    }

    #[tokio::test]
    async fn output_flag_writes_file_and_leaves_stdout_empty() {
        let url = mock_server(200, "file-body");
        let path = std::env::temp_dir().join(format!("wcurl_out_{}", std::process::id()));
        let out = run(&argv(&["-o", path.to_str().unwrap(), &url])).await;
        assert_eq!(out.exit_code, 0);
        assert!(out.stdout.is_empty());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "file-body");
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn an_http_error_status_is_a_transfer_that_worked() {
        // As in curl: without -f the response is written, nothing is said, and the status is 0.
        for status in [404, 500] {
            let url = mock_server(status, "boom");
            let out = run(&argv(&[&url])).await;
            assert_eq!(out.exit_code, 0, "{status}");
            assert_eq!(String::from_utf8(out.stdout).unwrap(), "boom");
            assert!(out.stderr.is_empty(), "{status}");
        }
    }

    #[tokio::test]
    async fn fail_with_body_writes_the_body_and_exits_22() {
        let url = mock_server(404, "page");
        let out = run(&argv(&["--fail-with-body", &url])).await;
        assert_eq!(out.exit_code, 22);
        assert_eq!(String::from_utf8(out.stdout).unwrap(), "page");
        assert_eq!(
            String::from_utf8(out.stderr).unwrap(),
            "curl: (22) The requested URL returned error: 404\n"
        );
        let url = mock_server(404, "page");
        let out = run(&argv(&["-f", &url])).await;
        assert_eq!(out.exit_code, 22);
        assert!(out.stdout.is_empty());
    }

    #[tokio::test]
    async fn missing_url_is_a_usage_error() {
        let out = run(&argv(&["-s"])).await;
        assert_eq!(out.exit_code, 2);
    }

    #[tokio::test]
    async fn a_refused_connection_is_curls_exit_7() {
        // Port 1 on localhost refuses — a connection failure, not an HTTP status.
        let out = run(&argv(&["http://127.0.0.1:1/"])).await;
        assert_eq!(out.exit_code, 7);
        assert!(out.stdout.is_empty());
        let stderr = String::from_utf8(out.stderr).unwrap();
        assert!(
            stderr.starts_with("curl: (7) Failed to connect to 127.0.0.1:1 after ")
                && stderr.ends_with(" ms: Could not connect to server\n"),
            "{stderr}"
        );
    }

    #[tokio::test]
    async fn a_host_that_does_not_resolve_is_curls_exit_6() {
        let out = run(&argv(&["-sS", "http://host.invalid/"])).await;
        assert_eq!(out.exit_code, 6);
        assert_eq!(
            String::from_utf8(out.stderr).unwrap(),
            "curl: (6) Could not resolve host: host.invalid\n"
        );
    }

    #[tokio::test]
    async fn a_port_out_of_range_is_rejected_before_connecting() {
        let out = run(&argv(&["-sS", "http://127.0.0.1:99999/"])).await;
        assert_eq!(out.exit_code, 3);
        assert_eq!(
            String::from_utf8(out.stderr).unwrap(),
            "curl: (3) URL rejected: Port number was not a decimal number between 0 and 65535\n"
        );
    }

    #[tokio::test]
    async fn each_url_is_a_transfer_and_the_last_one_sets_the_status() {
        let url = mock_server(200, "body");
        let out = run(&argv(&[
            "-s",
            "-w",
            "[%{http_code}]",
            &url,
            "http://127.0.0.1:1/",
        ]))
        .await;
        assert_eq!(out.exit_code, 7);
        assert_eq!(String::from_utf8(out.stdout).unwrap(), "body[200][000]");
    }

    /// A one-shot server that also sends an extra response header (for `-i`/`-w` assertions).
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

    /// A server that 302-redirects the first request to `/final`, then serves `body` (for `-L`).
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
    async fn include_prepends_response_headers() {
        let url = mock_server_h(200, ("X-Custom", "yes"), "the-body");
        let out = run(&argv(&["-i", &url])).await;
        let text = String::from_utf8(out.stdout).unwrap();
        assert!(
            text.starts_with("HTTP/1.1 200"),
            "status line first:\n{text}"
        );
        assert!(
            text.to_lowercase().contains("x-custom: yes"),
            "header shown:\n{text}"
        );
        assert!(text.contains("the-body"), "body follows:\n{text}");
    }

    #[tokio::test]
    async fn head_prints_headers_without_body() {
        let url = mock_server_h(200, ("X-Custom", "yes"), "should-not-appear");
        let out = run(&argv(&["-I", &url])).await;
        let text = String::from_utf8(out.stdout).unwrap();
        assert!(text.to_lowercase().contains("x-custom: yes"));
        assert!(
            !text.contains("should-not-appear"),
            "HEAD has no body:\n{text}"
        );
    }

    #[tokio::test]
    async fn fail_flag_suppresses_body_and_exits_22() {
        let url = mock_server(404, "error page");
        let out = run(&argv(&["-f", &url])).await;
        assert_eq!(out.exit_code, 22);
        assert!(out.stdout.is_empty(), "-f writes no body");
    }

    #[tokio::test]
    async fn write_out_expands_http_code() {
        let url = mock_server(200, "body");
        let out = run(&argv(&[
            "-s",
            "-o",
            "/dev/null",
            "-w",
            "%{http_code}\\n",
            &url,
        ]))
        .await;
        assert_eq!(String::from_utf8(out.stdout).unwrap(), "200\n");
    }

    #[tokio::test]
    async fn location_follows_the_redirect() {
        let url = mock_redirect("arrived");
        // Without -L: the 302 body (empty) and exit 0.
        let out = run(&argv(&["-s", &mock_redirect("x")])).await;
        assert_eq!(out.exit_code, 0);
        // With -L: the final body.
        let out = run(&argv(&["-sL", &url])).await;
        assert_eq!(String::from_utf8(out.stdout).unwrap(), "arrived");
    }

    // -- -F, -O/-J, --create-dirs, --retry, -b/-c, --compressed, extended -w ----------------

    use std::sync::{Arc, Mutex};

    /// A server that replies to successive connections in order (one `Reply` per accepted TCP
    /// connection — a fresh connection per request, which is what `Connection: close` plus a new
    /// `whttp::fetch` call each `--retry` attempt produces) and records each request's raw bytes
    /// (request line + headers + body) for the test to inspect.
    type Reply = (u16, Vec<(&'static str, &'static str)>, &'static str);
    fn recording_server(replies: Vec<Reply>) -> (String, Arc<Mutex<Vec<Vec<u8>>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let requests_thread = requests.clone();
        std::thread::spawn(move || {
            for (status, headers, body) in replies {
                let Ok((mut stream, _)) = listener.accept() else {
                    break;
                };
                let request = read_full_request(&mut stream);
                requests_thread.lock().unwrap().push(request);
                let mut head = format!("HTTP/1.1 {status} X\r\nContent-Length: {}\r\n", body.len());
                for (k, v) in &headers {
                    let _ = write!(head, "{k}: {v}\r\n");
                }
                head.push_str("Connection: close\r\n\r\n");
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(body.as_bytes());
                let _ = stream.flush();
            }
        });
        (base, requests)
    }

    /// Read one full HTTP/1.1 request (headers + a `Content-Length`-sized body) off `stream`.
    fn read_full_request(stream: &mut std::net::TcpStream) -> Vec<u8> {
        let mut buf = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            let n = stream.read(&mut chunk).unwrap_or(0);
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n]);
            let Some(split) = buf.windows(4).position(|w| w == b"\r\n\r\n") else {
                continue;
            };
            let header_end = split + 4;
            let content_length: usize = String::from_utf8_lossy(&buf[..split])
                .lines()
                .find_map(|line| {
                    line.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .and_then(|v| v.trim().parse().ok())
                })
                .unwrap_or(0);
            let mut needed = content_length.saturating_sub(buf.len() - header_end);
            while needed > 0 {
                let n = stream.read(&mut chunk).unwrap_or(0);
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&chunk[..n]);
                needed = needed.saturating_sub(n);
            }
            break;
        }
        buf
    }

    fn reply(status: u16, headers: &[(&'static str, &'static str)], body: &'static str) -> Reply {
        (status, headers.to_vec(), body)
    }

    #[tokio::test]
    async fn form_field_sends_a_multipart_body() {
        let (base, requests) = recording_server(vec![reply(200, &[], "ok")]);
        let out = run(&argv(&["-F", "a=1", "-F", "b=two", &base])).await;
        assert_eq!(out.exit_code, 0);
        let sent = requests.lock().unwrap();
        let text = String::from_utf8_lossy(&sent[0]);
        assert!(
            text.contains("content-type: multipart/form-data; boundary=")
                || text.contains("Content-Type: multipart/form-data; boundary="),
            "multipart content-type sent:\n{text}"
        );
        assert!(text.contains("name=\"a\""));
        assert!(text.contains("\r\n\r\n1"));
        assert!(text.contains("name=\"b\""));
        assert!(text.find("name=\"a\"").unwrap() < text.find("name=\"b\"").unwrap());
    }

    #[tokio::test]
    async fn form_file_field_uploads_the_file_contents() {
        let dir = std::env::temp_dir().join(format!("wcurl_form_file_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("note.txt"), b"hello file").unwrap();
        let (base, requests) = recording_server(vec![reply(200, &[], "ok")]);
        let out = run_in_directory(&argv(&["-F", "f=@note.txt", &base]), &dir).await;
        assert_eq!(out.exit_code, 0);
        let sent = requests.lock().unwrap();
        let text = String::from_utf8_lossy(&sent[0]);
        assert!(text.contains("filename=\"note.txt\""));
        assert!(text.contains("Content-Type: text/plain"));
        assert!(text.contains("hello file"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn remote_name_saves_under_the_url_basename() {
        let dir = std::env::temp_dir().join(format!("wcurl_remote_name_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let url = mock_server(200, "remote-body");
        let out = run_in_directory(&argv(&["-O", &format!("{url}/report.txt")]), &dir).await;
        assert_eq!(out.exit_code, 0);
        assert_eq!(
            std::fs::read_to_string(dir.join("report.txt")).unwrap(),
            "remote-body"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn remote_header_name_prefers_content_disposition() {
        let dir = std::env::temp_dir().join(format!("wcurl_remote_header_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let url = mock_server_h(
            200,
            ("Content-Disposition", "attachment; filename=\"real.csv\""),
            "csv-body",
        );
        let out = run_in_directory(&argv(&["-OJ", &format!("{url}/ignored-name")]), &dir).await;
        assert_eq!(out.exit_code, 0);
        assert_eq!(
            std::fs::read_to_string(dir.join("real.csv")).unwrap(),
            "csv-body"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn create_dirs_makes_the_leading_directories_for_output() {
        let dir = std::env::temp_dir().join(format!("wcurl_create_dirs_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let url = mock_server(200, "nested-body");
        let out =
            run_in_directory(&argv(&["--create-dirs", "-o", "a/b/out.txt", &url]), &dir).await;
        assert_eq!(out.exit_code, 0);
        assert_eq!(
            std::fs::read_to_string(dir.join("a/b/out.txt")).unwrap(),
            "nested-body"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn upload_file_puts_the_file_contents() {
        let dir = std::env::temp_dir().join(format!("wcurl_upload_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("payload.bin"), b"upload-me").unwrap();
        let (base, requests) = recording_server(vec![reply(200, &[], "stored")]);
        let out = run_in_directory(&argv(&["-T", "payload.bin", &base]), &dir).await;
        assert_eq!(out.exit_code, 0);
        let sent = requests.lock().unwrap();
        let text = String::from_utf8_lossy(&sent[0]);
        assert!(text.starts_with("PUT "), "defaults to PUT:\n{text}");
        assert!(text.ends_with("upload-me"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn upload_file_to_a_trailing_slash_url_appends_the_filename() {
        let dir = std::env::temp_dir().join(format!("wcurl_upload_slash_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("file.dat"), b"x").unwrap();
        let (base, requests) = recording_server(vec![reply(200, &[], "ok")]);
        let _ = run_in_directory(&argv(&["-T", "file.dat", &format!("{base}/dir/")]), &dir).await;
        let sent = requests.lock().unwrap();
        let text = String::from_utf8_lossy(&sent[0]);
        assert!(
            text.starts_with("PUT /dir/file.dat "),
            "filename appended to the trailing-slash URL:\n{text}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn retry_reattempts_on_a_retryable_status_then_succeeds() {
        let (base, requests) = recording_server(vec![
            reply(503, &[], "busy"),
            reply(503, &[], "busy"),
            reply(200, &[], "finally"),
        ]);
        let out = run(&argv(&["--retry", "2", &base])).await;
        assert_eq!(out.exit_code, 0);
        assert_eq!(String::from_utf8(out.stdout).unwrap(), "finally");
        assert_eq!(requests.lock().unwrap().len(), 3);
    }

    #[tokio::test]
    async fn retry_gives_up_after_the_budget_and_reports_the_last_status() {
        let (base, requests) = recording_server(vec![reply(500, &[], "a"), reply(500, &[], "b")]);
        let out = run(&argv(&["--retry", "1", &base])).await;
        assert_eq!(
            out.exit_code, 0,
            "without -f, the last response is the transfer's, as in curl"
        );
        assert_eq!(String::from_utf8(out.stdout).unwrap(), "b");
        assert_eq!(requests.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn non_retryable_status_is_not_retried() {
        let (base, requests) = recording_server(vec![reply(404, &[], "nope")]);
        let out = run(&argv(&["--retry", "3", &base])).await;
        assert_eq!(out.exit_code, 0);
        assert_eq!(requests.lock().unwrap().len(), 1, "404 is not retryable");
    }

    #[tokio::test]
    async fn cookie_literal_is_sent_and_jar_is_written_after_set_cookie() {
        let dir = std::env::temp_dir().join(format!("wcurl_cookie_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (base, requests) = recording_server(vec![reply(
            200,
            &[("Set-Cookie", "sid=abc123; Path=/")],
            "ok",
        )]);
        let out =
            run_in_directory(&argv(&["-b", "existing=1", "-c", "jar.txt", &base]), &dir).await;
        assert_eq!(out.exit_code, 0);
        let sent = String::from_utf8_lossy(&requests.lock().unwrap()[0]).to_ascii_lowercase();
        assert!(
            sent.contains("cookie: existing=1"),
            "the literal -b cookie was sent:\n{sent}"
        );
        let jar = std::fs::read_to_string(dir.join("jar.txt")).unwrap();
        assert!(
            jar.contains("sid\tabc123"),
            "Set-Cookie was persisted:\n{jar}"
        );
        assert!(
            !jar.contains("existing"),
            "a literal -b cookie is only sent, as in curl:\n{jar}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn cookie_jar_file_round_trips_through_b_and_c() {
        let dir = std::env::temp_dir().join(format!("wcurl_cookie_jar_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("in.txt"),
            "# Netscape HTTP Cookie File\n127.0.0.1\tFALSE\t/\tFALSE\t0\tsaved\tvalue1\n",
        )
        .unwrap();
        let (base, requests) = recording_server(vec![reply(200, &[], "ok")]);
        // The server binds 127.0.0.1, matching the jar's cookie domain exactly.
        let host_matched_url = base.clone();
        let _ = run_in_directory(&argv(&["-b", "in.txt", &host_matched_url]), &dir).await;
        let sent = String::from_utf8_lossy(&requests.lock().unwrap()[0]).to_ascii_lowercase();
        assert!(
            sent.contains("cookie: saved=value1"),
            "a cookie loaded from a jar FILE was sent:\n{sent}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn compressed_flag_decodes_a_gzip_response_end_to_end() {
        use std::io::Write as _;
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(b"decoded via wcurl").unwrap();
        let gz = encoder.finish().unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 2048];
                let _ = stream.read(&mut buf);
                let mut head = format!(
                    "HTTP/1.1 200 X\r\nContent-Encoding: gzip\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    gz.len()
                );
                let _ = stream.write_all(std::mem::take(&mut head).as_bytes());
                let _ = stream.write_all(&gz);
                let _ = stream.flush();
            }
        });
        let out = run(&argv(&["--compressed", &format!("http://{addr}")])).await;
        assert_eq!(String::from_utf8(out.stdout).unwrap(), "decoded via wcurl");
    }

    #[tokio::test]
    async fn write_out_reports_the_extended_variable_set() {
        let url = mock_server_h(200, ("X-Trace", "xyz"), "0123456789");
        let out = run(&argv(&[
            "-s",
            "-o",
            "/dev/null",
            "-d",
            "abc",
            "-w",
            "%{http_code} %{size_download} %{size_upload} %{num_redirects} %{method} %{header{X-Trace}}\\n",
            &url,
        ]))
        .await;
        assert_eq!(
            String::from_utf8(out.stdout).unwrap(),
            "200 10 3 0 POST xyz\n"
        );
    }

    #[tokio::test]
    async fn write_out_reports_exitcode_and_errormsg_on_transport_failure() {
        let out = run(&argv(&[
            "-s",
            "-w",
            "exit=%{exitcode} has-error=%{errormsg}\\n",
            "http://127.0.0.1:1/",
        ]))
        .await;
        assert_eq!(out.exit_code, 7);
        let text = String::from_utf8(out.stdout).unwrap();
        assert!(text.starts_with("exit=7 has-error="), "{text}");
        assert!(
            !text.trim_end().ends_with("has-error="),
            "errormsg is non-empty: {text}"
        );
    }

    // -- run_streaming (the shell's sink-based entry point) -------------------------

    async fn run_streaming_to_vec(args_: &[&str], cwd: &std::path::Path) -> (Outcome, Vec<u8>) {
        let mut sink = futures::io::AllowStdIo::new(Vec::new());
        let outcome = run_streaming(&argv(args_), cwd, &mut sink).await.unwrap();
        (outcome, sink.into_inner())
    }

    /// A server that sends `hello`, then a `.` every 20 ms for five seconds: a body that never
    /// goes idle long enough for a per-read timeout, only a whole-transfer one, to end it.
    fn trickle_server() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 2048];
                let _ = stream.read(&mut buf);
                let _ = stream.write_all(b"HTTP/1.1 200 X\r\nConnection: close\r\n\r\nhello");
                for _ in 0..250 {
                    std::thread::sleep(std::time::Duration::from_millis(20));
                    if stream
                        .write_all(b".")
                        .and_then(|()| stream.flush())
                        .is_err()
                    {
                        break;
                    }
                }
            }
        });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn max_time_ends_a_body_still_arriving_with_exit_28() {
        let (out, written) =
            run_streaming_to_vec(&["-m", "0.5", &trickle_server()], std::path::Path::new("."))
                .await;
        assert_eq!(out.exit_code, 28);
        assert!(written.starts_with(b"hello"), "{written:?}");
        let stderr = String::from_utf8(out.stderr).unwrap();
        assert!(
            stderr.starts_with("curl: (28) Operation timed out after ")
                && stderr.ends_with(&format!(" with {} bytes received\n", written.len())),
            "{stderr}"
        );
        // -s hides the message, not the status.
        let (out, _) = run_streaming_to_vec(
            &["-s", "-m", "0.5", &trickle_server()],
            std::path::Path::new("."),
        )
        .await;
        assert_eq!((out.exit_code, out.stderr.as_slice()), (28, &b""[..]));
    }

    #[tokio::test]
    async fn streaming_get_writes_the_body_through_the_sink_not_outcome() {
        let url = mock_server(200, "streamed-body");
        let (out, written) = run_streaming_to_vec(&[&url], std::path::Path::new(".")).await;
        assert_eq!(out.exit_code, 0);
        assert!(out.stdout.is_empty(), "the body went through the sink");
        assert_eq!(String::from_utf8(written).unwrap(), "streamed-body");
    }

    #[tokio::test]
    async fn streaming_include_prepends_headers_before_the_body() {
        let url = mock_server_h(200, ("X-Custom", "yes"), "the-body");
        let (out, written) = run_streaming_to_vec(&["-i", &url], std::path::Path::new(".")).await;
        assert_eq!(out.exit_code, 0);
        let text = String::from_utf8(written).unwrap();
        assert!(text.starts_with("HTTP/1.1 200"));
        assert!(text.to_lowercase().contains("x-custom: yes"));
        assert!(text.ends_with("the-body"));
    }

    #[tokio::test]
    async fn streaming_head_writes_only_headers() {
        let url = mock_server_h(200, ("X-Custom", "yes"), "should-not-appear");
        let (out, written) = run_streaming_to_vec(&["-I", &url], std::path::Path::new(".")).await;
        assert_eq!(out.exit_code, 0);
        let text = String::from_utf8(written).unwrap();
        assert!(text.to_lowercase().contains("x-custom: yes"));
        assert!(!text.contains("should-not-appear"));
    }

    #[tokio::test]
    async fn streaming_fail_flag_writes_nothing_and_exits_22() {
        let url = mock_server(404, "error page");
        let (out, written) = run_streaming_to_vec(&["-f", &url], std::path::Path::new(".")).await;
        assert_eq!(out.exit_code, 22);
        assert!(written.is_empty(), "-f writes no body");
    }

    #[tokio::test]
    async fn streaming_write_out_follows_the_body_through_outcome_not_the_sink() {
        let url = mock_server(200, "0123456789");
        let (out, written) = run_streaming_to_vec(
            &["-s", "-w", "%{http_code} %{size_download}\\n", &url],
            std::path::Path::new("."),
        )
        .await;
        assert_eq!(written, b"0123456789", "body went through the sink");
        assert_eq!(
            String::from_utf8(out.stdout).unwrap(),
            "200 10\n",
            "-w's own output comes back via Outcome, appended by the caller after the body"
        );
    }

    #[tokio::test]
    async fn streaming_several_urls_keep_each_write_out_after_its_own_body() {
        let first = mock_server(200, "one");
        let second = mock_server(200, "two");
        let (out, written) = run_streaming_to_vec(
            &["-s", "-w", "[%{http_code}]", &first, &second],
            std::path::Path::new("."),
        )
        .await;
        assert_eq!(String::from_utf8(written).unwrap(), "one[200]two");
        assert_eq!(String::from_utf8(out.stdout).unwrap(), "[200]");
        assert_eq!(out.exit_code, 0);
    }

    #[tokio::test]
    async fn streaming_retry_happens_before_any_body_byte_is_forwarded() {
        let (base, requests) = recording_server(vec![
            reply(503, &[], "busy"),
            reply(200, &[], "finally-streamed"),
        ]);
        let (out, written) =
            run_streaming_to_vec(&["--retry", "1", &base], std::path::Path::new(".")).await;
        assert_eq!(out.exit_code, 0);
        assert_eq!(String::from_utf8(written).unwrap(), "finally-streamed");
        assert_eq!(requests.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn streaming_output_file_writes_incrementally_and_the_sink_stays_empty() {
        let dir = std::env::temp_dir().join(format!("wcurl_stream_file_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let url = mock_server(200, "file-streamed-body");
        let mut sink = futures::io::AllowStdIo::new(Vec::new());
        let out = run_streaming(&argv(&["-o", "out.bin", &url]), &dir, &mut sink)
            .await
            .unwrap();
        assert_eq!(out.exit_code, 0);
        assert!(
            sink.into_inner().is_empty(),
            "nothing goes to the sink under -o"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("out.bin")).unwrap(),
            "file-streamed-body"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn streaming_remote_name_and_header_name_resolve_before_the_body_is_read() {
        let dir = std::env::temp_dir().join(format!("wcurl_stream_named_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let url = mock_server_h(
            200,
            ("Content-Disposition", "attachment; filename=\"real.csv\""),
            "csv-body",
        );
        let mut sink = futures::io::AllowStdIo::new(Vec::new());
        let out = run_streaming(&argv(&["-OJ", &format!("{url}/ignored")]), &dir, &mut sink)
            .await
            .unwrap();
        assert_eq!(out.exit_code, 0);
        assert_eq!(
            std::fs::read_to_string(dir.join("real.csv")).unwrap(),
            "csv-body"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn streaming_compressed_falls_back_to_the_buffered_path() {
        use std::io::Write as _;
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder
            .write_all(b"decoded via streaming fallback")
            .unwrap();
        let gz = encoder.finish().unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 2048];
                let _ = stream.read(&mut buf);
                let mut head = format!(
                    "HTTP/1.1 200 X\r\nContent-Encoding: gzip\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    gz.len()
                );
                let _ = stream.write_all(std::mem::take(&mut head).as_bytes());
                let _ = stream.write_all(&gz);
                let _ = stream.flush();
            }
        });
        let (out, written) = run_streaming_to_vec(
            &["--compressed", &format!("http://{addr}")],
            std::path::Path::new("."),
        )
        .await;
        assert_eq!(out.exit_code, 0);
        assert_eq!(
            String::from_utf8(written).unwrap(),
            "decoded via streaming fallback"
        );
    }

    /// A sink whose every write fails with `BrokenPipe` — proves `run_streaming` propagates a
    /// sink write error via `Err` rather than swallowing it into a normal `Outcome`.
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
        let result = run_streaming(&argv(&[&url]), std::path::Path::new("."), &mut sink).await;
        let error = result.expect_err("a closed sink must surface as Err, not a status code");
        assert_eq!(error.kind(), std::io::ErrorKind::BrokenPipe);
    }
}
