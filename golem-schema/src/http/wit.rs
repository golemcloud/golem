// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1 (https://license.golem.cloud/LICENSE).

use super::*;

#[cfg(all(feature = "guest", not(feature = "host")))]
pub use crate::schema::wit::guest::generated::golem::core::http as wire;
#[cfg(all(feature = "host", not(feature = "guest")))]
pub use crate::schema::wit::host::generated::golem::core::http as wire;

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
