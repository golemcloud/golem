use super::*;
use test_r::test;

fn backend(limit: u64) -> RateLimitBackendImpl {
    RateLimitBackendImpl::new("k3-contract".to_string(), limit, 1_000)
}

#[test]
fn configuration_change_preserves_old_decisions_and_independent_capacity() {
    let mut old = backend(1);
    let admitted = old.admit_at("principal".into(), "admitted".into(), 999);
    let rejected = old.admit_at("principal".into(), "rejected".into(), 999);
    let mut changed_limit = backend(2);
    let mut changed_window = RateLimitBackendImpl::new("k3-contract".into(), 1, 2_000);

    // The same logical IDs in another configuration must not reuse old decisions or counters.
    for backend in [&mut changed_limit, &mut changed_window] {
        let fresh = backend.admit_at("principal".into(), "rejected".into(), 999);
        assert!(fresh.admitted && !fresh.duplicate);
    }
    assert!(
        changed_limit
            .admit_at("principal".into(), "second".into(), 999)
            .admitted
    );
    assert!(
        !changed_window
            .admit_at("principal".into(), "second".into(), 1_000)
            .admitted
    );
    assert!(
        old.admit_at("principal".into(), "next-window".into(), 1_000)
            .admitted
    );

    // Replay pinned to the old configuration still returns both original decisions.
    let mut admitted_replay = old.admit_at("principal".into(), "admitted".into(), 5_000);
    let mut rejected_replay = old.admit_at("principal".into(), "rejected".into(), 5_000);
    assert!(admitted_replay.duplicate && rejected_replay.duplicate);
    admitted_replay.duplicate = false;
    rejected_replay.duplicate = false;
    assert_eq!(admitted_replay, admitted);
    assert_eq!(rejected_replay, rejected);
    assert_eq!(old.stats().committed_charges, 2);
}

#[test]
fn k3_rate_1_admission_key_is_only_the_canonical_principal() {
    assert_eq!(principal_key(&Principal::Anonymous), "anonymous");
}

#[test]
fn k3_rate_1_fixed_windows_refill_only_at_epoch_boundaries() {
    let key = "principal-a".to_string();
    let mut backend = backend(2);

    let first = backend.admit_at(key.clone(), "window-1-a".into(), 1_001);
    let second = backend.admit_at(key.clone(), "window-1-b".into(), 1_999);
    let rejected = backend.admit_at(key.clone(), "window-1-c".into(), 1_999);
    let refilled = backend.admit_at(key.clone(), "window-2-a".into(), 2_000);
    let rolled_back = backend.admit_at(key, "window-1-d".into(), 1_500);

    assert!(first.admitted);
    assert!(second.admitted);
    assert!(!rejected.admitted);
    assert_eq!(rejected.retry_after_milliseconds, 1);
    assert!(
        refilled.admitted,
        "the next epoch-aligned window must refill"
    );
    assert!(
        !rolled_back.admitted,
        "returning to a previously exhausted window must not refill it again"
    );
    assert_eq!(backend.stats().attempts, 5);
    assert_eq!(backend.stats().committed_charges, 3);
    assert_eq!(backend.stats().recorded_decisions, 5);
}

#[test]
fn k3_rate_3_stable_invocation_ids_preserve_admitted_and_rejected_decisions() {
    let key = "principal-a".to_string();
    let mut backend = backend(1);

    let admitted = backend.admit_at(key.clone(), "stable-admitted".into(), 999);
    let rejected = backend.admit_at(key.clone(), "stable-rejected".into(), 999);
    let admitted_replay = backend.admit_at(key.clone(), "stable-admitted".into(), 5_000);
    let rejected_replay = backend.admit_at(key, "stable-rejected".into(), 5_000);

    assert!(admitted.admitted);
    assert!(!rejected.admitted);
    assert!(admitted_replay.admitted && admitted_replay.duplicate);
    assert!(!rejected_replay.admitted && rejected_replay.duplicate);
    assert_eq!(backend.stats().attempts, 4);
    assert_eq!(backend.stats().committed_charges, 1);
    assert_eq!(backend.stats().recorded_decisions, 2);
}
