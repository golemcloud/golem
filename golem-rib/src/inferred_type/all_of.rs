// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.


use std::collections::VecDeque;
use crate::{InferredType, TypeInternal};

// This module is responsible to merge the types when constructing InferredType::AllOf, while
// selecting the type with maximum `TypeOrigin` information. This gives two advantages. We save some memory footprint
// (Ex: `{foo : string}` and `{foo: all_of(string, u8)}` will be merged to `{foo: all_of(string, u8)}`.)
// Secondly, this phase will choose to deduplicate the types based on maximum
// `TypeOrigin` allowing descriptive compilation error messages at the end.

// But this is done only if the types match exact. It doesn't do `unification` (its a separate phase)
// keeping things orthogonal for maintainability. This merging shouldn't be confused with `unification`.

// Example: We will not merge `{foo: string}` and `{foo: string, bar: u8}` to `{foo: all_of(string, string), bar: u8}`
// as they are different record types.

// However, we will merge `{foo: string}` and `{foo: u8}` to `{foo: (string, u8)}` or
// `{foo: string, bar: u8}` and `{foo: string, bar: string}` to `{foo: all_of(string, string), bar: all_of(u8, string)}`.
// We do not merge all_of(string, string) in the above example to `string` either.
#[derive(Clone, Debug, PartialEq)]
pub enum MergeTask {
    RecordBuilder(RecordBuilder),
    Inspect(TaskIndex, InferredType),
    AllOfBuilder(TaskIndex, Vec<TaskIndex>),
    Complete(TaskIndex, InferredType),
}

pub struct Inspect(InferredType);
pub type TaskIndex = usize;

#[derive(Default, Clone, Debug, PartialEq)]
pub struct RecordBuilder {
    task_index: TaskIndex, // The index in the task stack to which this builder belongs
    field_and_tasks: Vec<(String, Vec<TaskIndex>)>,
}

impl RecordBuilder {
    pub fn field_names(&self) -> Vec<&String> {
        self.field_and_tasks.iter().map(|(name, _)| name).collect()
    }

    pub fn new(index: TaskIndex, fields: Vec<(String, Vec<TaskIndex>)>) -> RecordBuilder {
        RecordBuilder {
            task_index: index,
            field_and_tasks: fields
        }
    }

    pub fn insert(&mut self, field_name: String, task_index: TaskIndex) {
        let mut found = false;
        self.field_and_tasks.iter_mut()
            .find(|(name, _)| name == &field_name)
            .map(|(_, task_indices)| {
                found = true;
                task_indices.push(task_index)
            });

        if !found {
            self.field_and_tasks.push((field_name, vec![task_index]));
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MergeTaskStack {
    tasks: Vec<MergeTask>,
}

impl MergeTaskStack {

    pub fn extend(&mut self, other: MergeTaskStack) {
        self.tasks.extend(other.tasks);
    }

    pub fn update_record_builder(&mut self, record_builder: RecordBuilder) {
        // does it exist before
        let index = record_builder.task_index;

        if let Some(_) = self.tasks.get(index) {
            self.tasks[index] = MergeTask::RecordBuilder(record_builder);
        } else {
            self.tasks.push(MergeTask::RecordBuilder(record_builder));
        }

    }
    pub fn update(&mut self, index: &TaskIndex, task: MergeTask) {
        if index < &self.tasks.len() {
            self.tasks[*index] = task;
        } else {
            self.tasks.push(task);
        }
    }

    pub fn next_index(&self) -> TaskIndex {
        self.tasks.len()
    }

    pub fn new() -> MergeTaskStack {
        MergeTaskStack { tasks: vec![] }
    }

    pub fn init(stack: Vec<MergeTask>) -> MergeTaskStack {
        MergeTaskStack { tasks: stack }
    }

    pub fn get_record(&self, record_fields: Vec<&String>) -> Option<RecordBuilder> {
        for task in self.tasks.iter().rev(){
            match task {
                MergeTask::RecordBuilder(builder) if builder.field_names() == record_fields =>
                    return Some(builder.clone()),

                _ => {}
            }
        }

        None
    }
}

fn get_merge_task(inferred_types: Vec<InferredType>) -> MergeTaskStack {
    let mut temp_task_queue = VecDeque::new();

    let merge_tasks =
        inferred_types.iter()
            .enumerate().map(|(i, inf)| MergeTask::Inspect(i, inf.clone())).collect::<Vec<_>>();

    temp_task_queue.extend(merge_tasks.clone());

    let mut task_stack: MergeTaskStack = MergeTaskStack::new();

    while let Some(task) = temp_task_queue.pop_front() {
        match task {
            MergeTask::Inspect(task_index, inferred_type) => {
                match inferred_type.internal_type() {
                    TypeInternal::Record(fields) => {
                        let mut new_record_builder = false;

                        let mut next_available_index = task_stack.next_index();

                        let record_identifier: Vec<&String> = fields.iter()
                            .map(|(field, _)| field)
                            .collect::<Vec<_>>();

                        let builder = task_stack.get_record(record_identifier).unwrap_or_else(||{
                            new_record_builder = true;
                            RecordBuilder::new(next_available_index, vec![])
                        });

                        let mut field_task_index = if new_record_builder {
                            next_available_index
                        } else {
                            next_available_index - 1
                        };

                        let mut new_builder = builder.clone();
                        let mut tasks = vec![];

                        for (field, inferred_type) in fields.iter() {
                            field_task_index += 1;

                            new_builder.insert(field.clone(), field_task_index);

                            tasks.push(
                                MergeTask::Inspect(field_task_index, inferred_type.clone())
                            );

                            temp_task_queue.push_back(MergeTask::Inspect(field_task_index, inferred_type.clone()));

                        }

                        task_stack.update_record_builder(new_builder);

                        let new_field_task_stack = MergeTaskStack::init(tasks);

                        task_stack.extend(new_field_task_stack);
                        dbg!(task_stack.clone());
                    }

                    // When it finds primitives it stop pushing more tasks into the ephemeral queue
                    // and only updates to the persistent task stack.
                    TypeInternal::Bool
                    |TypeInternal::S8
                    |TypeInternal::U8
                    |TypeInternal::S16
                    |TypeInternal::U16
                    |TypeInternal::S32
                    |TypeInternal::U32
                    |TypeInternal::S64
                    |TypeInternal::U64
                    |TypeInternal::F32
                    |TypeInternal::F64
                    |TypeInternal::Chr
                    |TypeInternal::Str => {
                        task_stack.update(&task_index, MergeTask::Complete(task_index.clone(), inferred_type.clone()))
                    },
                    _ => {}
                }
            }

            MergeTask::RecordBuilder(_) => {}
            MergeTask::AllOfBuilder(_, _) => {}
            MergeTask::Complete(index, task) => {
                task_stack.update(&index, MergeTask::Complete(index.clone(), task.clone()));
            }
        }
    }

    task_stack
}


pub fn flatten_all_of_list(types: &Vec<InferredType>) -> Vec<InferredType> {
    let mut all_of_types = vec![];

    for typ in types {
        match typ.internal_type() {
            TypeInternal::AllOf(all_of) => {
                let flattened = flatten_all_of_list(all_of);
                for t in flattened {
                    all_of_types.push(t);
                }
            }
            _ => {
                all_of_types.push(typ.clone());
            }
        }
    }

    all_of_types
}


mod tests {
    use test_r::test;

    use super::*;

    #[test]
    fn test_get_task_stack() {
        let inferred_types = vec![InferredType::record(
            vec![
                ("foo".to_string(), InferredType::s8()),
                ("bar".to_string(), InferredType::u8())
            ],
        ), InferredType::record(
            vec![
                ("foo".to_string(), InferredType::string())
            ],

        )];

        let result = get_merge_task(inferred_types);

        let expected = MergeTaskStack {
            tasks: vec![
                MergeTask::RecordBuilder(
                    RecordBuilder {
                        task_index: 0,
                        field_and_tasks: vec![
                            (
                                "foo".to_string(),
                                vec![
                                    1,
                                ],
                            ),
                            (
                                "bar".to_string(),
                                vec![
                                    2,
                                ],
                            ),
                        ],
                    },
                ),
                MergeTask::Complete(
                    1,
                    InferredType::s8(),
                ),
                MergeTask::Complete(
                    2,
                    InferredType::u8()
                ),
                MergeTask::RecordBuilder(
                    RecordBuilder {
                        task_index: 3,
                        field_and_tasks: vec![
                            (
                                "foo".to_string(),
                                vec![
                                    4,
                                ],
                            ),
                        ],
                    },
                ),
                MergeTask::Complete(
                    4,
                    InferredType::string(),
                ),
            ],
        };

        assert_eq!(result, expected);
    }

}
