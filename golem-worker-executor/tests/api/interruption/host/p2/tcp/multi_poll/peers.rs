use super::*;
use tokio::net::{TcpListener, TcpStream};

#[derive(Debug, Default)]
pub(super) struct PeerEffects {
    pub accepts: [usize; 2],
    pub sent: [usize; 2],
    pub closed: [usize; 2],
}

pub(super) async fn listeners() -> anyhow::Result<[TcpListener; 2]> {
    Ok([
        TcpListener::bind("127.0.0.1:0").await?,
        TcpListener::bind("127.0.0.1:0").await?,
    ])
}

pub(super) async fn accept_pair(
    listeners: &[TcpListener; 2],
    effects: &mut PeerEffects,
) -> anyhow::Result<[TcpStream; 2]> {
    let mut accepted = Vec::new();
    for (index, listener) in listeners.iter().enumerate() {
        let (peer, _) = tokio::time::timeout(Duration::from_secs(10), listener.accept())
            .await
            .context("TCP peer accept")??;
        effects.accepts[index] += 1;
        accepted.push(peer);
    }
    Ok(accepted.try_into().unwrap())
}

pub(super) async fn release_second(
    peers: &mut [TcpStream; 2],
    effects: &mut PeerEffects,
) -> anyhow::Result<()> {
    peers[1].write_all(b"x").await?;
    effects.sent[1] += 1;
    Ok(())
}

pub(super) async fn closed_pair(
    peers: &mut [TcpStream; 2],
    effects: &mut PeerEffects,
) -> anyhow::Result<()> {
    for (index, peer) in peers.iter_mut().enumerate() {
        let mut byte = [0];
        let read = tokio::time::timeout(Duration::from_secs(10), peer.read(&mut byte))
            .await
            .context("physical TCP socket closure")?;
        // The guest polls without consuming the byte. A native close with unread input may reset.
        ensure!(
            matches!(read, Ok(0))
                || matches!(&read, Err(error) if error.kind() == std::io::ErrorKind::ConnectionReset),
            "peer {index} was not closed: {read:?}"
        );
        effects.closed[index] += 1;
    }
    Ok(())
}

pub(super) async fn no_connections(listeners: &[TcpListener; 2]) -> anyhow::Result<()> {
    for listener in listeners {
        ensure!(
            tokio::time::timeout(Duration::from_millis(100), listener.accept())
                .await
                .is_err(),
            "unexpected native connection"
        );
    }
    Ok(())
}

pub(super) async fn pending_phase(
    phases: &mut tokio::sync::mpsc::UnboundedReceiver<P2PollPendingForTest>,
    key: &IdempotencyKey,
    count: usize,
) -> anyhow::Result<P2PollPendingForTest> {
    tokio::time::timeout(Duration::from_secs(10), async {
        let mut setup = Vec::new();
        loop {
            let phase = phases.recv().await.context("native poll observer closed")?;
            if phase.pollable_reps.len() == 1 {
                setup.push(phase);
                continue;
            }
            ensure!(
                setup
                    .iter()
                    .all(|phase| phase.state() == P2PollStateForTest::Returned)
            );
            ensure!(phase.invocation_key.as_ref() == Some(key));
            ensure!(phase.pollable_reps.len() == count);
            if count == 2 {
                ensure!(phase.pollable_reps[0] != phase.pollable_reps[1]);
            } else {
                ensure!(count == 3);
                ensure!(phase.pollable_reps[0] == phase.pollable_reps[2]);
                ensure!(phase.pollable_reps[0] != phase.pollable_reps[1]);
            }
            ensure!(
                phase.state() == P2PollStateForTest::Pending,
                "stale Pending: {phase:?}"
            );
            return Ok(phase);
        }
    })
    .await
    .context("native two-input poll Pending")?
}
