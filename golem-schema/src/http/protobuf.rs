// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1 (https://license.golem.cloud/LICENSE).

use super::*;
use crate::proto::golem::http as proto;

impl TryFrom<proto::HttpMethod> for HttpMethod {
    type Error = String;

    fn try_from(value: proto::HttpMethod) -> Result<Self, Self::Error> {
        use proto::{StandardHttpMethod, http_method::Value};
        match value.value.ok_or("Missing HttpMethod.value")? {
            Value::Standard(value) => {
                match StandardHttpMethod::try_from(value).map_err(|_| "Unknown HTTP method")? {
                    StandardHttpMethod::Get => Ok(Self::Get(Empty {})),
                    StandardHttpMethod::Head => Ok(Self::Head(Empty {})),
                    StandardHttpMethod::Post => Ok(Self::Post(Empty {})),
                    StandardHttpMethod::Put => Ok(Self::Put(Empty {})),
                    StandardHttpMethod::Delete => Ok(Self::Delete(Empty {})),
                    StandardHttpMethod::Connect => Ok(Self::Connect(Empty {})),
                    StandardHttpMethod::Options => Ok(Self::Options(Empty {})),
                    StandardHttpMethod::Trace => Ok(Self::Trace(Empty {})),
                    StandardHttpMethod::Patch => Ok(Self::Patch(Empty {})),
                    StandardHttpMethod::Unspecified => Err("Unknown HTTP method".into()),
                }
            }
            Value::Custom(value) => Ok(Self::Custom(CustomHttpMethod { value })),
        }
    }
}

impl From<HttpMethod> for proto::HttpMethod {
    fn from(value: HttpMethod) -> Self {
        use proto::{StandardHttpMethod, http_method::Value};
        Self {
            value: Some(match value {
                HttpMethod::Get(_) => Value::Standard(StandardHttpMethod::Get.into()),
                HttpMethod::Head(_) => Value::Standard(StandardHttpMethod::Head.into()),
                HttpMethod::Post(_) => Value::Standard(StandardHttpMethod::Post.into()),
                HttpMethod::Put(_) => Value::Standard(StandardHttpMethod::Put.into()),
                HttpMethod::Delete(_) => Value::Standard(StandardHttpMethod::Delete.into()),
                HttpMethod::Connect(_) => Value::Standard(StandardHttpMethod::Connect.into()),
                HttpMethod::Options(_) => Value::Standard(StandardHttpMethod::Options.into()),
                HttpMethod::Trace(_) => Value::Standard(StandardHttpMethod::Trace.into()),
                HttpMethod::Patch(_) => Value::Standard(StandardHttpMethod::Patch.into()),
                HttpMethod::Custom(value) => Value::Custom(value.value),
            }),
        }
    }
}

impl TryFrom<proto::PathSegment> for PathSegment {
    type Error = String;

    fn try_from(value: proto::PathSegment) -> Result<Self, Self::Error> {
        use proto::path_segment::Value;
        match value.value.ok_or("Missing PathSegment.value")? {
            Value::Literal(value) => Ok(Self::Literal(LiteralSegment { value: value.value })),
            Value::PathVariable(value) => Ok(Self::PathVariable(value.into())),
            Value::RemainingPathVariable(value) => Ok(Self::RemainingPathVariable(value.into())),
        }
    }
}

impl From<PathSegment> for proto::PathSegment {
    fn from(value: PathSegment) -> Self {
        use proto::path_segment::Value;
        Self {
            value: Some(match value {
                PathSegment::Literal(value) => {
                    Value::Literal(proto::LiteralSegment { value: value.value })
                }
                PathSegment::PathVariable(value) => Value::PathVariable(value.into()),
                PathSegment::RemainingPathVariable(value) => {
                    Value::RemainingPathVariable(value.into())
                }
            }),
        }
    }
}

impl From<proto::LiteralSegment> for LiteralSegment {
    fn from(value: proto::LiteralSegment) -> Self {
        Self { value: value.value }
    }
}

impl From<LiteralSegment> for proto::LiteralSegment {
    fn from(value: LiteralSegment) -> Self {
        Self { value: value.value }
    }
}

impl From<proto::PathVariable> for PathVariable {
    fn from(value: proto::PathVariable) -> Self {
        Self {
            variable_name: value.variable_name,
        }
    }
}

impl From<PathVariable> for proto::PathVariable {
    fn from(value: PathVariable) -> Self {
        Self {
            variable_name: value.variable_name,
        }
    }
}

impl From<proto::HeaderVariable> for HeaderVariable {
    fn from(value: proto::HeaderVariable) -> Self {
        Self {
            header_name: value.header_name,
            variable_name: value.variable_name,
        }
    }
}

impl From<HeaderVariable> for proto::HeaderVariable {
    fn from(value: HeaderVariable) -> Self {
        Self {
            header_name: value.header_name,
            variable_name: value.variable_name,
        }
    }
}

impl From<proto::QueryVariable> for QueryVariable {
    fn from(value: proto::QueryVariable) -> Self {
        Self {
            query_param_name: value.query_param_name,
            variable_name: value.variable_name,
        }
    }
}

impl From<QueryVariable> for proto::QueryVariable {
    fn from(value: QueryVariable) -> Self {
        Self {
            query_param_name: value.query_param_name,
            variable_name: value.variable_name,
        }
    }
}

impl TryFrom<proto::FileMapping> for FileMapping {
    type Error = String;

    fn try_from(value: proto::FileMapping) -> Result<Self, Self::Error> {
        use proto::file_mapping::Value;
        match value.value.ok_or("Missing FileMapping.value")? {
            Value::Exact(value) => Ok(Self::Exact(ExactFileMapping {
                public_path: value.public_path,
                file_path: value.file_path,
            })),
            Value::Subtree(value) => Ok(Self::Subtree(SubtreeFileMapping {
                public_prefix: value.public_prefix,
                filesystem_root: value.filesystem_root,
            })),
        }
    }
}

impl From<FileMapping> for proto::FileMapping {
    fn from(value: FileMapping) -> Self {
        use proto::file_mapping::Value;
        Self {
            value: Some(match value {
                FileMapping::Exact(value) => Value::Exact(proto::ExactFileMapping {
                    public_path: value.public_path,
                    file_path: value.file_path,
                }),
                FileMapping::Subtree(value) => Value::Subtree(proto::SubtreeFileMapping {
                    public_prefix: value.public_prefix,
                    filesystem_root: value.filesystem_root,
                }),
            }),
        }
    }
}

impl From<proto::FileResponseHeader> for FileResponseHeader {
    fn from(value: proto::FileResponseHeader) -> Self {
        Self {
            name: value.name,
            value: value.value,
        }
    }
}

impl From<FileResponseHeader> for proto::FileResponseHeader {
    fn from(value: FileResponseHeader) -> Self {
        Self {
            name: value.name,
            value: value.value,
        }
    }
}

impl From<HttpRouteMode> for proto::HttpRouteMode {
    fn from(value: HttpRouteMode) -> Self {
        match value {
            HttpRouteMode::DurableStreams => Self::DurableStreams,
            HttpRouteMode::Plain => Self::Plain,
            HttpRouteMode::Sse => Self::Sse,
            HttpRouteMode::WebSocket => Self::WebSocket,
        }
    }
}

impl TryFrom<i32> for HttpRouteMode {
    type Error = String;

    fn try_from(value: i32) -> Result<Self, Self::Error> {
        match proto::HttpRouteMode::try_from(value).map_err(|e| e.to_string())? {
            proto::HttpRouteMode::Unspecified => Err("Invalid or missing HTTP route mode".into()),
            proto::HttpRouteMode::DurableStreams => Ok(Self::DurableStreams),
            proto::HttpRouteMode::Plain => Ok(Self::Plain),
            proto::HttpRouteMode::Sse => Ok(Self::Sse),
            proto::HttpRouteMode::WebSocket => Ok(Self::WebSocket),
        }
    }
}

impl TryFrom<proto::DurableStreamRouteOptions> for DurableStreamRouteOptions {
    type Error = String;

    fn try_from(value: proto::DurableStreamRouteOptions) -> Result<Self, Self::Error> {
        Ok(Self {
            slots: value
                .slots
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
            load: value.load.map(Into::into),
        })
    }
}

impl From<DurableStreamRouteOptions> for proto::DurableStreamRouteOptions {
    fn from(value: DurableStreamRouteOptions) -> Self {
        Self {
            slots: value.slots.into_iter().map(Into::into).collect(),
            load: value.load.map(Into::into),
        }
    }
}

impl TryFrom<proto::DurableStreamSlotOptions> for DurableStreamSlotOptions {
    type Error = String;

    fn try_from(value: proto::DurableStreamSlotOptions) -> Result<Self, Self::Error> {
        Ok(Self {
            source: value
                .source
                .ok_or("Missing field: durable_stream_slot_options.source")?
                .try_into()?,
            name: value.name,
            content_type: value.content_type,
        })
    }
}

impl From<DurableStreamSlotOptions> for proto::DurableStreamSlotOptions {
    fn from(value: DurableStreamSlotOptions) -> Self {
        Self {
            source: Some(value.source.into()),
            name: value.name,
            content_type: value.content_type,
        }
    }
}

impl TryFrom<proto::DurableStreamSlotSource> for DurableStreamSlotSource {
    type Error = String;

    fn try_from(value: proto::DurableStreamSlotSource) -> Result<Self, Self::Error> {
        use proto::durable_stream_slot_source::Value;
        match value
            .value
            .ok_or("Missing oneof: durable_stream_slot_source.value")?
        {
            Value::Input(name) => Ok(Self::Input(DurableStreamInputSlotSource { name })),
            Value::Output(name) => Ok(Self::Output(DurableStreamOutputSlotSource { name })),
        }
    }
}

impl From<DurableStreamSlotSource> for proto::DurableStreamSlotSource {
    fn from(value: DurableStreamSlotSource) -> Self {
        use proto::durable_stream_slot_source::Value;
        Self {
            value: Some(match value {
                DurableStreamSlotSource::Input(source) => Value::Input(source.name),
                DurableStreamSlotSource::Output(source) => Value::Output(source.name),
            }),
        }
    }
}

impl From<proto::DurableStreamRouteLoadOptions> for DurableStreamRouteLoadOptions {
    fn from(value: proto::DurableStreamRouteLoadOptions) -> Self {
        Self {
            max_concurrent_readers_per_stream: value.max_concurrent_readers_per_stream,
            max_append_requests_per_second_per_stream: value
                .max_append_requests_per_second_per_stream,
        }
    }
}

impl From<DurableStreamRouteLoadOptions> for proto::DurableStreamRouteLoadOptions {
    fn from(value: DurableStreamRouteLoadOptions) -> Self {
        Self {
            max_concurrent_readers_per_stream: value.max_concurrent_readers_per_stream,
            max_append_requests_per_second_per_stream: value
                .max_append_requests_per_second_per_stream,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    #[test]
    fn route_modes_round_trip_and_reject_unspecified_or_unknown_values() {
        for mode in [
            HttpRouteMode::DurableStreams,
            HttpRouteMode::Plain,
            HttpRouteMode::Sse,
            HttpRouteMode::WebSocket,
        ] {
            let encoded = proto::HttpRouteMode::from(mode) as i32;
            assert_eq!(HttpRouteMode::try_from(encoded).unwrap(), mode);
        }
        for invalid in [0, -1, 5] {
            assert!(HttpRouteMode::try_from(invalid).is_err());
        }
    }
}
