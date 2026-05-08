use golem_rust::agent_definition;
use std::marker::PhantomData;

#[agent_definition]
trait GenericAgent {
    fn new(id: String) -> Self;
    fn len(&self) -> usize;
}

struct GenericAgentImpl<'a, T, const N: usize> {
    id: String,
    _marker: PhantomData<&'a [T; N]>,
}

impl<'a, T, const N: usize> GenericAgent for GenericAgentImpl<'a, T, N>
where
    T: Clone,
{
    fn new(id: String) -> Self {
        Self {
            id,
            _marker: PhantomData,
        }
    }

    fn len(&self) -> usize {
        self.id.len() + N
    }
}

fn main() {}
