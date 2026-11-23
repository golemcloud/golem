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

#[async_trait]
trait Traced {
    async fn positional(&self) -> usize;
    async fn trailing(&self) -> usize;
    async fn named(&self) -> usize;
    async fn default_name(&self) -> usize;
}

struct TracedSubject;
#[async_trait]
#[region_simple]
impl Traced for TracedSubject {
    #[tracing::instrument("positional span", skip_all)]
    async fn positional(&self) -> usize {
        11
    }
    #[tracing::instrument(skip_all, "trailing span")]
    async fn trailing(&self) -> usize {
        23
    }
    #[tracing::instrument(name = "named span", skip_all)]
    async fn named(&self) -> usize {
        37
    }
    #[cfg_attr(all(), allow(dead_code))]
    #[tracing::instrument(target = "other target", skip_all, fields(name = "field value"))]
    async fn default_name(&self) -> usize {
        41
    }
}

#[test]
fn instrument_names_and_lazy_spans() {
    let events: Events = Arc::new(Mutex::new(Vec::new()));
    let _subscriber = tracing::subscriber::set_default(Subscriber(events.clone()));
    let subject = TracedSubject;
    let cases = [
        (subject.positional(), "positional span", 11),
        (subject.trailing(), "trailing span", 23),
        (subject.named(), "named span", 37),
        (subject.default_name(), "default_name", 41),
    ];
    assert!(
        events.lock().unwrap().is_empty(),
        "construction must not create spans"
    );
    for (mut future, name, expected) in cases {
        assert_eq!(poll(future.as_mut()), Poll::Ready(expected));
        drop(future);
        let mut events = events.lock().unwrap();
        assert_eq!(
            events.as_slice(),
            &[
                ("new", name),
                ("enter", "run"),
                ("exit", "run"),
                ("enter", "run"),
                ("exit", "run"),
            ]
        );
        events.clear();
    }
}

mod allocation {
    use super::*;
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::cell::Cell;
    use test_r::test;

    thread_local! {
        static ALLOCATIONS: Cell<Option<usize>> = const { Cell::new(None) };
    }

    struct CountingAllocator;
    #[global_allocator]
    static ALLOCATOR: CountingAllocator = CountingAllocator;

    fn record_allocation() {
        // Other test threads and allocations outside the measurement are ignored.
        let _ = ALLOCATIONS.try_with(|count| {
            if let Some(value) = count.get() {
                count.set(Some(value + 1));
            }
        });
    }

    unsafe impl GlobalAlloc for CountingAllocator {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            record_allocation();
            unsafe { System.alloc(layout) }
        }
        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            record_allocation();
            unsafe { System.alloc_zeroed(layout) }
        }
        unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
            record_allocation();
            unsafe { System.realloc(ptr, layout, size) }
        }
        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            unsafe { System.dealloc(ptr, layout) }
        }
    }

    fn measured<T>(operation: impl FnOnce() -> T) -> (T, usize) {
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                ALLOCATIONS.set(None);
            }
        }
        ALLOCATIONS.set(Some(0));
        let _reset = Reset;
        let result = operation();
        (result, ALLOCATIONS.get().unwrap())
    }

    #[async_trait]
    trait Value {
        async fn value(&self, input: usize) -> usize;
    }
    struct Plain;
    struct Region;
    #[async_trait]
    impl Value for Plain {
        async fn value(&self, input: usize) -> usize {
            input * 3 + 1
        }
    }
    #[async_trait]
    #[region_simple]
    impl Value for Region {
        async fn value(&self, input: usize) -> usize {
            input * 3 + 1
        }
    }

    #[test]
    fn one_allocation_through_completion() {
        for subject in [&Plain as &dyn Value, &Region as &dyn Value] {
            let (result, allocations) = measured(|| {
                let mut future = std::hint::black_box(subject.value(7));
                let result = poll(future.as_mut());
                drop(future);
                result
            });
            assert_eq!(result, Poll::Ready(22));
            assert_eq!(allocations, 1);
        }
        let state = AtomicUsize::new(11);
        let generic = Generic(&state, 1u8);
        let (result, allocations) = measured(|| {
            let mut future = std::hint::black_box(generic.compute((), &3, &17));
            let result = poll(future.as_mut());
            drop(future);
            result
        });
        assert_eq!(result, Poll::Ready(43));
        assert_eq!(allocations, 1);
    }
}
