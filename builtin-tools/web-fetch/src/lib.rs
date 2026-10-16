use encoding_rs::{DecoderResult, Encoding, UTF_8, UTF_16BE, UTF_16LE};
use golem_rust::durability::{Durability, DurableFunctionType};
use golem_rust::wasip3::clocks::monotonic_clock;
use golem_rust::wasip3::http::{client, types};
use golem_rust::wasip3::wit_bindgen::StreamResult;
use golem_rust::wasip3::wit_future;
use golem_rust::{
    FromSchema, FromWire, IntoSchema, IntoWire, ToolError, WireSchema, tool_definition,
    tool_implementation,
};
use std::collections::HashSet;
use std::future::Future;
use std::pin::pin;
use std::task::Poll;
use url::{Position, Url};

pub const DEFAULT_TIMEOUT_MS: u64 = 10_000;
pub const MAX_TIMEOUT_MS: u64 = 30_000;
pub const DEFAULT_MAX_RESPONSE_BYTES: u64 = 2 * 1024 * 1024;
pub const MAX_RESPONSE_BYTES: u64 = 5 * 1024 * 1024;
pub const DEFAULT_MAX_REDIRECTS: u32 = 5;
pub const MAX_REDIRECTS: u32 = 10;
const MAX_HTML_DOM_DEPTH: usize = 512;
const MAX_HTML_NODE_MARKERS: usize = 100_000;
const MAX_HTML_RENDER_WORK: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FetchConfig {
    timeout_ms: u64,
    max_response_bytes: u64,
    max_redirects: u32,
    convert_html_to_text: bool,
}

impl FetchConfig {
    fn from_options(
        timeout_ms: Option<u64>,
        max_response_bytes: Option<u64>,
        max_redirects: Option<u32>,
        convert_html_to_text: Option<bool>,
    ) -> Result<Self, WebFetchError> {
        Ok(Self {
            timeout_ms: safety_limit(
                "timeout_ms",
                timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS),
                1,
                MAX_TIMEOUT_MS,
            )?,
            max_response_bytes: safety_limit(
                "max_response_bytes",
                max_response_bytes.unwrap_or(DEFAULT_MAX_RESPONSE_BYTES),
                1,
                MAX_RESPONSE_BYTES,
            )?,
            max_redirects: safety_limit(
                "max_redirects",
                u64::from(max_redirects.unwrap_or(DEFAULT_MAX_REDIRECTS)),
                0,
                u64::from(MAX_REDIRECTS),
            )? as u32,
            convert_html_to_text: convert_html_to_text.unwrap_or(false),
        })
    }
}

fn safety_limit(key: &str, value: u64, minimum: u64, maximum: u64) -> Result<u64, WebFetchError> {
    if !(minimum..=maximum).contains(&value) {
        return Err(WebFetchError::InvalidSafetyLimit(format!(
            "'{key}' must be between {minimum} and {maximum}, got {value}"
        )));
    }
    Ok(value)
}

fn parse_url(raw: &str) -> Result<Url, WebFetchError> {
    let mut url = Url::parse(raw)
        .map_err(|error| WebFetchError::InvalidUrl(format!("invalid URL: {error}")))?;
    validate_url(&url)?;
    url.set_fragment(None);
    Ok(url)
}

fn validate_url(url: &Url) -> Result<(), WebFetchError> {
    if !matches!(url.scheme(), "http" | "https") {
        return Err(WebFetchError::UnsupportedScheme(format!(
            "URL scheme '{}' is not supported",
            url.scheme()
        )));
    }
    if url.host().is_none() {
        return Err(WebFetchError::InvalidUrl(format!(
            "URL '{}' has no host",
            url.as_str()
        )));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(WebFetchError::InvalidUrl(
            "URLs with embedded credentials are not allowed".to_string(),
        ));
    }
    Ok(())
}

async fn follow_redirects(
    initial_url: Url,
    config: FetchConfig,
    deadline: u64,
) -> Result<(Url, types::Response), WebFetchError> {
    let mut current = initial_url;
    let mut visited = HashSet::new();
    visited.insert(current.clone());
    let mut followed = 0;

    loop {
        let response = send_get(&current, deadline, config.timeout_ms).await?;
        ensure_before_deadline(deadline, config.timeout_ms)?;
        let status = response.get_status_code();
        if !is_redirect(status) {
            return Ok((current, response));
        }

        let Some(location) = response_header(&response, "location")? else {
            return Ok((current, response));
        };
        if followed >= config.max_redirects {
            return Err(WebFetchError::RedirectLimit(format!(
                "redirect limit of {} exceeded at '{}'",
                config.max_redirects, current
            )));
        }
        let next = resolve_redirect(&current, &location)?;
        if current.scheme() == "https" && next.scheme() == "http" {
            return Err(WebFetchError::UnsafeRedirect(format!(
                "redirect from HTTPS to HTTP is not allowed: '{current}' to '{next}'"
            )));
        }
        if !visited.insert(next.clone()) {
            return Err(WebFetchError::UnsafeRedirect(format!(
                "redirect cycle detected at '{next}'"
            )));
        }
        drop(response);
        current = next;
        followed += 1;
    }
}

fn resolve_redirect(current: &Url, location: &str) -> Result<Url, WebFetchError> {
    let mut next = current.join(location).map_err(|error| {
        WebFetchError::UnsafeRedirect(format!("invalid redirect target from '{current}': {error}"))
    })?;
    validate_url(&next).map_err(|error| {
        WebFetchError::UnsafeRedirect(format!(
            "redirect target from '{current}' is not allowed: {error:?}"
        ))
    })?;
    next.set_fragment(None);
    Ok(next)
}

async fn send_get(
    url: &Url,
    deadline: u64,
    configured_timeout_ms: u64,
) -> Result<types::Response, WebFetchError> {
    let headers = types::Fields::from_list(&[
        (
            "accept".to_string(),
            b"text/html, text/plain, application/json, application/xml, text/xml;q=0.9, */*;q=0.1"
                .to_vec(),
        ),
        ("accept-charset".to_string(), b"utf-8, *;q=0.1".to_vec()),
    ])
    .map_err(|error| WebFetchError::Transport(format!("failed to build headers: {error:?}")))?;
    let options = types::RequestOptions::new();
    let timeout_ms = remaining_timeout_ms(deadline, configured_timeout_ms)?;
    let timeout_ns = timeout_ms.saturating_mul(1_000_000);
    options
        .set_connect_timeout(Some(timeout_ns))
        .map_err(|error| WebFetchError::Transport(format!("invalid connect timeout: {error:?}")))?;
    options
        .set_first_byte_timeout(Some(timeout_ns))
        .map_err(|error| {
            WebFetchError::Transport(format!("invalid first-byte timeout: {error:?}"))
        })?;
    options
        .set_between_bytes_timeout(Some(timeout_ns))
        .map_err(|error| {
            WebFetchError::Transport(format!("invalid between-bytes timeout: {error:?}"))
        })?;
    let (trailers_tx, trailers_rx) = wit_future::new(|| Ok(None));
    let (request, _transmit) = types::Request::new(headers, None, trailers_rx, Some(options));
    drop(trailers_tx);
    request
        .set_method(&types::Method::Get)
        .map_err(|_| WebFetchError::InvalidUrl(format!("invalid GET URL '{url}'")))?;
    request
        .set_scheme(Some(&match url.scheme() {
            "http" => types::Scheme::Http,
            "https" => types::Scheme::Https,
            _ => unreachable!("URL was validated before request construction"),
        }))
        .map_err(|_| WebFetchError::InvalidUrl(format!("invalid URL scheme in '{url}'")))?;
    request
        .set_authority(Some(&url[Position::BeforeHost..Position::AfterPort]))
        .map_err(|_| WebFetchError::InvalidUrl(format!("invalid URL authority in '{url}'")))?;
    request
        .set_path_with_query(Some(&url[Position::BeforePath..Position::AfterQuery]))
        .map_err(|_| WebFetchError::InvalidUrl(format!("invalid URL path in '{url}'")))?;
    match client::send(request).await {
        Ok(response) => Ok(response),
        Err(_) if monotonic_clock::now() >= deadline => Err(timeout_error(configured_timeout_ms)),
        Err(error) => Err(WebFetchError::Transport(format!(
            "GET '{url}' failed: {error:?}"
        ))),
    }
}

fn response_header(
    response: &types::Response,
    name: &str,
) -> Result<Option<String>, WebFetchError> {
    let headers = response.get_headers();
    let values = headers.get(name);
    drop(headers);
    let Some(value) = values.first() else {
        return Ok(None);
    };
    String::from_utf8(value.clone()).map(Some).map_err(|_| {
        WebFetchError::Transport(format!("response header '{name}' is not valid UTF-8"))
    })
}

fn is_redirect(status: u16) -> bool {
    matches!(status, 301 | 302 | 303 | 307 | 308)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ContentKind {
    Html,
    Text,
}

#[derive(Clone, Copy, Debug)]
struct ParsedContentType {
    kind: ContentKind,
    encoding: ParsedEncoding,
    header_missing: bool,
}

#[derive(Clone, Copy, Debug)]
enum ParsedEncoding {
    Declared(&'static Encoding),
    Utf16WithBom,
}

fn parse_content_type(content_type: Option<&str>) -> Result<ParsedContentType, WebFetchError> {
    let Some(content_type) = content_type else {
        return Ok(ParsedContentType {
            kind: ContentKind::Text,
            encoding: ParsedEncoding::Declared(UTF_8),
            header_missing: true,
        });
    };
    let media_type = content_type.parse::<mime::Mime>().map_err(|error| {
        WebFetchError::UnsupportedContentType(format!(
            "response Content-Type '{content_type}' is invalid: {error}"
        ))
    })?;
    let kind = if (media_type.type_() == mime::TEXT && media_type.subtype() == mime::HTML)
        || matches!(
            (media_type.type_().as_str(), media_type.subtype().as_str()),
            ("application", "xhtml")
        ) && media_type
            .suffix()
            .is_some_and(|suffix| suffix == mime::XML)
    {
        ContentKind::Html
    } else if media_type.type_() == mime::TEXT
        || matches!(
            (media_type.type_().as_str(), media_type.subtype().as_str()),
            ("application", "json")
                | ("application", "xml")
                | ("application", "javascript")
                | ("application", "x-javascript")
                | ("application", "graphql")
                | ("application", "yaml")
                | ("application", "x-yaml")
                | ("application", "toml")
        )
        || (media_type.type_() == mime::APPLICATION
            && matches!(
                media_type.suffix().map(|suffix| suffix.as_str()),
                Some("json" | "xml")
            ))
    {
        ContentKind::Text
    } else {
        return Err(WebFetchError::UnsupportedContentType(format!(
            "response Content-Type '{content_type}' is not supported"
        )));
    };
    let encoding = match media_type.get_param(mime::CHARSET) {
        None => ParsedEncoding::Declared(UTF_8),
        Some(label) if label.as_str().eq_ignore_ascii_case("utf-16") => {
            ParsedEncoding::Utf16WithBom
        }
        Some(label) => ParsedEncoding::Declared(
            Encoding::for_label(label.as_str().as_bytes()).ok_or_else(|| {
                WebFetchError::InvalidTextEncoding(format!(
                    "unsupported response charset '{label}'"
                ))
            })?,
        ),
    };
    Ok(ParsedContentType {
        kind,
        encoding,
        header_missing: false,
    })
}

async fn read_bounded_body(
    response: types::Response,
    max_bytes: usize,
    deadline: u64,
    timeout_ms: u64,
) -> Result<(Vec<u8>, bool), WebFetchError> {
    let (response_done_tx, response_done_rx) = wit_future::new(|| Ok(()));
    let (mut body, trailers) = types::Response::consume_body(response, response_done_rx);
    let mut bytes = Vec::with_capacity(max_bytes.min(64 * 1024));

    while bytes.len() < max_bytes {
        ensure_before_deadline(deadline, timeout_ms)?;
        let allowance = (max_bytes - bytes.len()).min(64 * 1024);
        let timed_out = {
            let mut read = pin!(body.read(Vec::with_capacity(allowance)));
            let mut timer = pin!(monotonic_clock::wait_until(deadline));
            futures::future::poll_fn(|context| {
                if let Poll::Ready(result) = read.as_mut().poll(context) {
                    return Poll::Ready(Ok(result));
                }
                if timer.as_mut().poll(context).is_ready() {
                    return Poll::Ready(Err(read.as_mut().cancel()));
                }
                Poll::Pending
            })
            .await
        };
        let (result, buffer) = match timed_out {
            Ok(result) => result,
            Err((StreamResult::Dropped, _)) => {
                drop(body);
                trailers.await.map_err(|error| {
                    WebFetchError::Transport(format!(
                        "failed while receiving response trailers: {error:?}"
                    ))
                })?;
                response_done_tx.write(Ok(())).await.map_err(|error| {
                    WebFetchError::Transport(format!(
                        "failed to acknowledge response body: {error:?}"
                    ))
                })?;
                return Err(timeout_error(timeout_ms));
            }
            Err((_result, _buffer)) => {
                drop(body);
                drop(trailers);
                drop(response_done_tx);
                return Err(timeout_error(timeout_ms));
            }
        };
        ensure_before_deadline(deadline, timeout_ms)?;
        match result {
            StreamResult::Complete(count) => {
                bytes.extend_from_slice(&buffer[..count]);
            }
            StreamResult::Dropped => {
                drop(body);
                trailers.await.map_err(|error| {
                    WebFetchError::Transport(format!(
                        "failed while receiving response trailers: {error:?}"
                    ))
                })?;
                response_done_tx.write(Ok(())).await.map_err(|error| {
                    WebFetchError::Transport(format!(
                        "failed to acknowledge response body: {error:?}"
                    ))
                })?;
                ensure_before_deadline(deadline, timeout_ms)?;
                return Ok((bytes, false));
            }
            StreamResult::Cancelled => {
                return Err(WebFetchError::Transport(
                    "response body read was cancelled".to_string(),
                ));
            }
        }
    }

    drop(body);
    drop(trailers);
    drop(response_done_tx);
    Ok((bytes, true))
}

fn decode_content(
    bytes: &[u8],
    content_type: ParsedContentType,
    truncated: bool,
) -> Result<String, WebFetchError> {
    let (encoding, bytes, remove_bom) = match content_type.encoding {
        ParsedEncoding::Declared(encoding) => (encoding, bytes, true),
        ParsedEncoding::Utf16WithBom if bytes.starts_with(&[0xfe, 0xff]) => {
            (UTF_16BE, &bytes[2..], false)
        }
        ParsedEncoding::Utf16WithBom if bytes.starts_with(&[0xff, 0xfe]) => {
            (UTF_16LE, &bytes[2..], false)
        }
        ParsedEncoding::Utf16WithBom => {
            return Err(WebFetchError::InvalidTextEncoding(
                "response declared charset utf-16 but has no byte-order mark".to_string(),
            ));
        }
    };
    let mut decoder = if remove_bom {
        encoding.new_decoder_with_bom_removal()
    } else {
        encoding.new_decoder_without_bom_handling()
    };
    let capacity = decoder
        .max_utf8_buffer_length_without_replacement(bytes.len())
        .unwrap_or(bytes.len().saturating_mul(3));
    let mut decoded = String::with_capacity(capacity);
    let (result, read) =
        decoder.decode_to_string_without_replacement(bytes, &mut decoded, !truncated);
    match result {
        DecoderResult::InputEmpty if read == bytes.len() => {}
        DecoderResult::Malformed(_, _) => {
            return Err(WebFetchError::InvalidTextEncoding(format!(
                "response contains invalid {} data",
                encoding.name()
            )));
        }
        DecoderResult::OutputFull | DecoderResult::InputEmpty => {
            return Err(WebFetchError::InvalidTextEncoding(
                "response text decoder did not consume the bounded body".to_string(),
            ));
        }
    }
    if content_type.header_missing && decoded.contains('\0') {
        return Err(WebFetchError::UnsupportedContentType(
            "response without Content-Type appears to be binary".to_string(),
        ));
    }
    Ok(decoded)
}

fn truncate_utf8(mut content: String, max_bytes: usize) -> (String, bool) {
    if content.len() <= max_bytes {
        return (content, false);
    }
    let mut boundary = max_bytes;
    while !content.is_char_boundary(boundary) {
        boundary -= 1;
    }
    content.truncate(boundary);
    (content, true)
}

fn convert_content(
    bytes: &[u8],
    content_type: ParsedContentType,
    input_truncated: bool,
    max_output_bytes: usize,
    convert_html_to_text: bool,
) -> Result<(String, bool), WebFetchError> {
    let decoded = decode_content(bytes, content_type, input_truncated)?;
    let content = match (content_type.kind, convert_html_to_text) {
        (ContentKind::Html, true) => convert_html(&decoded)?,
        (ContentKind::Html | ContentKind::Text, false) | (ContentKind::Text, true) => decoded,
    };
    let (content, output_truncated) = truncate_utf8(content, max_output_bytes);
    Ok((content, input_truncated || output_truncated))
}

fn convert_html(html: &str) -> Result<String, WebFetchError> {
    validate_html_source(html)?;
    let config = html2text::config::plain().unicode_strikeout(false);
    let dom = config.parse_html(html.as_bytes()).map_err(|error| {
        WebFetchError::ContentConversion(format!("failed to parse HTML response: {error}"))
    })?;
    sanitize_html_dom(&dom.document)?;
    validate_html_dom_render_work(&dom.document)?;
    let tree = config.dom_to_render_tree(&dom).map_err(|error| {
        WebFetchError::ContentConversion(format!("failed to prepare HTML response: {error}"))
    })?;
    config.render_to_string(tree, 100).map_err(|error| {
        WebFetchError::ContentConversion(format!("failed to render HTML response: {error}"))
    })
}

fn validate_html_source(html: &str) -> Result<(), WebFetchError> {
    let node_markers = html.bytes().filter(|byte| *byte == b'<').count();
    if node_markers > MAX_HTML_NODE_MARKERS {
        return Err(WebFetchError::ContentConversion(format!(
            "HTML document exceeds the maximum node marker count of {MAX_HTML_NODE_MARKERS}"
        )));
    }
    Ok(())
}

fn validate_html_dom_render_work(document: &html2text::Handle) -> Result<(), WebFetchError> {
    let mut serialized = Vec::new();
    for child in document.children.borrow().iter() {
        child.serialize(&mut serialized).map_err(|error| {
            WebFetchError::ContentConversion(format!(
                "failed to inspect normalized HTML response: {error}"
            ))
        })?;
    }

    let mut pending = vec![(document.clone(), 0_usize, 0_usize)];
    let mut node_count = 0_usize;
    let mut max_prefix_depth = 0_usize;
    let mut max_prefix_width = 0_usize;
    while let Some((node, parent_prefix_depth, parent_prefix_width)) = pending.pop() {
        node_count = node_count.saturating_add(1);
        let prefix_width = html_prefix_width(&node);
        let prefix_depth = parent_prefix_depth + usize::from(prefix_width > 0);
        let cumulative_prefix_width = parent_prefix_width.saturating_add(prefix_width);
        max_prefix_depth = max_prefix_depth.max(prefix_depth);
        max_prefix_width = max_prefix_width.max(cumulative_prefix_width);
        pending.extend(
            node.children
                .borrow()
                .iter()
                .cloned()
                .map(|child| (child, prefix_depth, cumulative_prefix_width)),
        );
    }

    let available_width = 100_usize.saturating_sub(max_prefix_width).max(1);
    let expanded_text_width = serialized.len().saturating_add(
        serialized
            .iter()
            .filter(|byte| **byte == b'\t')
            .count()
            .saturating_mul(7),
    );
    let estimated_lines = serialized
        .iter()
        .filter(|byte| **byte == b'\n')
        .count()
        .saturating_add(expanded_text_width.div_ceil(available_width))
        .saturating_add(node_count);
    let estimated_work = estimated_lines.saturating_mul(max_prefix_depth.saturating_add(1));
    if estimated_work > MAX_HTML_RENDER_WORK {
        return Err(WebFetchError::ContentConversion(format!(
            "HTML document exceeds the maximum render work budget of {MAX_HTML_RENDER_WORK}"
        )));
    }
    Ok(())
}

fn html_prefix_width(node: &html2text::Handle) -> usize {
    match node.element_name().as_deref() {
        Some("blockquote" | "ul" | "dd") => 2,
        Some("ol") => {
            let item_count = node
                .children
                .borrow()
                .iter()
                .filter(|child| child.element_name().as_deref() == Some("li"))
                .count()
                .max(1);
            item_count.ilog10() as usize + 3
        }
        Some("h1") => 2,
        Some("h2") => 3,
        Some("h3") => 4,
        Some("h4") => 5,
        Some("h5") => 6,
        Some("h6") => 7,
        _ => 0,
    }
}

fn sanitize_html_dom(node: &html2text::Handle) -> Result<usize, WebFetchError> {
    let mut pending = vec![(node.clone(), 0)];
    let mut nodes = Vec::new();
    let mut replacement_count = 0;
    while let Some((node, depth)) = pending.pop() {
        if depth > MAX_HTML_DOM_DEPTH {
            return Err(WebFetchError::ContentConversion(format!(
                "HTML document exceeds the maximum nesting depth of {MAX_HTML_DOM_DEPTH}"
            )));
        }
        pending.extend(
            node.children
                .borrow()
                .iter()
                .cloned()
                .map(|child| (child, depth + 1)),
        );
        replacement_count += node
            .children
            .borrow()
            .iter()
            .filter(|child| is_table_layout_element(child))
            .count();
        if node.element_name().as_deref() == Some("ol")
            && let html2text::Element { attrs, .. } = &node.data
        {
            attrs.borrow_mut().clear();
        }
        nodes.push(node);
    }

    if replacement_count == 0 {
        return Ok(0);
    }

    let placeholder_source = "<div></div>".repeat(replacement_count);
    let placeholder_dom = html2text::config::plain()
        .parse_html(placeholder_source.as_bytes())
        .map_err(|error| {
            WebFetchError::ContentConversion(format!("failed to prepare safe HTML layout: {error}"))
        })?;
    let mut pending = vec![placeholder_dom.document.clone()];
    let mut replacements = Vec::with_capacity(replacement_count);
    let mut placeholder_nodes = Vec::new();
    while let Some(node) = pending.pop() {
        pending.extend(node.children.borrow().iter().cloned());
        if node.element_name().as_deref() == Some("div") {
            replacements.push(node.clone());
        }
        placeholder_nodes.push(node);
    }
    for node in placeholder_nodes {
        node.children
            .borrow_mut()
            .retain(|child| child.element_name().as_deref() != Some("div"));
    }
    for replacement in &replacements {
        replacement.parent.set(None);
    }

    for node in nodes.into_iter().rev() {
        let mut children = node.children.borrow_mut();
        let mut normalized = Vec::with_capacity(children.len());
        for child in children.drain(..) {
            if is_table_layout_element(&child) {
                let replacement = replacements.pop().ok_or_else(|| {
                    WebFetchError::ContentConversion(
                        "failed to normalize HTML table layout".to_string(),
                    )
                })?;
                for grandchild in child.children.borrow_mut().drain(..) {
                    grandchild
                        .parent
                        .set(Some(std::rc::Rc::downgrade(&replacement)));
                    replacement.children.borrow_mut().push(grandchild);
                }
                replacement.parent.set(Some(std::rc::Rc::downgrade(&node)));
                child.parent.set(None);
                normalized.push(replacement);
            } else {
                normalized.push(child);
            }
        }
        *children = normalized;
    }

    Ok(replacement_count)
}

fn is_table_layout_element(node: &html2text::Handle) -> bool {
    node.element_name().is_some_and(|name| {
        matches!(
            name.as_str(),
            "table" | "thead" | "tbody" | "tfoot" | "tr" | "td" | "th"
        )
    })
}

fn timeout_error(timeout_ms: u64) -> WebFetchError {
    WebFetchError::Timeout(format!(
        "web fetch exceeded the configured {timeout_ms} ms deadline"
    ))
}

fn ensure_before_deadline(deadline: u64, timeout_ms: u64) -> Result<(), WebFetchError> {
    if monotonic_clock::now() >= deadline {
        Err(timeout_error(timeout_ms))
    } else {
        Ok(())
    }
}

fn remaining_timeout_ms(deadline: u64, timeout_ms: u64) -> Result<u64, WebFetchError> {
    let remaining_ns = deadline
        .checked_sub(monotonic_clock::now())
        .filter(|remaining| *remaining > 0)
        .ok_or_else(|| timeout_error(timeout_ms))?;
    Ok(remaining_ns.saturating_add(999_999) / 1_000_000)
}

async fn with_total_timeout<T>(
    deadline: u64,
    timeout_ms: u64,
    operation: impl Future<Output = Result<T, WebFetchError>>,
) -> Result<T, WebFetchError> {
    let mut operation = pin!(operation);
    let mut timer = pin!(monotonic_clock::wait_until(deadline));
    futures::future::poll_fn(|context| {
        if let Poll::Ready(result) = operation.as_mut().poll(context) {
            return Poll::Ready(result);
        }
        if timer.as_mut().poll(context).is_ready() {
            return Poll::Ready(Err(timeout_error(timeout_ms)));
        }
        Poll::Pending
    })
    .await
}

async fn execute_fetch(
    initial_url: Url,
    config: FetchConfig,
    deadline: u64,
) -> Result<WebFetchResult, WebFetchError> {
    let (final_url, response) = follow_redirects(initial_url, config, deadline).await?;
    let status = response.get_status_code();
    let content_type = response_header(&response, "content-type")?;
    let parsed_content_type = parse_content_type(content_type.as_deref())?;
    let max_bytes = usize::try_from(config.max_response_bytes).map_err(|_| {
        WebFetchError::InvalidSafetyLimit(
            "max_response_bytes does not fit this platform".to_string(),
        )
    })?;
    let (bytes, input_truncated) =
        read_bounded_body(response, max_bytes, deadline, config.timeout_ms).await?;
    let (content, truncated) = convert_content(
        &bytes,
        parsed_content_type,
        input_truncated,
        max_bytes,
        config.convert_html_to_text,
    )?;
    ensure_before_deadline(deadline, config.timeout_ms)?;
    Ok(WebFetchResult {
        final_url: final_url.into(),
        status,
        content_type,
        content,
        truncated,
    })
}

#[derive(Debug, Clone, PartialEq, Eq, IntoSchema, FromSchema, FromWire, IntoWire, WireSchema)]
pub struct WebFetchResult {
    /// The URL that produced the response after following redirects.
    #[schema(doc = "Final URL that produced the response after redirects.")]
    pub final_url: String,
    /// The HTTP response status code.
    #[schema(doc = "HTTP response status code, including non-success statuses.")]
    pub status: u16,
    /// The original response Content-Type header, without modification, when present.
    #[schema(doc = "Original response Content-Type header, when present.")]
    pub content_type: Option<String>,
    /// Bounded textual response content, optionally converted from HTML to readable text.
    #[schema(doc = "Bounded textual content, optionally converted from HTML to readable text.")]
    pub content: String,
    /// True when downloading or conversion stopped at the configured response-size limit.
    #[schema(doc = "Whether downloading or conversion reached the configured byte limit.")]
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, ToolError)]
pub enum WebFetchError {
    /// The supplied URL is malformed or contains credentials.
    #[tool_error(kind = "usage-error", exit_code = 2)]
    InvalidUrl(String),
    /// Only HTTP and HTTPS URLs are supported.
    #[tool_error(kind = "usage-error", exit_code = 2)]
    UnsupportedScheme(String),
    /// A redirect violated the tool's safety policy.
    #[tool_error(kind = "runtime-error", exit_code = 1)]
    UnsafeRedirect(String),
    /// The configured redirect limit was exceeded.
    #[tool_error(kind = "runtime-error", exit_code = 1)]
    RedirectLimit(String),
    /// The configured total request deadline elapsed.
    #[tool_error(kind = "runtime-error", exit_code = 1)]
    Timeout(String),
    /// The HTTP exchange failed while sending the request or receiving the response body.
    #[tool_error(kind = "runtime-error", exit_code = 1)]
    Transport(String),
    /// The response media type is not supported as text-oriented content.
    #[tool_error(kind = "runtime-error", exit_code = 1)]
    UnsupportedContentType(String),
    /// The response could not be decoded using its declared or default text encoding.
    #[tool_error(kind = "runtime-error", exit_code = 1)]
    InvalidTextEncoding(String),
    /// Text-oriented response conversion failed.
    #[tool_error(kind = "runtime-error", exit_code = 1)]
    ContentConversion(String),
    /// An invocation safety parameter is outside its immutable hard limit.
    #[tool_error(kind = "usage-error", exit_code = 2)]
    InvalidSafetyLimit(String),
}

impl From<&WebFetchError> for WebFetchError {
    fn from(value: &WebFetchError) -> Self {
        value.clone()
    }
}

#[tool_definition(version = "0.1.1")]
pub trait WebFetch {
    /// Retrieves text-oriented content with GET from an HTTP or HTTPS URL. Non-success statuses
    /// are returned normally when their bodies are supported. HTML is returned as decoded source
    /// unless `convert_html_to_text` is true.
    /// Downloaded bytes and final UTF-8 output are independently bounded by
    /// `max_response_bytes`; reaching either bound returns valid UTF-8 with `truncated` set.
    /// Completed results and typed errors replay without another HTTP request. If execution is
    /// interrupted before the result is recorded, recovery may retry the GET request.
    /// Optional invocation options support `--timeout-ms` in `1..=30000`,
    /// `--max-response-bytes` in `1..=5242880`, and `--max-redirects` in `0..=10`; omitted values
    /// use safe defaults. With zero redirects, a response requiring another request returns a
    /// redirect-limit error.
    #[command(annotations(
        read_only = true,
        destructive = false,
        idempotent = true,
        open_world = true
    ))]
    #[arg(
        url = "positional",
        doc = "Absolute HTTP or HTTPS URL without embedded credentials."
    )]
    #[arg(
        timeout_ms = "option",
        doc = "Optional total deadline in milliseconds (default 10000, maximum 30000)."
    )]
    #[arg(
        max_response_bytes = "option",
        doc = "Optional response and output byte limit (default 2097152, maximum 5242880)."
    )]
    #[arg(
        max_redirects = "option",
        doc = "Optional redirect limit (default 5, maximum 10); zero disables redirects."
    )]
    #[arg(
        convert_html_to_text = "option",
        doc = "Optionally convert HTML responses to readable text (default false)."
    )]
    async fn web_fetch(
        &self,
        url: String,
        timeout_ms: Option<u64>,
        max_response_bytes: Option<u64>,
        max_redirects: Option<u32>,
        convert_html_to_text: Option<bool>,
    ) -> Result<WebFetchResult, WebFetchError>;
}

struct WebFetchImpl;

#[tool_implementation]
impl WebFetch for WebFetchImpl {
    async fn web_fetch(
        &self,
        url: String,
        timeout_ms: Option<u64>,
        max_response_bytes: Option<u64>,
        max_redirects: Option<u32>,
        convert_html_to_text: Option<bool>,
    ) -> Result<WebFetchResult, WebFetchError> {
        let config = FetchConfig::from_options(
            timeout_ms,
            max_response_bytes,
            max_redirects,
            convert_html_to_text,
        )?;
        let initial_url = parse_url(&url)?;
        let durable_input = (
            initial_url.to_string(),
            config.timeout_ms,
            config.max_response_bytes,
            config.max_redirects,
            config.convert_html_to_text,
        );
        Durability::<WebFetchResult, WebFetchError>::new(
            "golem:web-fetch",
            "fetch",
            DurableFunctionType::WriteRemote,
            &durable_input,
        )
        .run_async(|| async move {
            let deadline =
                monotonic_clock::now().saturating_add(config.timeout_ms.saturating_mul(1_000_000));
            with_total_timeout(
                deadline,
                config.timeout_ms,
                execute_fetch(initial_url, config, deadline),
            )
            .await
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_uses_safe_defaults() {
        assert_eq!(
            FetchConfig::from_options(None, None, None, None).unwrap(),
            FetchConfig {
                timeout_ms: 10_000,
                max_response_bytes: 2 * 1024 * 1024,
                max_redirects: 5,
                convert_html_to_text: false,
            }
        );
    }

    #[test]
    fn config_accepts_overrides_at_hard_boundaries() {
        assert_eq!(
            FetchConfig::from_options(Some(1), Some(1), Some(10), Some(true)).unwrap(),
            FetchConfig {
                timeout_ms: 1,
                max_response_bytes: 1,
                max_redirects: MAX_REDIRECTS,
                convert_html_to_text: true,
            }
        );
        assert_eq!(
            FetchConfig::from_options(None, None, Some(0), None)
                .unwrap()
                .max_redirects,
            0
        );
    }

    #[test]
    fn config_rejects_out_of_range_values() {
        for options in [
            (Some(0), None, None),
            (Some(MAX_TIMEOUT_MS + 1), None, None),
            (None, Some(0), None),
            (None, Some(MAX_RESPONSE_BYTES + 1), None),
            (None, None, Some(MAX_REDIRECTS + 1)),
        ] {
            assert!(matches!(
                FetchConfig::from_options(options.0, options.1, options.2, None),
                Err(WebFetchError::InvalidSafetyLimit(_))
            ));
        }
    }

    #[test]
    fn url_validation_accepts_http_and_https_and_removes_fragments() {
        assert_eq!(
            parse_url("https://example.com/path?q=1#section")
                .unwrap()
                .as_str(),
            "https://example.com/path?q=1"
        );
        assert!(parse_url("http://[::1]:8080/").is_ok());
    }

    #[test]
    fn url_validation_rejects_credentials_and_other_schemes() {
        for url in [
            "https://user:password@example.com/",
            "https://user:secret@example.com:bad/",
        ] {
            let error = parse_url(url).unwrap_err();
            assert!(matches!(error, WebFetchError::InvalidUrl(_)));
            assert!(!format!("{error:?}").contains("password"));
            assert!(!format!("{error:?}").contains("secret"));
        }
        assert!(matches!(
            parse_url("file:///tmp/data"),
            Err(WebFetchError::UnsupportedScheme(_))
        ));
        assert!(matches!(
            parse_url("relative/path"),
            Err(WebFetchError::InvalidUrl(_))
        ));
    }

    #[test]
    fn redirect_statuses_are_explicit() {
        for status in [301, 302, 303, 307, 308] {
            assert!(is_redirect(status));
        }
        for status in [200, 300, 304, 305, 306, 400] {
            assert!(!is_redirect(status));
        }
    }

    #[test]
    fn redirect_resolution_strips_fragments_before_cycle_comparison() {
        let current = parse_url("https://example.com/a").unwrap();
        assert_eq!(
            resolve_redirect(&current, "/a#one").unwrap(),
            current,
            "a fragment-only redirect must resolve to the already visited request URL"
        );
        assert_eq!(
            resolve_redirect(&current, "/b#two").unwrap().as_str(),
            "https://example.com/b"
        );
        let error = resolve_redirect(&current, "https://user:secret@example.com/").unwrap_err();
        assert!(matches!(error, WebFetchError::UnsafeRedirect(_)));
        assert!(!format!("{error:?}").contains("secret"));
    }

    #[test]
    fn content_types_distinguish_text_html_and_binary() {
        assert_eq!(
            parse_content_type(Some("Text/HTML; charset=UTF-8"))
                .unwrap()
                .kind,
            ContentKind::Html
        );
        assert_eq!(
            parse_content_type(Some("application/problem+json"))
                .unwrap()
                .kind,
            ContentKind::Text
        );
        assert!(matches!(
            parse_content_type(Some("image/png")),
            Err(WebFetchError::UnsupportedContentType(_))
        ));
        assert!(matches!(
            parse_content_type(Some("image/svg+xml")),
            Err(WebFetchError::UnsupportedContentType(_))
        ));
    }

    #[test]
    fn decoding_honors_charset_and_rejects_invalid_text() {
        let windows_1252 = parse_content_type(Some("text/plain; charset=windows-1252")).unwrap();
        assert_eq!(
            decode_content(&[0x63, 0x61, 0x66, 0xe9], windows_1252, false).unwrap(),
            "café"
        );
        let quoted_parameter = parse_content_type(Some(
            "text/plain; note=\"x; charset=windows-1252\"; charset=utf-8",
        ))
        .unwrap();
        assert_eq!(
            decode_content("é".as_bytes(), quoted_parameter, false).unwrap(),
            "é"
        );
        let utf8 = parse_content_type(Some("text/plain; charset=utf-8")).unwrap();
        assert!(matches!(
            decode_content(&[0xff], utf8, false),
            Err(WebFetchError::InvalidTextEncoding(_))
        ));
        assert!(matches!(
            decode_content(b"a\0b", parse_content_type(None).unwrap(), false),
            Err(WebFetchError::UnsupportedContentType(_))
        ));

        let utf16 = parse_content_type(Some("text/plain; charset=utf-16")).unwrap();
        assert_eq!(
            decode_content(&[0xfe, 0xff, 0x00, 0x41], utf16, false).unwrap(),
            "A"
        );
        assert_eq!(
            decode_content(&[0xff, 0xfe, 0x41, 0x00], utf16, false).unwrap(),
            "A"
        );
        assert!(matches!(
            decode_content(&[0x00, 0x41], utf16, false),
            Err(WebFetchError::InvalidTextEncoding(_))
        ));
        assert_eq!(
            decode_content(&[0xfe, 0xff, 0x00], utf16, true).unwrap(),
            ""
        );
    }

    #[test]
    fn truncated_input_drops_only_an_incomplete_terminal_character() {
        let utf8 = parse_content_type(Some("text/plain; charset=utf-8")).unwrap();
        assert_eq!(decode_content(&[b'a', 0xc3], utf8, true).unwrap(), "a");
        assert!(matches!(
            decode_content(&[0xff, b'a'], utf8, true),
            Err(WebFetchError::InvalidTextEncoding(_))
        ));
    }

    #[test]
    fn output_truncation_preserves_utf8_boundaries() {
        let (content, truncated) = truncate_utf8("aéz".to_string(), 2);
        assert_eq!(content, "a");
        assert!(truncated);
        assert_eq!(
            truncate_utf8("aé".to_string(), 3),
            ("aé".to_string(), false)
        );
    }

    #[test]
    fn html_conversion_is_opt_in() {
        let html = parse_content_type(Some("text/html; charset=utf-8")).unwrap();
        let source = b"<html><head><title>Example</title><script>secret()</script></head><body><h1>Hello</h1><p>Readable text.</p></body></html>";
        let (content, truncated) = convert_content(source, html, false, 1024, false).unwrap();
        assert_eq!(content, String::from_utf8(source.to_vec()).unwrap());
        assert!(!truncated);

        let (content, truncated) = convert_content(source, html, false, 1024, true).unwrap();
        assert!(content.contains("Hello"), "{content}");
        assert!(content.contains("Readable text."), "{content}");
        assert!(!content.contains("secret"), "{content}");
        assert!(!truncated);
    }

    #[test]
    fn html_tables_are_normalized_before_rendering() {
        let content = convert_html("<table><tr><th>Name</th><td>Golem</td></tr></table>").unwrap();
        assert!(content.contains("Name"), "{content}");
        assert!(content.contains("Golem"), "{content}");
        assert!(!content.contains("NameGolem"), "{content}");
    }

    #[test]
    fn irregular_html_tables_do_not_create_a_dense_layout() {
        let mut source = String::from("<!-- ' --><table><tr>");
        for _ in 0..256 {
            source.push_str("<td>wide</td>");
        }
        source.push_str("</tr>");
        for _ in 0..256 {
            source.push_str("<tr><td>narrow</td></tr>");
        }
        source.push_str("</table>");

        let config = html2text::config::plain();
        let dom = config.parse_html(source.as_bytes()).unwrap();
        sanitize_html_dom(&dom.document).unwrap();
        let mut pending = vec![dom.document];
        while let Some(node) = pending.pop() {
            assert!(
                !node.element_name().is_some_and(|name| matches!(
                    name.as_str(),
                    "table" | "thead" | "tbody" | "tfoot" | "tr" | "td" | "th"
                )),
                "table layout element survived normalization"
            );
            pending.extend(node.children.borrow().iter().cloned());
        }

        let content = convert_html(&source).unwrap();
        assert!(content.contains("wide"), "{content}");
        assert!(content.contains("narrow"), "{content}");
    }

    #[test]
    fn nested_html_tables_are_replaced_once_per_layout_element() {
        let depth = 100;
        let mut source = "<table><tr><td>".repeat(depth);
        source.push('x');
        source.push_str(&"</td></tr></table>".repeat(depth));
        let config = html2text::config::plain();
        let dom = config.parse_html(source.as_bytes()).unwrap();

        assert_eq!(sanitize_html_dom(&dom.document).unwrap(), depth * 4);
        let mut pending = vec![dom.document];
        while let Some(node) = pending.pop() {
            assert!(!is_table_layout_element(&node));
            pending.extend(node.children.borrow().iter().cloned());
        }
    }

    #[test]
    fn excessively_deep_html_is_rejected_before_rendering() {
        let depth = MAX_HTML_DOM_DEPTH + 10;
        let source = format!("{}x{}", "<div>".repeat(depth), "</div>".repeat(depth));
        assert!(matches!(
            convert_html(&source),
            Err(WebFetchError::ContentConversion(message))
                if message.contains("maximum nesting depth")
        ));
    }

    #[test]
    fn html_render_amplification_is_rejected_before_rendering() {
        let mut source = "<blockquote>".repeat(200);
        source.push_str("<pre>");
        source.push_str(&"x\n".repeat(100_000));
        source.push_str("</pre>");
        source.push_str(&"</blockquote>".repeat(200));

        assert!(matches!(
            convert_html(&source),
            Err(WebFetchError::ContentConversion(message))
                if message.contains("render work budget")
        ));
    }

    #[test]
    fn normalized_html_newlines_count_toward_render_budget() {
        for lines in ["x\r".repeat(1_000_000), "x&#10;".repeat(410_000)] {
            let source = format!(
                "{}<pre>{lines}</pre>{}",
                "<blockquote>".repeat(40),
                "</blockquote>".repeat(40)
            );
            assert!(matches!(
                convert_html(&source),
                Err(WebFetchError::ContentConversion(message))
                    if message.contains("render work budget")
            ));
        }
    }

    #[test]
    fn all_indenting_html_elements_count_toward_render_budget() {
        let definition_list = format!(
            "{}<pre>{}</pre>{}",
            "<dl><dd>".repeat(40),
            "x\n".repeat(1_000_000),
            "</dd></dl>".repeat(40)
        );
        let ordered_list = format!(
            "{}{}{}",
            "<ol><li>".repeat(32),
            "x".repeat(4_000_000),
            "</li></ol>".repeat(32)
        );

        for source in [definition_list, ordered_list] {
            assert!(matches!(
                convert_html(&source),
                Err(WebFetchError::ContentConversion(message))
                    if message.contains("render work budget")
            ));
        }
    }

    #[test]
    fn preformatted_tabs_count_toward_render_budget() {
        for text in ["\tx".repeat(1_000_000), "&#9;x".repeat(700_000)] {
            let source = format!(
                "{}<pre>{text}</pre>{}",
                "<blockquote>".repeat(46),
                "</blockquote>".repeat(46)
            );
            assert!(matches!(
                convert_html(&source),
                Err(WebFetchError::ContentConversion(message))
                    if message.contains("render work budget")
            ));
        }
    }

    #[test]
    fn ordered_list_numbering_is_normalized_before_rendering() {
        for start in [i64::MIN, i64::MAX] {
            let content =
                convert_html(&format!("<ol start=\"{start}\"><li>a</li><li>b</li></ol>")).unwrap();
            assert!(content.contains("1. a"), "{content}");
            assert!(content.contains("2. b"), "{content}");
        }
    }

    #[test]
    fn nested_strikeout_does_not_amplify_html_output() {
        let depth = 100;
        let payload = "readable";
        let source = format!(
            "{}{}{}",
            "<s><del>".repeat(depth),
            payload,
            "</del></s>".repeat(depth)
        );
        let content = convert_html(&source).unwrap();
        assert_eq!(content.trim(), payload);
        assert!(!content.contains('\u{0336}'));
    }
}
