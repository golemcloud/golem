pub trait ArenaMember<Arena> {
    type NonRecursive;
    fn deflate(&self, arena: &mut Arena) -> Self::NonRecursive;
    fn inflate(non_rec: Self::NonRecursive, arena: &Arena) -> Self;
}

// MARKER: Box wrapper
impl<A, T: ArenaMember<A>> ArenaMember<A> for Box<T> {
    type NonRecursive = T::NonRecursive;
    fn deflate(&self, arena: &mut A) -> Self::NonRecursive {
        self.as_ref().deflate(arena)
    }
    fn inflate(val: Self::NonRecursive, arena: &A) -> Self {
        Box::new(T::inflate(val, arena))
    }
}

// MARKER: Option wrapper
impl<A, T: ArenaMember<A>> ArenaMember<A> for Option<T> {
    type NonRecursive = Option<T::NonRecursive>;
    fn deflate(&self, arena: &mut A) -> Self::NonRecursive {
        self.as_ref().map(|inner| inner.deflate(arena))
    }
    fn inflate(val: Self::NonRecursive, arena: &A) -> Self {
        val.map(|inner| T::inflate(inner, arena))
    }
}

// MARKER: Vector wrapper
impl<A, T: ArenaMember<A>> ArenaMember<A> for Vec<T> {
    type NonRecursive = Vec<T::NonRecursive>;
    fn deflate(&self, arena: &mut A) -> Self::NonRecursive {
        self.iter().map(|item| item.deflate(arena)).collect()
    }
    fn inflate(val: Self::NonRecursive, arena: &A) -> Self {
        val.into_iter().map(|item| T::inflate(item, arena)).collect()
    }
}

// MARKER: Leaf Types (Non-recursive)
macro_rules! impl_arena_leaf {
    ($($t:ty),*) => {
        $(
            impl<A> ArenaMember<A> for $t {
                type NonRecursive = $t;
                fn deflate(&self, _: &mut A) -> Self::NonRecursive { self.clone() }
                fn inflate(val: Self::NonRecursive, _: &A) -> Self { val }
            }
        )*
    };
}

impl_arena_leaf!(i32, u32, i64, u64, f32, f64, String, bool, usize);
