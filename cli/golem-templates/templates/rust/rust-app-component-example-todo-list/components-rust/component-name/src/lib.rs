#[allow(static_mut_refs)]
mod bindings;

use crate::bindings::exports::pa_ck::na_me_exports::component_name_api::Guest;
use crate::bindings::exports::pa_ck::na_me_exports::component_name_api::Item;
use crate::bindings::exports::pa_ck::na_me_exports::component_name_api::NewItem;
use crate::bindings::exports::pa_ck::na_me_exports::component_name_api::Priority;
use crate::bindings::exports::pa_ck::na_me_exports::component_name_api::Query;
use crate::bindings::exports::pa_ck::na_me_exports::component_name_api::QuerySort;
use crate::bindings::exports::pa_ck::na_me_exports::component_name_api::Status;
use crate::bindings::exports::pa_ck::na_me_exports::component_name_api::UpdateItem;

use chrono::{DateTime, Utc};
use once_cell::sync::Lazy;
use std::cell::RefCell;
use std::{cmp, collections::HashMap, num::TryFromIntError};
use uuid::Uuid;

struct Component;

/**
 * This is one of any number of data types that our application
 * uses. Golem will take care to persist all application state,
 * whether that state is local to a function being executed or
 * global across the entire program.
 */
struct State {
    items: HashMap<String, Item>,
}

const DATE_TIME_FORMAT: &str = "%Y-%m-%d %H:%M:%S %z";

const QUERY_MAX_LIMIT: u32 = 100;
const QUERY_DEFAULT_LIMIT: u32 = 10;

thread_local! {
    static STATE: RefCell<Lazy<State>> = RefCell::new(
        Lazy::new(|| State {
            items: HashMap::new(),
    })
)}

fn unix_time_from_option_string(s: Option<String>) -> Result<Option<i64>, String> {
    s.map(|s| {
        let unix_time = DateTime::parse_from_str(&s, DATE_TIME_FORMAT)
            .map_err(|e| {
                let error_message = format!(
                    "ERROR: '{s}' is NOT in the required format of '{}': {:?}.",
                    DATE_TIME_FORMAT,
                    e.to_string()
                );
                println!("{error_message}");

                error_message
            })?
            .timestamp();

        Ok(unix_time) as Result<i64, String>
    })
    .transpose()
}

#[inline]
fn print_count(state: &State) {
    println!(
        "You have {} items left in your todo list.",
        state.items.len()
    );
}

trait ExtensionsForUpdateItem {
    fn change_is_present(&self) -> bool;
}
impl ExtensionsForUpdateItem for UpdateItem {
    fn change_is_present(&self) -> bool {
        self.title.is_some()
            || self.priority.is_some()
            || self.status.is_some()
            || self.deadline.is_some()
    }
}
trait Ordinal {
    fn ordinal(&self) -> u8;
}
impl Ordinal for Priority {
    fn ordinal(&self) -> u8 {
        match self {
            Priority::Low => 0,
            Priority::Medium => 1,
            Priority::High => 2,
        }
    }
}

impl Guest for Component {
    fn add(item: NewItem) -> Result<Item, String> {
        let title = item.title.trim();

        if title.is_empty() {
            return Err("Title cannot be empty".to_string());
        }

        let deadline = unix_time_from_option_string(item.deadline)?;

        let id = Uuid::new_v4().to_string();

        let now = Utc::now().timestamp();

        let item = Item {
            id,
            title: title.to_string(),
            priority: item.priority,
            deadline,
            status: Status::Backlog,
            created_timestamp: now,
            updated_timestamp: now,
        };

        println!("New item created: {:?}", item);

        let result = item.clone();

        STATE.with_borrow_mut(|state| {
            state.items.insert(item.id.clone(), item);
        });

        Ok(result)
    }

    fn update(id: String, change: UpdateItem) -> Result<Item, String> {
        if change.change_is_present() {
            let deadline_update = unix_time_from_option_string(change.deadline)?;

            STATE.with_borrow_mut(|state| {
                if let Some(item) = state.items.get_mut(&id) {
                    let mut modified = false;

                    if let Some(title_update) = change.title {
                        let title_update = title_update.trim();

                        if !{ title_update.is_empty() } && item.title != title_update {
                            item.title = title_update.to_string();
                            modified = true;
                        }
                    }
                    if let Some(priority_update) = change.priority {
                        if item.priority != priority_update {
                            item.priority = priority_update;
                            modified = true;
                        }
                    }
                    if let Some(status_update) = change.status {
                        if item.status != status_update {
                            item.status = status_update;
                            modified = true;
                        }
                    }
                    if item.deadline != deadline_update {
                        item.deadline = deadline_update;
                        modified = true;
                    }

                    if modified {
                        item.updated_timestamp = Utc::now().timestamp();
                        println!("Updated item with ID '{}'.", id);
                    } else {
                        println!("No update applied to item with ID '{}'.", id);
                    }

                    Ok(item.clone())
                } else {
                    let error_message = format!("Item with ID '{}' not found!", id);
                    println!("{error_message}");

                    Err(error_message)
                }
            })
        } else {
            Err("At least one change must be present.".to_string())
        }
    }

    fn search(query: Query) -> Result<Vec<Item>, String> {
        let deadline = unix_time_from_option_string(query.deadline)?;

        let limit: usize = query
            .limit
            .map(|n| {
                if n > QUERY_MAX_LIMIT {
                    QUERY_MAX_LIMIT
                } else {
                    n
                }
            })
            .unwrap_or(QUERY_DEFAULT_LIMIT)
            .try_into()
            .map_err(|e: TryFromIntError| e.to_string())?;

        STATE.with_borrow_mut(|state| {
            let mut result: Vec<_> = state
                .items
                .values()
                .filter(|item| {
                    query
                        .keyword
                        .as_ref()
                        .map(|keyword| item.title.contains(keyword))
                        .unwrap_or(true)
                        && query
                            .priority
                            .map(|priority| item.priority == priority)
                            .unwrap_or(true)
                        && query
                            .status
                            .map(|status| item.status == status)
                            .unwrap_or(true)
                        && deadline
                            .map(|deadline| {
                                if let Some(before) = item.deadline {
                                    before <= deadline
                                } else {
                                    true
                                }
                            })
                            .unwrap_or(true)
                })
                .cloned()
                .collect();

            match query.sort {
                Some(QuerySort::Priority) => {
                    result.sort_by_key(|item| cmp::Reverse(item.priority.ordinal()));
                }
                Some(QuerySort::Deadline) => {
                    result.sort_by_key(|item| cmp::Reverse(item.deadline));
                }
                None => {
                    result.sort_by_key(|item| item.title.clone());
                }
            };

            result = result.into_iter().take(limit).collect();

            if result.is_empty() {
                println!("No matching todo found.");
            } else {
                print!("Found {} matching items: ", result.len());

                result.iter().for_each(|item| {
                    let deadline = item
                        .deadline
                        .and_then(|i: i64| DateTime::from_timestamp(i, 0))
                        .map(|datetime| datetime.to_string())
                        .unwrap_or("<No deadline set>".to_string());

                    println!("{:?} {}", item, deadline);
                });
            }
            Ok(result)
        })
    }

    fn count() -> u32 {
        STATE.with_borrow_mut(|state| {
            let count = state.items.len() as u32;

            println!("You have {} items in your todo list.", count);

            count
        })
    }

    fn delete(id: String) -> Result<(), String> {
        STATE.with_borrow_mut(|state| {
            if state.items.contains_key(&id) {
                state.items.remove(&id);

                println!("Deleted item with ID '{}'.", id);
                print_count(state);

                Ok(())
            } else {
                let error_message = format!("Item with ID '{}' not found!", id);
                println!("{error_message}");

                Err(error_message)
            }
        })
    }

    fn delete_done_items() {
        STATE.with_borrow_mut(|state| {
            let mut count = 0_u32;

            state.items.retain(|_, item| {
                if item.status == Status::Done {
                    count += 1;
                    false
                } else {
                    true
                }
            });

            println!("Deleted {} Done items.", count);
            print_count(state);
        });
    }

    fn delete_all() {
        STATE.with_borrow_mut(|state| {
            state.items.clear();

            println!("Deleted all items.");
        });
    }

    fn get(id: String) -> Result<Item, String> {
        STATE.with_borrow_mut(|state| {
            if let Some(item) = state.items.get(&id) {
                println!("Found item with ID '{}'.", id);

                Ok(item.clone())
            } else {
                Err(format!("Item with ID '{}' not found!", id))
            }
        })
    }
}

bindings::export!(Component with_types_in bindings);
