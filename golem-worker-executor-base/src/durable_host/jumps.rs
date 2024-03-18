use golem_common::model::{Jump, OplogEntry};
use std::fmt::{Display, Formatter};

/// Structure holding all the active jumps for the current execution of a worker
pub struct ActiveJumps {
    jumps: Vec<Jump>,
}

impl ActiveJumps {
    /// Constructs an empty set of active jumps
    pub fn new() -> Self {
        Self { jumps: Vec::new() }
    }

    /// Initializes active jumps from the an oplog entry. If it was a jump entry use information
    /// from that, otherwise it returns empty active jumps
    pub fn from_oplog_entry(entry: &OplogEntry) -> Self {
        match entry {
            OplogEntry::Jump { jumps, .. } => Self {
                jumps: jumps.clone(),
            },
            _ => Self::new(),
        }
    }

    /// Returns the list of active jumps
    pub fn jumps(&self) -> &Vec<Jump> {
        &self.jumps
    }

    /// Adds a new jump definition to the list of active jumps
    pub fn add_jump(&mut self, jump: Jump) {
        self.jumps.push(jump);
    }

    /// Checks whether there is an active jump to perform when reaching the given oplog index.
    /// If there is, returns the oplog index where the execution should continue, otherwise returns None.
    pub fn has_active_jump(&self, at: u64) -> Option<u64> {
        // TODO: optimize this by introducing a map during construction
        for jump in &self.jumps {
            if jump.target_oplog_idx == at {
                return Some(jump.source_oplog_idx);
            }
        }
        None
    }
}

impl Display for ActiveJumps {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "[{}]",
            self.jumps
                .iter()
                .map(|jump| format!("<{} => {}>", jump.source_oplog_idx, jump.target_oplog_idx))
                .collect::<Vec<String>>()
                .join(", ")
        )
    }
}
