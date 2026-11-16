// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1 (https://license.golem.cloud/LICENSE).

use super::*;

#[cfg(all(feature = "guest", not(feature = "host")))]
pub use crate::schema::wit::guest::generated::golem::core::http as wire;
#[cfg(all(feature = "host", not(feature = "guest")))]
pub use crate::schema::wit::host::generated::golem::core::http as wire;

impl From<HttpMethod> for wire::HttpMethod {
    fn from(value: HttpMethod) -> Self {
        match value {
            HttpMethod::Get(_) => Self::Get,
            HttpMethod::Head(_) => Self::Head,
            HttpMethod::Post(_) => Self::Post,
            HttpMethod::Put(_) => Self::Put,
            HttpMethod::Delete(_) => Self::Delete,
            HttpMethod::Connect(_) => Self::Connect,
            HttpMethod::Options(_) => Self::Options,
            HttpMethod::Trace(_) => Self::Trace,
            HttpMethod::Patch(_) => Self::Patch,
            HttpMethod::Custom(value) => Self::Custom(value.value),
        }
    }
}

impl From<wire::HttpMethod> for HttpMethod {
    fn from(value: wire::HttpMethod) -> Self {
        match value {
            wire::HttpMethod::Get => Self::Get(Empty {}),
            wire::HttpMethod::Head => Self::Head(Empty {}),
            wire::HttpMethod::Post => Self::Post(Empty {}),
            wire::HttpMethod::Put => Self::Put(Empty {}),
            wire::HttpMethod::Delete => Self::Delete(Empty {}),
            wire::HttpMethod::Connect => Self::Connect(Empty {}),
            wire::HttpMethod::Options => Self::Options(Empty {}),
            wire::HttpMethod::Trace => Self::Trace(Empty {}),
            wire::HttpMethod::Patch => Self::Patch(Empty {}),
            wire::HttpMethod::Custom(value) => Self::Custom(CustomHttpMethod { value }),
        }
    }
}

impl From<PathSegment> for wire::PathSegment {
    fn from(value: PathSegment) -> Self {
        match value {
            PathSegment::Literal(value) => Self::Literal(value.value),
            PathSegment::PathVariable(value) => Self::PathVariable(value.into()),
            PathSegment::RemainingPathVariable(value) => Self::RemainingPathVariable(value.into()),
        }
    }
}

impl From<wire::PathSegment> for PathSegment {
    fn from(value: wire::PathSegment) -> Self {
        match value {
            wire::PathSegment::Literal(value) => Self::Literal(LiteralSegment { value }),
            wire::PathSegment::PathVariable(value) => Self::PathVariable(value.into()),
            wire::PathSegment::RemainingPathVariable(value) => {
                Self::RemainingPathVariable(value.into())
            }
        }
    }
}

impl From<PathVariable> for wire::PathVariable {
    fn from(value: PathVariable) -> Self {
        Self {
            variable_name: value.variable_name,
        }
    }
}

impl From<wire::PathVariable> for PathVariable {
    fn from(value: wire::PathVariable) -> Self {
        Self {
            variable_name: value.variable_name,
        }
    }
}

impl From<HeaderVariable> for wire::HeaderVariable {
    fn from(value: HeaderVariable) -> Self {
        Self {
            header_name: value.header_name,
            variable_name: value.variable_name,
        }
    }
}

impl From<wire::HeaderVariable> for HeaderVariable {
    fn from(value: wire::HeaderVariable) -> Self {
        Self {
            header_name: value.header_name,
            variable_name: value.variable_name,
        }
    }
}

impl From<QueryVariable> for wire::QueryVariable {
    fn from(value: QueryVariable) -> Self {
        Self {
            query_param_name: value.query_param_name,
            variable_name: value.variable_name,
        }
    }
}

impl From<wire::QueryVariable> for QueryVariable {
    fn from(value: wire::QueryVariable) -> Self {
        Self {
            query_param_name: value.query_param_name,
            variable_name: value.variable_name,
        }
    }
}

impl From<FileMapping> for wire::FileMapping {
    fn from(value: FileMapping) -> Self {
        match value {
            FileMapping::Exact(value) => Self::Exact(wire::ExactFileMapping {
                public_path: value.public_path,
                file_path: value.file_path,
            }),
            FileMapping::Subtree(value) => Self::Subtree(wire::SubtreeFileMapping {
                public_prefix: value.public_prefix,
                filesystem_root: value.filesystem_root,
            }),
        }
    }
}

impl From<wire::FileMapping> for FileMapping {
    fn from(value: wire::FileMapping) -> Self {
        match value {
            wire::FileMapping::Exact(value) => Self::Exact(ExactFileMapping {
                public_path: value.public_path,
                file_path: value.file_path,
            }),
            wire::FileMapping::Subtree(value) => Self::Subtree(SubtreeFileMapping {
                public_prefix: value.public_prefix,
                filesystem_root: value.filesystem_root,
            }),
        }
    }
}

impl From<FileResponseHeader> for wire::FileResponseHeader {
    fn from(value: FileResponseHeader) -> Self {
        Self {
            name: value.name,
            value: value.value,
        }
    }
}

impl From<wire::FileResponseHeader> for FileResponseHeader {
    fn from(value: wire::FileResponseHeader) -> Self {
        Self {
            name: value.name,
            value: value.value,
        }
    }
}

impl From<HttpRouteMode> for wire::HttpRouteMode {
    fn from(value: HttpRouteMode) -> Self {
        match value {
            HttpRouteMode::DurableStreams => Self::DurableStreams,
            HttpRouteMode::Plain => Self::Plain,
            HttpRouteMode::Sse => Self::Sse,
            HttpRouteMode::WebSocket => Self::WebSocket,
        }
    }
}

impl From<wire::HttpRouteMode> for HttpRouteMode {
    fn from(value: wire::HttpRouteMode) -> Self {
        match value {
            wire::HttpRouteMode::DurableStreams => Self::DurableStreams,
            wire::HttpRouteMode::Plain => Self::Plain,
            wire::HttpRouteMode::Sse => Self::Sse,
            wire::HttpRouteMode::WebSocket => Self::WebSocket,
        }
    }
}

impl From<DurableStreamRouteOptions> for wire::DurableStreamRouteOptions {
    fn from(value: DurableStreamRouteOptions) -> Self {
        Self {
            slots: value.slots.into_iter().map(Into::into).collect(),
            load: value.load.map(Into::into),
        }
    }
}

impl From<wire::DurableStreamRouteOptions> for DurableStreamRouteOptions {
    fn from(value: wire::DurableStreamRouteOptions) -> Self {
        Self {
            slots: value.slots.into_iter().map(Into::into).collect(),
            load: value.load.map(Into::into),
        }
    }
}

impl From<DurableStreamSlotOptions> for wire::DurableStreamSlotOptions {
    fn from(value: DurableStreamSlotOptions) -> Self {
        Self {
            source: value.source.into(),
            name: value.name,
            content_type: value.content_type,
        }
    }
}

impl From<wire::DurableStreamSlotOptions> for DurableStreamSlotOptions {
    fn from(value: wire::DurableStreamSlotOptions) -> Self {
        Self {
            source: value.source.into(),
            name: value.name,
            content_type: value.content_type,
        }
    }
}

impl From<DurableStreamSlotSource> for wire::DurableStreamSlotSource {
    fn from(value: DurableStreamSlotSource) -> Self {
        match value {
            DurableStreamSlotSource::Input(source) => Self::Input(source.name),
            DurableStreamSlotSource::Output(source) => Self::Output(source.name),
        }
    }
}

impl From<wire::DurableStreamSlotSource> for DurableStreamSlotSource {
    fn from(value: wire::DurableStreamSlotSource) -> Self {
        match value {
            wire::DurableStreamSlotSource::Input(name) => {
                Self::Input(DurableStreamInputSlotSource { name })
            }
            wire::DurableStreamSlotSource::Output(name) => {
                Self::Output(DurableStreamOutputSlotSource { name })
            }
        }
    }
}

impl From<DurableStreamRouteLoadOptions> for wire::DurableStreamRouteLoadOptions {
    fn from(value: DurableStreamRouteLoadOptions) -> Self {
        Self {
            max_concurrent_readers_per_stream: value.max_concurrent_readers_per_stream,
            max_append_requests_per_second_per_stream: value
                .max_append_requests_per_second_per_stream,
        }
    }
}

impl From<wire::DurableStreamRouteLoadOptions> for DurableStreamRouteLoadOptions {
    fn from(value: wire::DurableStreamRouteLoadOptions) -> Self {
        Self {
            max_concurrent_readers_per_stream: value.max_concurrent_readers_per_stream,
            max_append_requests_per_second_per_stream: value
                .max_append_requests_per_second_per_stream,
        }
    }
}
