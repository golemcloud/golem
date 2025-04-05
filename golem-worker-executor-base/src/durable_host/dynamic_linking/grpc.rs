use std::str::FromStr;

use http::{uri::PathAndQuery, Uri};
use hyper_rustls::HttpsConnectorBuilder;
use prost_reflect::{prost::Message, DynamicMessage, MethodDescriptor};
use serde::{Deserialize, Serialize};
use tonic::{client::Grpc, transport::Channel, Status};

pub struct GrpcClient {
    grpc: Grpc<Channel>,
}

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct GrpcConfiguration {
    pub url: String,
    pub secret_token: String,
}

impl GrpcClient {
    pub async fn new(uri: &Uri) -> anyhow::Result<GrpcClient> {
        let channel_builder = Channel::builder(uri.clone());

        match uri.scheme().map(|s| s.as_str()) {
            Some("http") => {
                let channel = channel_builder.connect().await?;
                Ok(GrpcClient {
                    grpc: Grpc::new(channel),
                })
            }
            Some("https") => {
                let mut root_store = rustls::RootCertStore::empty();
                let certs = rustls_native_certs::load_native_certs()
                    .expect("failed to load platform certs");

                for cert in certs {
                    root_store.add(cert.into_owned())?;
                }

                let channel = channel_builder
                    .connect_with_connector(
                        HttpsConnectorBuilder::new()
                            .with_tls_config(
                                rustls::ClientConfig::builder()
                                    .with_root_certificates(root_store)
                                    .with_no_client_auth(),
                            )
                            .https_only()
                            .enable_http2()
                            .build(),
                    )
                    .await?;

                Ok(GrpcClient {
                    grpc: Grpc::new(channel),
                })
            }
            _ => Err(anyhow::anyhow!("Unsupported scheme")),
        }
    }

    pub async fn unary_call(
        mut self,
        method: &prost_reflect::MethodDescriptor,
        message: &prost_reflect::DynamicMessage,
        metadata: tonic::metadata::MetadataMap,
    ) -> Result<tonic::Response<prost_reflect::DynamicMessage>, Status> {
        let path = PathAndQuery::from_str(
            format!("/{}/{}", method.parent_service().full_name(), method.name()).as_str(),
        )
        .unwrap();

        self.grpc
            .ready()
            .await
            .map_err(|err| Status::from_error(err.into()))?;

        let request =
            tonic::Request::from_parts(metadata, tonic::Extensions::default(), message.clone());

        self.grpc
            .unary(request, path, CustomCodec::new(method.clone()))
            .await
    }
}

#[derive(Clone, Debug)]
struct CustomCodec(MethodDescriptor);

impl CustomCodec {
    fn new(method: MethodDescriptor) -> Self {
        Self(method)
    }
}

impl tonic::codec::Codec for CustomCodec {
    type Encode = DynamicMessage;

    type Decode = DynamicMessage;

    type Encoder = CustomCodec;

    type Decoder = CustomCodec;

    fn encoder(&mut self) -> Self::Encoder {
        self.clone()
    }

    fn decoder(&mut self) -> Self::Decoder {
        self.clone()
    }
}

impl tonic::codec::Encoder for CustomCodec {
    type Item = DynamicMessage;
    type Error = Status;

    fn encode(
        &mut self,
        message: Self::Item,
        dst: &mut tonic::codec::EncodeBuf<'_>,
    ) -> Result<(), Self::Error> {
        message.encode(dst).expect("Failed to encode message");
        Ok(())
    }
}

impl tonic::codec::Decoder for CustomCodec {
    type Item = DynamicMessage;
    type Error = Status;

    fn decode(
        &mut self,
        src: &mut tonic::codec::DecodeBuf<'_>,
    ) -> Result<Option<Self::Item>, Self::Error> {
        let mut message: DynamicMessage = DynamicMessage::new(self.0.output());

        message
            .merge(src)
            .map_err(|err| Status::from_error(err.into()))?;

        Ok(Some(message))
    }
}

#[derive(Serialize, Debug)]
pub struct GrpcStatus {
    #[serde[serialize_with = "code_serializer"]]
    pub code: tonic::Code, // its a enum

    pub message: String,
    pub details: Vec<u8>,
}

// code_serializer
fn code_serializer<S>(code: &tonic::Code, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    let code_str = match code {
        tonic::Code::Ok => "ok",
        tonic::Code::Cancelled => "cancelled",
        tonic::Code::Unknown => "unknown",
        tonic::Code::InvalidArgument => "invalid-argument",
        tonic::Code::DeadlineExceeded => "deadline-exceeded",
        tonic::Code::NotFound => "not-found",
        tonic::Code::AlreadyExists => "already-exists",
        tonic::Code::PermissionDenied => "permission-denied",
        tonic::Code::ResourceExhausted => "resource-exhausted",
        tonic::Code::FailedPrecondition => "failed-precondition",
        tonic::Code::Aborted => "aborted",
        tonic::Code::OutOfRange => "out-of-range",
        tonic::Code::Unimplemented => "unimplemented",
        tonic::Code::Internal => "internal",
        tonic::Code::Unavailable => "unavailable",
        tonic::Code::DataLoss => "data-loss",
        tonic::Code::Unauthenticated => "unauthenticated",
    };
    serializer.serialize_str(code_str)
}
