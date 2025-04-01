use rib::InferredExpr;
use rustyline::history::{DefaultHistory, History, SearchDirection, SearchResult};
use std::path::Path;

pub struct RibReplHistory {
    pub current_compiled_expr: Option<InferredExpr>,
    history: DefaultHistory,
}

impl Default for RibReplHistory {
    fn default() -> Self {
        Self::new()
    }
}

impl RibReplHistory {
    pub fn update_current_compiled_expr(&mut self, expr: InferredExpr) {
        self.current_compiled_expr = Some(expr);
    }

    pub fn new() -> Self {
        Self {
            current_compiled_expr: None,
            history: DefaultHistory::new(),
        }
    }
}

impl History for RibReplHistory {
    fn get(&self, index: usize, dir: SearchDirection) -> rustyline::Result<Option<SearchResult>> {
        self.history.get(index, dir)
    }

    fn add(&mut self, line: &str) -> rustyline::Result<bool> {
        self.history.add(line)
    }

    fn add_owned(&mut self, line: String) -> rustyline::Result<bool> {
        self.history.add_owned(line)
    }

    fn len(&self) -> usize {
        self.history.len()
    }

    fn is_empty(&self) -> bool {
        self.history.is_empty()
    }

    fn set_max_len(&mut self, len: usize) -> rustyline::Result<()> {
        self.history.set_max_len(len)
    }

    fn ignore_dups(&mut self, yes: bool) -> rustyline::Result<()> {
        self.history.ignore_dups(yes)
    }

    fn ignore_space(&mut self, yes: bool) {
        self.history.ignore_space(yes)
    }

    fn save(&mut self, path: &Path) -> rustyline::Result<()> {
        self.history.save(path)
    }

    fn append(&mut self, path: &Path) -> rustyline::Result<()> {
        self.history.append(path)
    }

    fn load(&mut self, path: &Path) -> rustyline::Result<()> {
        self.history.load(path)
    }

    fn clear(&mut self) -> rustyline::Result<()> {
        self.history.clear()
    }

    fn search(
        &self,
        term: &str,
        start: usize,
        dir: SearchDirection,
    ) -> rustyline::Result<Option<SearchResult>> {
        self.history.search(term, start, dir)
    }

    fn starts_with(
        &self,
        term: &str,
        start: usize,
        dir: SearchDirection,
    ) -> rustyline::Result<Option<SearchResult>> {
        self.history.starts_with(term, start, dir)
    }
}

pub fn retrieve_history(history: &dyn History) -> Vec<String> {
    let len = history.len();

    // Retrieve all history entries that were valid
    let mut entries = vec![];

    for i in 0..len {
        let entry = history
            .get(i, SearchDirection::Forward)
            .map(|e| e.map(|s| s.entry.to_string()))
            .unwrap();

        if let Some(entry) = entry {
            entries.push(entry);
        }
    }

    entries
}
