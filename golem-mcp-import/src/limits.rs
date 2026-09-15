//! Bound JSON traversal and serialization without allocating a second document.

use serde::Serialize;
use serde_json::Value;
use std::io;

#[derive(Debug, PartialEq)]
pub(crate) enum Exceeded {
    Bytes,
    Nodes,
    Depth,
}

pub(crate) fn check(
    value: &Value,
    bytes: usize,
    nodes: usize,
    depth: usize,
) -> Result<(), Exceeded> {
    walk(value, depth, &mut { nodes })?;
    check_bytes(value, bytes)
}

pub(crate) fn walk(value: &Value, depth: usize, nodes: &mut usize) -> Result<(), Exceeded> {
    *nodes = nodes.checked_sub(1).ok_or(Exceeded::Nodes)?;
    let mut child =
        |value: &Value| walk(value, depth.checked_sub(1).ok_or(Exceeded::Depth)?, nodes);
    match value {
        Value::Array(values) => values.iter().try_for_each(&mut child),
        Value::Object(values) => values.values().try_for_each(&mut child),
        _ => Ok(()),
    }
}

struct Budget(usize);

impl Budget {
    fn charge(&mut self, bytes: usize) -> Result<(), Exceeded> {
        self.0 = self.0.checked_sub(bytes).ok_or(Exceeded::Bytes)?;
        Ok(())
    }
}

impl io::Write for Budget {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.charge(bytes.len())
            .map_err(|_| io::Error::other("JSON byte limit exceeded"))?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub(crate) fn check_bytes(value: &impl Serialize, bytes: usize) -> Result<(), Exceeded> {
    serde_json::to_writer(Budget(bytes), value).map_err(|_| Exceeded::Bytes)
}

/// Count the complete listing, including definitions too deep to serialize recursively.
pub(crate) fn check_array_bytes(values: &[Value], bytes: usize) -> Result<(), Exceeded> {
    let mut budget = Budget(bytes);
    budget.charge(2 + values.len().saturating_sub(1))?;
    let mut pending: Vec<_> = values.iter().collect();
    while let Some(value) = pending.pop() {
        match value {
            Value::Array(values) => {
                budget.charge(2 + values.len().saturating_sub(1))?;
                pending.extend(values);
            }
            Value::Object(values) => {
                budget.charge(2 + values.len() + values.len().saturating_sub(1))?;
                for (key, value) in values {
                    serde_json::to_writer(&mut budget, key).map_err(|_| Exceeded::Bytes)?;
                    pending.push(value);
                }
            }
            _ => serde_json::to_writer(&mut budget, value).map_err(|_| Exceeded::Bytes)?,
        }
    }
    Ok(())
}
