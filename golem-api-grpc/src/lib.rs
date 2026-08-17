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

#[cfg(test)]
test_r::enable!();

#[allow(clippy::large_enum_variant)]
pub mod proto {
    use crate::proto::golem::worker::UpdateMode;
    use desert_rust::{
        BinaryDeserializer, BinaryOutput, BinarySerializer, DeserializationContext,
        SerializationContext,
    };

    use uuid::Uuid;

    tonic::include_proto!("mod");

    pub const FILE_DESCRIPTOR_SET: &[u8] = tonic::include_file_descriptor_set!("services");

    impl From<Uuid> for golem::common::Uuid {
        fn from(value: Uuid) -> Self {
            let (high_bits, low_bits) = value.as_u64_pair();
            golem::common::Uuid {
                high_bits,
                low_bits,
            }
        }
    }

    impl From<golem::common::Uuid> for Uuid {
        fn from(value: golem::common::Uuid) -> Self {
            let high_bits = value.high_bits;
            let low_bits = value.low_bits;
            Uuid::from_u64_pair(high_bits, low_bits)
        }
    }

    impl BinarySerializer for UpdateMode {
        fn serialize<Output: BinaryOutput>(
            &self,
            context: &mut SerializationContext<Output>,
        ) -> desert_rust::Result<()> {
            match self {
                UpdateMode::Automatic => 0u8.serialize(context),
                UpdateMode::Manual => 1u8.serialize(context),
            }
        }
    }

    impl BinaryDeserializer for UpdateMode {
        fn deserialize(context: &mut DeserializationContext<'_>) -> desert_rust::Result<Self> {
            match u8::deserialize(context)? {
                0u8 => Ok(UpdateMode::Automatic),
                1u8 => Ok(UpdateMode::Manual),
                other => Err(desert_rust::Error::InvalidConstructorId {
                    constructor_id: other as u32,
                    type_name: "UpdateMode".to_string(),
                }),
            }
        }
    }
}

use proto::golem::worker::{
    InvocationCancel, InvocationFrame, InvocationStart, invocation_cancel, invocation_frame,
};

pub fn expect_invocation_start(frame: InvocationFrame) -> Result<InvocationStart, String> {
    match frame.frame {
        Some(invocation_frame::Frame::Start(start)) => Ok(start),
        Some(_) => Err("the first invocation frame must be start".to_string()),
        None => Err("invocation frame has no payload".to_string()),
    }
}

pub fn validate_invocation_request_tail(frame: InvocationFrame) -> Result<InvocationFrame, String> {
    match frame.frame.as_ref() {
        Some(
            invocation_frame::Frame::Demand(_)
            | invocation_frame::Frame::Item(_)
            | invocation_frame::Frame::End(_)
            | invocation_frame::Frame::StreamError(_)
            | invocation_frame::Frame::Detach(_),
        ) => Ok(frame),
        Some(invocation_frame::Frame::Cancel(cancel)) => {
            let kind = cancel.kind;
            invocation_cancel::Kind::try_from(kind)
                .map(|_| frame)
                .map_err(|_| format!("invalid invocation cancellation kind {kind}"))
        }
        Some(invocation_frame::Frame::Start(_)) => {
            Err("invocation start may only appear as the first frame".to_string())
        }
        Some(invocation_frame::Frame::Result(_) | invocation_frame::Frame::Finished(_)) => {
            Err("response frame received on invocation request stream".to_string())
        }
        None => Err("invocation frame has no payload".to_string()),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InvocationResponsePhase {
    BeforeResult,
    AfterResult,
    Complete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvocationResponseState {
    phase: InvocationResponsePhase,
}

impl Default for InvocationResponseState {
    fn default() -> Self {
        Self {
            phase: InvocationResponsePhase::BeforeResult,
        }
    }
}

impl InvocationResponseState {
    pub fn validate(&mut self, frame: &InvocationFrame) -> Result<(), String> {
        if self.phase == InvocationResponsePhase::Complete {
            return Err("invocation response frame received after completion".to_string());
        }

        match frame.frame.as_ref() {
            Some(invocation_frame::Frame::Start(_)) | Some(invocation_frame::Frame::Cancel(_)) => {
                Err("request frame received on invocation response stream".to_string())
            }
            Some(invocation_frame::Frame::Result(_)) => match self.phase {
                InvocationResponsePhase::BeforeResult => {
                    self.phase = InvocationResponsePhase::AfterResult;
                    Ok(())
                }
                InvocationResponsePhase::AfterResult => {
                    Err("invocation response contains more than one result".to_string())
                }
                InvocationResponsePhase::Complete => unreachable!(),
            },
            Some(invocation_frame::Frame::Finished(finished)) => {
                use proto::golem::worker::invocation_session_finished::Outcome;

                match finished.outcome.as_ref() {
                    Some(Outcome::Success(_))
                        if self.phase == InvocationResponsePhase::BeforeResult =>
                    {
                        return Err(
                            "invocation completed successfully before publishing a result"
                                .to_string(),
                        );
                    }
                    Some(Outcome::ProtocolFailure(failure)) => {
                        if proto::golem::worker::invocation_protocol_failure::Kind::try_from(
                            failure.kind,
                        )
                        .is_err()
                        {
                            return Err(format!(
                                "invalid invocation protocol failure kind {}",
                                failure.kind
                            ));
                        }
                    }
                    Some(Outcome::Success(_) | Outcome::Failure(_)) => {}
                    None => return Err("invocation completion has no outcome".to_string()),
                }
                self.phase = InvocationResponsePhase::Complete;
                Ok(())
            }
            Some(
                invocation_frame::Frame::Demand(_)
                | invocation_frame::Frame::Item(_)
                | invocation_frame::Frame::End(_)
                | invocation_frame::Frame::StreamError(_)
                | invocation_frame::Frame::Detach(_),
            ) => Ok(()),
            None => Err("invocation frame has no payload".to_string()),
        }
    }

    pub fn is_complete(&self) -> bool {
        self.phase == InvocationResponsePhase::Complete
    }
}

pub fn invocation_cancel_frame(
    kind: invocation_cancel::Kind,
    details: Option<String>,
) -> InvocationFrame {
    InvocationFrame {
        frame: Some(invocation_frame::Frame::Cancel(InvocationCancel {
            kind: kind as i32,
            details,
        })),
    }
}

#[cfg(test)]
mod protocol_tests {
    use super::*;
    use proto::golem::common::Empty;
    use proto::golem::worker::invocation_session_finished::Outcome;
    use proto::golem::worker::{InvocationResult, InvocationSessionFinished, StreamDemand};
    use test_r::test;

    fn response_frame(frame: invocation_frame::Frame) -> InvocationFrame {
        InvocationFrame { frame: Some(frame) }
    }

    fn successful_completion() -> invocation_frame::Frame {
        invocation_frame::Frame::Finished(InvocationSessionFinished {
            outcome: Some(Outcome::Success(Empty {})),
        })
    }

    #[test]
    fn request_start_rules() {
        assert!(expect_invocation_start(InvocationFrame::default()).is_err());
        assert!(
            expect_invocation_start(response_frame(invocation_frame::Frame::Result(
                InvocationResult::default(),
            )))
            .is_err()
        );
        assert!(
            expect_invocation_start(response_frame(invocation_frame::Frame::Start(
                InvocationStart::default(),
            )))
            .is_ok()
        );
    }

    #[test]
    fn request_tail_rules() {
        assert!(
            validate_invocation_request_tail(response_frame(invocation_frame::Frame::Demand(
                StreamDemand { stream_id: 1 },
            )))
            .is_ok()
        );
        assert!(
            validate_invocation_request_tail(invocation_cancel_frame(
                invocation_cancel::Kind::Semantic,
                None,
            ))
            .is_ok()
        );

        let invalid = [
            InvocationFrame::default(),
            response_frame(invocation_frame::Frame::Start(InvocationStart::default())),
            response_frame(invocation_frame::Frame::Result(InvocationResult::default())),
            response_frame(successful_completion()),
        ];
        for frame in invalid {
            assert!(validate_invocation_request_tail(frame).is_err());
        }
    }

    #[test]
    fn response_requires_one_result_before_successful_completion() {
        let mut state = InvocationResponseState::default();
        assert!(
            state
                .validate(&response_frame(invocation_frame::Frame::Demand(
                    StreamDemand { stream_id: 1 },
                )))
                .is_ok()
        );
        assert!(
            state
                .validate(&response_frame(successful_completion()))
                .is_err()
        );
        assert!(
            state
                .validate(&response_frame(invocation_frame::Frame::Result(
                    InvocationResult::default(),
                )))
                .is_ok()
        );
        assert!(
            state
                .validate(&response_frame(invocation_frame::Frame::Result(
                    InvocationResult::default(),
                )))
                .is_err()
        );
        assert!(
            state
                .validate(&response_frame(successful_completion()))
                .is_ok()
        );
        assert!(state.is_complete());
    }

    #[test]
    fn response_failure_is_a_single_completion() {
        let mut state = InvocationResponseState::default();
        let failure = || {
            response_frame(invocation_frame::Frame::Finished(
                InvocationSessionFinished {
                    outcome: Some(Outcome::ProtocolFailure(
                        proto::golem::worker::InvocationProtocolFailure {
                            kind: proto::golem::worker::invocation_protocol_failure::Kind::Protocol
                                as i32,
                            details: "failed".to_string(),
                        },
                    )),
                },
            ))
        };
        assert!(state.validate(&failure()).is_ok());
        assert!(state.is_complete());
        assert!(state.validate(&failure()).is_err());
    }

    #[test]
    fn response_rejects_request_and_empty_frames() {
        let invalid = [
            InvocationFrame::default(),
            response_frame(invocation_frame::Frame::Start(InvocationStart::default())),
            invocation_cancel_frame(invocation_cancel::Kind::Semantic, None),
        ];
        for frame in invalid {
            assert!(InvocationResponseState::default().validate(&frame).is_err());
        }
    }
}
