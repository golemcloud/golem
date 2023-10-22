use std::fmt::Display;

pub struct WitEmpty {}

pub struct WitPerson {
    pub name2: String,
    pub age: i32,
}

pub enum WitBidResult {
    Failure2(String, u32),
    Someone2 { name: String, age: u32 },
    Success2,
}

pub enum WitColors {
    Red,
    White,

    Yellow2,
}

impl Display for WitColors {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WitColors::Red => write!(f, "Read"),
            WitColors::White => write!(f, "Write"),
            WitColors::Yellow2 => write!(f, "Yellow 2"),
        }
    }
}
