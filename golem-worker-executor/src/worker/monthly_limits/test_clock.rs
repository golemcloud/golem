use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, watch};

#[derive(Debug, Eq, PartialEq)]
pub struct MonthlyTimerPollForTest {
    pub now: Duration,
    pub deadline: Duration,
}

pub struct MonthlyClockForTest {
    now: watch::Sender<Duration>,
    polls: mpsc::UnboundedSender<MonthlyTimerPollForTest>,
}

impl MonthlyClockForTest {
    pub fn new() -> (Arc<Self>, mpsc::UnboundedReceiver<MonthlyTimerPollForTest>) {
        let (now, _) = watch::channel(Duration::ZERO);
        let (polls, receiver) = mpsc::unbounded_channel();
        (Arc::new(Self { now, polls }), receiver)
    }

    pub fn advance(&self, elapsed: Duration) {
        self.now.send_modify(|now| *now += elapsed);
    }

    pub fn active_sleeps(&self) -> usize {
        self.now.receiver_count()
    }

    pub(super) async fn sleep(&self, duration: Duration) {
        let mut updates = self.now.subscribe();
        let deadline = *updates.borrow() + duration;
        loop {
            let now = *updates.borrow_and_update();
            let _ = self.polls.send(MonthlyTimerPollForTest { now, deadline });
            if now >= deadline {
                return;
            }
            updates.changed().await.expect("clock owns the sender");
        }
    }
}
