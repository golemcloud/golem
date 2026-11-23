use async_trait::async_trait;
use golem_async_trait::region_simple;
use std::future::Future;
use std::pin::Pin;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use std::task::{Context, Poll, Waker};
use test_r::test;

test_r::enable!();

type Events = Arc<Mutex<Vec<(&'static str, &'static str)>>>;
struct Subscriber(Events);
impl tracing::Subscriber for Subscriber {
    fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, attrs: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        self.0
            .lock()
            .unwrap()
            .push(("new", attrs.metadata().name()));
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn event(&self, _: &tracing::Event<'_>) {}
    fn enter(&self, _: &tracing::span::Id) {
        self.0.lock().unwrap().push(("enter", "run"));
    }
    fn exit(&self, _: &tracing::span::Id) {
        self.0.lock().unwrap().push(("exit", "run"));
    }
}

struct Guard(Arc<AtomicUsize>);
impl Drop for Guard {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[async_trait]
trait Operation {
    async fn run(&self, value: usize, guard: Guard) -> usize;
    async fn wait(&self, guard: Guard);
    async fn defaulted(&self, value: usize) -> usize {
        value * 7 + 3
    }
}

struct Subject(Arc<AtomicUsize>);
#[async_trait]
#[region_simple]
impl Operation for Subject {
    #[tracing::instrument(skip_all)]
    async fn run(&self, value: usize, guard: Guard) -> usize {
        self.0.fetch_add(1, Ordering::SeqCst);
        drop(guard);
        value * 3 + 1
    }
    async fn wait(&self, _guard: Guard) {
        self.0.fetch_add(1, Ordering::SeqCst);
        std::future::pending::<()>().await;
    }
}

#[async_trait]
trait Borrowed {
    type Output;
    async fn compute<T: Send + Sync>(&self, value: T, left: &usize, right: &usize) -> Self::Output;
}

struct Generic<'a, T>(&'a AtomicUsize, T);
#[async_trait]
#[region_simple]
impl<'a, U: Send + Sync> Borrowed for Generic<'a, U> {
    type Output = usize;
    async fn compute<T: Send + Sync>(
        &self,
        _value: T,
        left: &usize,
        right: &usize,
    ) -> Self::Output {
        let _ = &self.1;
        let _: Option<Self::Output> = None;
        self.0.load(Ordering::SeqCst) + left * 5 + right
    }
}

#[async_trait(?Send)]
trait Local {
    async fn local(&self) -> usize;
}
struct LocalSubject(std::rc::Rc<usize>);
#[async_trait(?Send)]
#[region_simple]
impl Local for LocalSubject {
    async fn local(&self) -> usize {
        *self.0
    }
}

struct Leaf;
struct Reader;
mod scope {
    #[async_trait::async_trait]
    pub(super) trait Route<T: Send> {
        async fn read(&self) -> T;
        async fn forward(&self, target: &super::Leaf) -> T;
    }
    pub(super) trait OtherRoute {
        fn read(&self) -> u16;
    }
}

#[async_trait]
#[region_simple]
impl scope::Route<u16> for Leaf {
    async fn read(&self) -> u16 {
        17
    }
    async fn forward(&self, target: &Leaf) -> u16 {
        target.read().await + 1
    }
}

#[async_trait]
#[region_simple]
impl scope::Route<u16> for Reader {
    async fn read(&self) -> u16 {
        23
    }
    async fn forward(&self, target: &Leaf) -> u16 {
        target.read().await + 4
    }
}

impl scope::OtherRoute for Leaf {
    fn read(&self) -> u16 {
        999
    }
}

fn send<F: Future + Send>(future: F) -> F {
    future
}
fn poll<F: Future + ?Sized>(f: Pin<&mut F>) -> Poll<F::Output> {
    f.poll(&mut Context::from_waker(Waker::noop()))
}

#[test]
fn boxed_future_contracts() {
    let events: Events = Arc::new(Mutex::new(Vec::with_capacity(128)));
    let _subscriber = tracing::subscriber::set_default(Subscriber(events.clone()));
    let calls = Arc::new(AtomicUsize::new(0));
    let drops = Arc::new(AtomicUsize::new(0));
    let subject: Box<dyn Operation + Send + Sync> = Box::new(Subject(calls.clone()));
    assert_eq!(poll(subject.defaulted(4).as_mut()), Poll::Ready(31));
    let future = send(subject.run(7, Guard(drops.clone())));
    assert_eq!(calls.load(Ordering::SeqCst), 0, "construction must be lazy");
    drop(future);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        drops.load(Ordering::SeqCst),
        1,
        "unpolled cancellation drops captures"
    );
    let mut future = send(subject.run(7, Guard(drops.clone())));
    assert_eq!(poll(future.as_mut()), Poll::Ready(22));
    drop(future);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(drops.load(Ordering::SeqCst), 2);
    let mut future = send(subject.wait(Guard(drops.clone())));
    assert_eq!(poll(future.as_mut()), Poll::Pending);
    drop(future);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        drops.load(Ordering::SeqCst),
        3,
        "pending cancellation drops captures"
    );
    let state = AtomicUsize::new(11);
    let generic = Generic(&state, 1u8);
    let mut future = send(generic.compute(String::from("owned"), &3, &17));
    assert_eq!(poll(future.as_mut()), Poll::Ready(43));
    let local: Box<dyn Local> = Box::new(LocalSubject(std::rc::Rc::new(19)));
    assert_eq!(poll(local.local().as_mut()), Poll::Ready(19));
    assert_eq!(
        poll(scope::Route::<u16>::forward(&Reader, &Leaf).as_mut()),
        Poll::Ready(21)
    );
    assert_eq!(
        poll(scope::Route::<u16>::forward(&Leaf, &Leaf).as_mut()),
        Poll::Ready(18)
    );
    {
        use scope::OtherRoute;
        assert_eq!(
            Leaf.read(),
            999,
            "generated imports must not affect caller method selection"
        );
    }
    let events = events.lock().unwrap();
    assert_eq!(
        events.as_slice(),
        &[
            ("new", "run"),
            ("enter", "run"),
            ("exit", "run"),
            ("enter", "run"),
            ("exit", "run")
        ],
        "tracing names and poll/drop order must remain unchanged"
    );
}

mod second_scope {
    #[async_trait::async_trait]
    pub(super) trait Route {
        async fn read(&self) -> u16;
    }
}

#[async_trait]
#[region_simple]
impl second_scope::Route for Leaf {
    async fn read(&self) -> u16 {
        41
    }
}

#[async_trait]
#[region_simple]
impl scope::Route<u32> for Reader {
    async fn read(&self) -> u32 {
        37
    }
    async fn forward(&self, _target: &Leaf) -> u32 {
        <Self as scope::Route<u32>>::read(self).await + 2
    }
}

#[test]
fn qualified_traits_and_generic_instantiations_do_not_collide() {
    assert_eq!(
        poll(second_scope::Route::read(&Leaf).as_mut()),
        Poll::Ready(41)
    );
    assert_eq!(
        poll(scope::Route::<u16>::read(&Leaf).as_mut()),
        Poll::Ready(17)
    );
    assert_eq!(
        poll(scope::Route::<u32>::forward(&Reader, &Leaf).as_mut()),
        Poll::Ready(39)
    );
}
