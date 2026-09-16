use super::cancellation_handle;
use test_r::test;
use test_r::timeout;
use tokio_util::sync::CancellationToken;

#[test]
#[timeout("5s")]
async fn adapter_handle_observes_caller_cancellation() {
    let token = CancellationToken::new();
    let handle = cancellation_handle(Some(token.clone()));
    assert!(!handle.is_cancelled());

    token.cancel();

    handle.cancelled().await;
    assert!(handle.is_cancelled());
}

#[test]
#[timeout("5s")]
async fn adapter_handle_without_live_cancellation_stays_pending() {
    let handle = cancellation_handle(None);
    assert!(!handle.is_cancelled());
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(10), handle.cancelled())
            .await
            .is_err()
    );
}
