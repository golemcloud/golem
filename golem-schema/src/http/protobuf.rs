// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1 (https://license.golem.cloud/LICENSE).

use super::*;
use crate::proto::golem::http as proto;

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
