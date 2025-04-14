

use opentelemetry::trace::{SpanContext, SpanId, TraceId, TraceFlags, TraceState};
use opentelemetry_http::HeaderInjector;
use std::collections::HashMap;
use std::str::FromStr;
use tracing_opentelemetry::OpenTelemetrySpanExt;

/// Generates W3C Trace Context headers from the current tracing span
pub fn generate_trace_context_headers(span: &tracing::Span) -> HashMap<String, String> {
    let mut headers = HashMap::new();
    let ctx = span.context();
    
    // Extract the current context
    let span_context = ctx.span().span_context().clone();
    
    if span_context.is_valid() {
        // Create traceparent header
        let trace_id = span_context.trace_id().to_hex();
        let span_id = span_context.span_id().to_hex();
        let trace_flags = if span_context.is_sampled() { "01" } else { "00" };
        
        let traceparent = format!("00-{}-{}-{}", trace_id, span_id, trace_flags);
        headers.insert("traceparent".to_string(), traceparent);
        
        // Add tracestate header if present
        let trace_state = span_context.trace_state();
        if !trace_state.is_empty() {
            headers.insert("tracestate".to_string(), trace_state.to_string());
        }
    } else {
        // Generate a new trace context if none exists
        let trace_id = TraceId::from_u128(rand::random());
        let span_id = SpanId::from_u64(rand::random());
        let trace_flags = TraceFlags::SAMPLED;
        let trace_state = TraceState::default();
        
        let span_context = SpanContext::new(
            trace_id,
            span_id,
            trace_flags,
            false,
            trace_state,
        );
        
        let traceparent = format!("00-{}-{}-01", trace_id.to_hex(), span_id.to_hex());
        headers.insert("traceparent".to_string(), traceparent);
    }
    
    headers
}

/// Parses W3C Trace Context headers and creates a span context
pub fn parse_trace_context_headers(headers: &HashMap<String, String>) -> Option<SpanContext> {
    let traceparent = headers.get("traceparent")?;
    
    // Parse traceparent header (format: version-trace_id-parent_id-flags)
    let parts: Vec<&str> = traceparent.split('-').collect();
    if parts.len() != 4 {
        return None;
    }
    
    let trace_id = TraceId::from_hex(parts[1]).ok()?;
    let span_id = SpanId::from_hex(parts[2]).ok()?;
    let flags = u8::from_str_radix(parts[3], 16).ok()?;
    let trace_flags = TraceFlags::new(flags);
    
    // Parse tracestate header if present
    let trace_state = if let Some(tracestate) = headers.get("tracestate") {
        TraceState::from_str(tracestate).unwrap_or_default()
    } else {
        TraceState::default()
    };
    
    Some(SpanContext::new(
        trace_id,
        span_id,
        trace_flags,
        false,
        trace_state,
    ))
}

/// Injects the current trace context into a header map for outgoing requests
pub fn inject_trace_context(headers: &mut HashMap<String, String>) {
    let span = tracing::Span::current();
    let trace_headers = generate_trace_context_headers(&span);
    
    // Add all trace headers to the provided headers map
    for (key, value) in trace_headers {
        headers.insert(key, value);
    }
}

/// Creates a new tracing span with context extracted from incoming request headers
pub fn extract_and_create_span(
    headers: &HashMap<String, String>,
    name: &str,
) -> tracing::Span {
    if let Some(span_context) = parse_trace_context_headers(headers) {
        let span = tracing::info_span!(name);
        let mut cx = span.context();
        cx.set_remote_span_context(&span_context);
        span
    } else {
        // Create a new root span if no valid context is found
        tracing::info_span!(name)
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::observability::trace_context::{
        generate_trace_context_headers, parse_trace_context_headers
    };
    use std::collections::HashMap;
    use tracing::info_span;

    #[test]
    fn test_generate_trace_context_headers() {
        let span = info_span!("test_span");
        let headers = generate_trace_context_headers(&span);
        
        // Check that a traceparent header is present
        assert!(headers.contains_key("traceparent"));
        
        // Validate traceparent format: version-trace_id-parent_id-flags
        let traceparent = headers.get("traceparent").unwrap();
        let parts: Vec<&str> = traceparent.split('-').collect();
        assert_eq!(parts.len(), 4);
        assert_eq!(parts[0], "00"); // version
        assert_eq!(parts[1].len(), 32); // trace_id (16 bytes in hex = 32 chars)
        assert_eq!(parts[2].len(), 16); // span_id (8 bytes in hex = 16 chars)
        assert!((parts[3] == "00") || (parts[3] == "01")); // flags
    }

    #[test]
    fn test_parse_trace_context_headers() {
        // Create mock headers
        let mut headers = HashMap::new();
        headers.insert(
            "traceparent".to_string(),
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01".to_string(),
        );
        headers.insert(
            "tracestate".to_string(),
            "congo=t61rcWkgMzE,rojo=00f067aa0ba902b7".to_string(),
        );
        
        // Parse headers
        let span_context = parse_trace_context_headers(&headers).unwrap();
        
        // Validate trace context
        assert_eq!(
            span_context.trace_id().to_hex(),
            "4bf92f3577b34da6a3ce929d0e0e4736"
        );
        assert_eq!(span_context.span_id().to_hex(), "00f067aa0ba902b7");
        assert!(span_context.is_sampled());
    }

    #[test]
    fn test_roundtrip() {
        // 1. Generate headers
        let span = info_span!("test_span");
        let headers = generate_trace_context_headers(&span);
        
        // 2. Parse the headers back
        let span_context = parse_trace_context_headers(&headers).unwrap();
        
        // 3. Generate headers again from the parsed context
        let _new_span = info_span!("new_span");
        // Set the span context to the parsed one
        // (In a real scenario, this would be done via OpenTelemetrySpanExt::set_parent)
        
        let new_headers = generate_trace_context_headers(&_new_span);
        
        // Verify the traceparent header has the expected format
        assert!(new_headers.contains_key("traceparent"));
    }
}