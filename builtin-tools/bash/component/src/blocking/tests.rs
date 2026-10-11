use super::*;
use std::rc::Rc;
use std::task::Waker;

fn poll_once(task: &mut ShellTask) -> Poll<()> {
    Pin::new(task).poll(&mut Context::from_waker(Waker::noop()))
}

/// A task that writes `mark` to `log` when it runs.
fn marking(log: &Rc<RefCell<Vec<u8>>>, mark: u8) -> ShellTask {
    let log = Rc::clone(log);
    ShellTask::new(Box::pin(async move { log.borrow_mut().push(mark) }))
}

#[test]
fn a_task_that_a_blocking_wait_polls_does_not_run_and_comes_back() {
    let log = Rc::new(RefCell::new(Vec::new()));
    let mut task = marking(&log, 1);
    let mut back = Vec::new();
    let poll = uninterrupted(|| poll_once(&mut task), |task| back.push(task));
    // For the wait the task has ended, so the wait does not stay for it.
    assert_eq!(poll, Poll::Ready(()));
    assert!(log.borrow().is_empty());
    assert_eq!(back.len(), 1);
    assert_eq!(poll_once(&mut back[0]), Poll::Ready(()));
    assert_eq!(*log.borrow(), [1]);
}

#[test]
fn tasks_come_back_in_the_order_in_which_the_wait_polled_them() {
    let log = Rc::new(RefCell::new(Vec::new()));
    let mut tasks = [marking(&log, 1), marking(&log, 2), marking(&log, 3)];
    let mut back = Vec::new();
    uninterrupted(
        || {
            for task in &mut tasks {
                assert_eq!(poll_once(task), Poll::Ready(()));
            }
        },
        |task| back.push(task),
    );
    assert!(log.borrow().is_empty());
    for task in &mut back {
        assert_eq!(poll_once(task), Poll::Ready(()));
    }
    assert_eq!(*log.borrow(), [1, 2, 3]);
}

#[test]
fn a_task_runs_as_usual_when_no_blocking_wait_runs() {
    let log = Rc::new(RefCell::new(Vec::new()));
    let mut task = marking(&log, 1);
    assert_eq!(poll_once(&mut task), Poll::Ready(()));
    assert_eq!(*log.borrow(), [1]);
    // A task that waits stays pending, and it is polled again after the wait of another command.
    let mut steps = 0;
    let mut waiting = ShellTask::new(Box::pin(std::future::poll_fn(move |_| {
        steps += 1;
        if steps < 2 {
            Poll::Pending
        } else {
            Poll::Ready(())
        }
    })));
    assert_eq!(poll_once(&mut waiting), Poll::Pending);
    let mut back: Vec<ShellTask> = Vec::new();
    uninterrupted(|| (), |task| back.push(task));
    assert!(back.is_empty());
    assert_eq!(poll_once(&mut waiting), Poll::Ready(()));
}
