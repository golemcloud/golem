// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! ProtoJSON and schema-directed REQUIRED-field restoration (§5.7).
use a2a::*;
use a2a_pb::protojson_conv::{self, ProtoJsonPayload, ProtoJsonPayloadError};
use serde_json::{Value, json};

/// Types supported by the shared wire codec.
pub trait WirePayload: Sized {
    fn encode(&self) -> Result<Value, ProtoJsonPayloadError>;
    fn decode(value: Value) -> Result<Self, ProtoJsonPayloadError>;
}

pub fn to_value<T: WirePayload>(value: &T) -> Result<Value, ProtoJsonPayloadError> {
    value.encode()
}

pub fn from_value<T: WirePayload>(value: Value) -> Result<T, ProtoJsonPayloadError> {
    T::decode(value)
}

macro_rules! upstream {
    ($($ty:ident => $kind:ident),* $(,)?) => {$ (
        impl WirePayload for $ty {
            fn encode(&self) -> Result<Value, ProtoJsonPayloadError> {
                let mut value = protojson_conv::to_value(self)?;
                normalize(&mut value, Kind::$kind);
                Ok(value)
            }
            fn decode(value: Value) -> Result<Self, ProtoJsonPayloadError> {
                protojson_conv::from_value(value)
            }
        }
    )*};
}

upstream! {
    AgentCard => Card, Task => Task, ListTasksResponse => List,
    SendMessageResponse => Envelope, StreamResponse => Envelope,
    SendMessageRequest => Envelope,
    GetTaskRequest => IdRequest, ListTasksRequest => Other, CancelTaskRequest => IdRequest,
    SubscribeToTaskRequest => IdRequest, GetExtendedAgentCardRequest => Other,
    GetTaskPushNotificationConfigRequest => ConfigRequest,
    DeleteTaskPushNotificationConfigRequest => ConfigRequest,
    ListTaskPushNotificationConfigsRequest => ConfigListRequest,
    TaskPushNotificationConfig => PushConfig, ListTaskPushNotificationConfigsResponse => ConfigList,
}

// Upstream exposes converters but not ProtoJsonPayload impls for these objects.
struct Payload<T>(T);

macro_rules! object {
    ($ty:ident, $kind:ident, $to:ident, $from:ident) => {
        impl ProtoJsonPayload for Payload<$ty> {
            type Proto = a2a_pb::proto::$ty;
            type ProtoJson = a2a_pb::protojson::$ty;
            fn to_proto(value: &Self) -> Self::Proto {
                a2a_pb::pbconv::$to(&value.0)
            }
            fn try_from_proto(value: &Self::Proto) -> Result<Self, ProtoJsonPayloadError> {
                a2a_pb::pbconv::$from(value)
                    .map(Payload)
                    .map_err(ProtoJsonPayloadError::Invalid)
            }
        }
        impl WirePayload for $ty {
            fn encode(&self) -> Result<Value, ProtoJsonPayloadError> {
                let mut value = protojson_conv::to_value(&Payload(self.clone()))?;
                normalize(&mut value, Kind::$kind);
                Ok(value)
            }
            fn decode(value: Value) -> Result<Self, ProtoJsonPayloadError> {
                protojson_conv::from_value::<Payload<Self>>(value).map(|value| value.0)
            }
        }
    };
}

object!(Part, Other, to_proto_part, from_proto_part);
object!(Message, Message, to_proto_message, from_proto_message);
object!(Artifact, Artifact, to_proto_artifact, from_proto_artifact);
object!(
    TaskStatusUpdateEvent,
    StatusUpdate,
    to_proto_task_status_update_event,
    from_proto_task_status_update_event
);
object!(
    TaskArtifactUpdateEvent,
    ArtifactUpdate,
    to_proto_task_artifact_update_event,
    from_proto_task_artifact_update_event
);

#[derive(Clone, Copy)]
enum Kind {
    Other,
    Card,
    Task,
    Message,
    Artifact,
    Status,
    StatusUpdate,
    ArtifactUpdate,
    Envelope,
    List,
    Interface,
    Skill,
    Provider,
    Signature,
    SecuritySchemes,
    SecurityScheme,
    ApiKey,
    HttpAuth,
    OAuth2,
    OpenId,
    Flows,
    AuthorizationCode,
    ClientCredentials,
    DeviceCode,
    IdRequest,
    ConfigRequest,
    ConfigListRequest,
    ConfigList,
    SendConfig,
    PushConfig,
    Authentication,
}

// These defaults follow v1.0.1 specification/a2a.proto's REQUIRED annotations.
// Traverse only protocol fields: metadata and Part.data belong to the caller.
fn normalize(value: &mut Value, kind: Kind) {
    let defaults = match kind {
        Kind::Card => {
            json!({"name":"", "description":"", "supportedInterfaces":[], "version":"", "capabilities":{}, "defaultInputModes":[], "defaultOutputModes":[], "skills":[]})
        }
        Kind::Task => json!({"id":"", "status":{}}),
        Kind::Message => json!({"messageId":"", "role":"ROLE_UNSPECIFIED", "parts":[]}),
        Kind::Artifact => json!({"artifactId":"", "parts":[]}),
        Kind::Status => json!({"state":"TASK_STATE_UNSPECIFIED"}),
        Kind::StatusUpdate => json!({"taskId":"", "contextId":"", "status":{}}),
        Kind::ArtifactUpdate => json!({"taskId":"", "contextId":"", "artifact":{}}),
        Kind::List => json!({"tasks":[], "nextPageToken":"", "pageSize":0, "totalSize":0}),
        Kind::Interface => json!({"url":"", "protocolBinding":"", "protocolVersion":""}),
        Kind::Skill => json!({"id":"", "name":"", "description":"", "tags":[]}),
        Kind::Provider => json!({"url":"", "organization":""}),
        Kind::Signature => json!({"protected":"", "signature":""}),
        Kind::ApiKey => json!({"location":"", "name":""}),
        Kind::HttpAuth => json!({"scheme":""}),
        Kind::OAuth2 => json!({"flows":{}}),
        Kind::OpenId => json!({"openIdConnectUrl":""}),
        Kind::AuthorizationCode => json!({"authorizationUrl":"", "tokenUrl":"", "scopes":{}}),
        Kind::ClientCredentials => json!({"tokenUrl":"", "scopes":{}}),
        Kind::DeviceCode => json!({"deviceAuthorizationUrl":"", "tokenUrl":"", "scopes":{}}),
        Kind::IdRequest => json!({"id":""}),
        Kind::ConfigRequest => json!({"taskId":"", "id":""}),
        Kind::ConfigListRequest => json!({"taskId":""}),
        Kind::PushConfig => json!({"url":""}),
        Kind::Authentication => json!({"scheme":""}),
        Kind::Other
        | Kind::Envelope
        | Kind::SecuritySchemes
        | Kind::SecurityScheme
        | Kind::ConfigList
        | Kind::SendConfig
        | Kind::Flows => json!({}),
    };
    let Some(map) = value.as_object_mut() else {
        return;
    };
    for (key, default) in defaults.as_object().unwrap() {
        map.entry(key.clone()).or_insert_with(|| default.clone());
    }
    if matches!(kind, Kind::SecuritySchemes) {
        for scheme in map.values_mut() {
            normalize(scheme, Kind::SecurityScheme);
        }
        return;
    }
    let children: &[(&str, Kind)] = match kind {
        Kind::Card => &[
            ("supportedInterfaces", Kind::Interface),
            ("skills", Kind::Skill),
            ("provider", Kind::Provider),
            ("signatures", Kind::Signature),
            ("securitySchemes", Kind::SecuritySchemes),
        ],
        Kind::SecurityScheme => &[
            ("apiKeySecurityScheme", Kind::ApiKey),
            ("httpAuthSecurityScheme", Kind::HttpAuth),
            ("oauth2SecurityScheme", Kind::OAuth2),
            ("openIdConnectSecurityScheme", Kind::OpenId),
        ],
        Kind::OAuth2 => &[("flows", Kind::Flows)],
        Kind::Flows => &[
            ("authorizationCode", Kind::AuthorizationCode),
            ("clientCredentials", Kind::ClientCredentials),
            ("deviceCode", Kind::DeviceCode),
        ],
        Kind::Task => &[
            ("status", Kind::Status),
            ("history", Kind::Message),
            ("artifacts", Kind::Artifact),
        ],
        Kind::Status => &[("message", Kind::Message)],
        Kind::StatusUpdate => &[("status", Kind::Status)],
        Kind::ArtifactUpdate => &[("artifact", Kind::Artifact)],
        Kind::Envelope => &[
            ("task", Kind::Task),
            ("message", Kind::Message),
            ("statusUpdate", Kind::StatusUpdate),
            ("artifactUpdate", Kind::ArtifactUpdate),
            ("configuration", Kind::SendConfig),
        ],
        Kind::List => &[("tasks", Kind::Task)],
        Kind::ConfigList => &[("configs", Kind::PushConfig)],
        Kind::SendConfig => &[("taskPushNotificationConfig", Kind::PushConfig)],
        Kind::PushConfig => &[("authentication", Kind::Authentication)],
        _ => &[],
    };
    for (key, child) in children {
        if let Some(value) = map.get_mut(*key) {
            if let Some(items) = value.as_array_mut() {
                for item in items {
                    normalize(item, *child);
                }
            } else {
                normalize(value, *child);
            }
        }
    }
}
