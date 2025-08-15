pub struct State {
mut value: UInt64
} derive(Show)

fn State::new() -> State {
    { value: 0 }
}

pub fn add(self: State, value: UInt64) -> Unit {
    self.value += value
}

pub let state: State = State::new();
