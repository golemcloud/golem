use rustyline::history::{History, SearchDirection};

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
