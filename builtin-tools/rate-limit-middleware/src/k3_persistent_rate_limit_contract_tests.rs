use super::*;

fn backend() -> RateLimitBackendImpl {
    RateLimitBackendImpl::new("k3-contract".to_string())
}

#[test]
fn k3_rate_1_admission_key_is_only_the_canonical_principal() {
    assert_eq!(principal_key(&Principal::Anonymous), "anonymous");
}

#[test]
fn k3_rate_1_fixed_windows_refill_only_at_epoch_boundaries() {
    let key = "principal-a".to_string();
    let mut backend = backend();

    let first = backend.admit_at(key.clone(), "window-1-a".into(), 2, 1_000, 1_001);
    let second = backend.admit_at(key.clone(), "window-1-b".into(), 2, 1_000, 1_999);
    let rejected = backend.admit_at(key.clone(), "window-1-c".into(), 2, 1_000, 1_999);
    let refilled = backend.admit_at(key.clone(), "window-2-a".into(), 2, 1_000, 2_000);
    let rolled_back = backend.admit_at(key, "window-1-d".into(), 2, 1_000, 1_500);

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
    let mut backend = backend();

    let admitted = backend.admit_at(key.clone(), "stable-admitted".into(), 1, 1_000, 999);
    let rejected = backend.admit_at(key.clone(), "stable-rejected".into(), 1, 1_000, 999);
    let admitted_replay = backend.admit_at(key.clone(), "stable-admitted".into(), 1, 1_000, 5_000);
    let rejected_replay = backend.admit_at(key, "stable-rejected".into(), 1, 1_000, 5_000);

    assert!(admitted.admitted);
    assert!(!rejected.admitted);
    assert!(admitted_replay.admitted && admitted_replay.duplicate);
    assert!(!rejected_replay.admitted && rejected_replay.duplicate);
    assert_eq!(backend.stats().attempts, 4);
    assert_eq!(backend.stats().committed_charges, 1);
    assert_eq!(backend.stats().recorded_decisions, 2);
}
