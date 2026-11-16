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

use super::*;
use proptest::prelude::*;
use test_r::test;

const OPEN: AdmissionState = AdmissionState::Open;
const REPLAY: AdmissionState = AdmissionState::ReplayOpen;
const TRANSITIONING: AdmissionState = AdmissionState::Transitioning;
const REPLAY_TRANSITIONING: AdmissionState = AdmissionState::ReplayTransitioning;

#[test]
fn every_admission_state_and_event_has_an_explicit_outcome() {
    use AccessError::{Revoked, Transitioning};
    use AdmissionEvent::*;
    use AdmissionState::{Invalidated, Sealed};

    let events = [
        BeginTransition,
        EnableReplayAccess,
        BeginReplayDrain,
        FinishTransition,
        Seal,
        Invalidate,
    ];
    let cases = [
        (
            OPEN,
            [
                Ok(TRANSITIONING),
                Ok(REPLAY),
                Err(Transitioning),
                Ok(OPEN),
                Ok(Sealed),
                Ok(Invalidated),
            ],
        ),
        (
            REPLAY,
            [
                Ok(REPLAY_TRANSITIONING),
                Ok(REPLAY),
                Ok(TRANSITIONING),
                Ok(REPLAY),
                Ok(Sealed),
                Ok(Invalidated),
            ],
        ),
        (
            TRANSITIONING,
            [
                Err(Transitioning),
                Ok(TRANSITIONING),
                Err(Transitioning),
                Ok(OPEN),
                Ok(Sealed),
                Ok(Invalidated),
            ],
        ),
        (
            REPLAY_TRANSITIONING,
            [
                Err(Transitioning),
                Ok(REPLAY_TRANSITIONING),
                Err(Transitioning),
                Ok(REPLAY),
                Ok(Sealed),
                Ok(Invalidated),
            ],
        ),
        (
            Sealed,
            [
                Err(Revoked),
                Ok(Sealed),
                Err(Revoked),
                Ok(Sealed),
                Ok(Sealed),
                Ok(Invalidated),
            ],
        ),
        (
            Invalidated,
            [
                Err(Revoked),
                Ok(Invalidated),
                Err(Revoked),
                Ok(Invalidated),
                Ok(Invalidated),
                Ok(Invalidated),
            ],
        ),
    ];

    for (state, outcomes) in cases {
        for (event, expected) in events.into_iter().zip(outcomes) {
            assert_eq!(transition(state, event), expected, "{state:?}, {event:?}");
            let mut admission = Admission { state };
            assert_eq!(admission.apply(event), expected.map(|_| ()));
            assert_eq!(admission.state, expected.unwrap_or(state));
        }
    }
}

#[test]
fn admission_queries_cover_calls_replay_handles_and_already_admitted_opens() {
    use AccessError::{Revoked, Transitioning};
    use AdmissionState::{Invalidated, Sealed};

    let cases = [
        (OPEN, Ok(()), false, true, false),
        (REPLAY, Ok(()), true, true, false),
        (TRANSITIONING, Err(Transitioning), false, true, false),
        (REPLAY_TRANSITIONING, Err(Transitioning), true, true, false),
        (Sealed, Err(Revoked), false, false, false),
        (Invalidated, Err(Revoked), false, false, true),
    ];
    for (state, call, replay, node, invalidated) in cases {
        let admission = Admission { state };
        assert_eq!(admission.admit_call(), call, "{state:?}");
        assert_eq!(admission.replay_access_allowed(), replay, "{state:?}");
        assert_eq!(admission.can_return_node(), node, "{state:?}");
        assert_eq!(admission.is_invalidated(), invalidated, "{state:?}");
    }
}

#[test]
fn ordinary_transitions_keep_replay_access_but_replay_drain_revokes_it() {
    let mut admission = Admission::new();
    assert_eq!(admission.state, OPEN);
    admission.enable_replay_access();
    admission.begin_transition().unwrap();
    assert!(admission.replay_access_allowed());
    assert_eq!(admission.admit_call(), Err(AccessError::Transitioning));
    admission.finish_transition();
    assert!(admission.replay_access_allowed());
    assert_eq!(admission.admit_call(), Ok(()));

    admission.begin_replay_drain().unwrap();
    assert!(!admission.replay_access_allowed());
    assert_eq!(admission.admit_call(), Err(AccessError::Transitioning));
    admission.finish_transition();
    assert!(!admission.replay_access_allowed());
    assert_eq!(admission.admit_call(), Ok(()));
    assert_eq!(
        admission.begin_replay_drain(),
        Err(AccessError::Transitioning)
    );
}

#[test]
fn a_late_transition_completion_or_replay_enable_cannot_reopen_terminal_admission() {
    for invalidated in [false, true] {
        let mut admission = Admission::new();
        admission.enable_replay_access();
        admission.begin_transition().unwrap();
        if invalidated {
            admission.invalidate();
        }
        admission.seal();
        admission.finish_transition();
        admission.enable_replay_access();

        assert_eq!(admission.admit_call(), Err(AccessError::Revoked));
        assert!(!admission.replay_access_allowed());
        assert!(!admission.can_return_node());
        assert_eq!(admission.is_invalidated(), invalidated);
    }
}

fn admission_states() -> impl Strategy<Value = AdmissionState> {
    prop::sample::select(vec![
        OPEN,
        REPLAY,
        TRANSITIONING,
        REPLAY_TRANSITIONING,
        AdmissionState::Sealed,
        AdmissionState::Invalidated,
    ])
}

fn admission_events() -> impl Strategy<Value = AdmissionEvent> {
    prop::sample::select(vec![
        AdmissionEvent::BeginTransition,
        AdmissionEvent::EnableReplayAccess,
        AdmissionEvent::BeginReplayDrain,
        AdmissionEvent::FinishTransition,
        AdmissionEvent::Seal,
        AdmissionEvent::Invalidate,
    ])
}

proptest! {
    #[test]
    fn invalidated_admission_never_recovers(
        initial in admission_states(),
        events in prop::collection::vec(admission_events(), 0..128),
    ) {
        let mut admission = Admission { state: initial };
        for event in std::iter::once(AdmissionEvent::Invalidate).chain(events) {
            let _ = admission.apply(event);
            prop_assert!(admission.is_invalidated());
            prop_assert_eq!(admission.admit_call(), Err(AccessError::Revoked));
            prop_assert!(!admission.replay_access_allowed());
            prop_assert!(!admission.can_return_node());
        }
    }

    #[test]
    fn sealed_admission_never_reopens(
        initial in admission_states(),
        events in prop::collection::vec(admission_events(), 0..128),
    ) {
        let mut admission = Admission { state: initial };
        for event in std::iter::once(AdmissionEvent::Seal).chain(events) {
            let _ = admission.apply(event);
            prop_assert!(matches!(
                admission.state,
                AdmissionState::Sealed | AdmissionState::Invalidated
            ));
            prop_assert_eq!(admission.admit_call(), Err(AccessError::Revoked));
            prop_assert!(!admission.replay_access_allowed());
            prop_assert!(!admission.can_return_node());
        }
    }

    #[test]
    fn replay_drain_revokes_access_until_it_is_explicitly_enabled_again(
        events in prop::collection::vec(admission_events(), 0..128),
    ) {
        let mut admission = Admission::new();
        admission.enable_replay_access();
        admission.begin_replay_drain().unwrap();
        prop_assert!(!admission.replay_access_allowed());
        let mut replay_revoked = true;

        for event in events {
            let previous = admission.state;
            let result = admission.apply(event);
            match event {
                AdmissionEvent::EnableReplayAccess
                    if matches!(previous, AdmissionState::Open | AdmissionState::ReplayOpen) => {
                        replay_revoked = false;
                    }
                AdmissionEvent::BeginReplayDrain if result.is_ok() => {
                    replay_revoked = true;
                }
                AdmissionEvent::Seal | AdmissionEvent::Invalidate => {
                    replay_revoked = true;
                }
                _ => {}
            }
            if replay_revoked {
                prop_assert!(!admission.replay_access_allowed());
            }
        }
    }
}
