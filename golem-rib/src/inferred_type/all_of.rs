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

use crate::{InferredType, Path, TypeInternal};
use std::collections::{HashMap, HashSet, VecDeque};

use crate::inferred_type::TypeOrigin;
pub(crate) use internal::*;
pub(crate) use type_identifiers::*;

// This module is responsible to merge the types when constructing InferredType::AllOf, while
// selecting the type with maximum `TypeOrigin` information. This gives two advantages.

// We may have a better memory footprint with this phase as an added advantage
// (Ex: `{foo : string}` and `{foo: all_of(string, u8)}` will be merged to `{foo: all_of(string, u8)}`).
// More importantly, by doing such merge, this phase will/can choose to deduplicate the types based on maximum
// `TypeOrigin` allowing descriptive compilation error messages at the end. Otherwise, we will have
// types with less `TypeOrigin` information at the unification phase, forcing the compiler to fail
// with less descriptive error messages.

// Importantly, this merging is NOT unification. Merging is done only if types match exact.
// It doesn't do `unification` (it's a separate phase)
// keeping things orthogonal for maintainability. Also, such an early unification result in invariants to appear
// in final unification resulting in invalid states. So it's better not to try even if it seems like a good idea.

// Example:
// In this phase, will not merge `{foo: string}` and `{foo: string, bar: u8}` to `{foo: all_of(string, string), bar: u8}`
// as they are different record types.
// However, we will merge `{foo: string}` and `{foo: u8}` to `{foo: (string, u8)}` or
// `{foo: string, bar: u8}` and `{foo: string, bar: string}` to `{foo: all_of(string, string), bar: all_of(u8, string)}`.

// High level Implementation detail:
// MergeTaskStack is a set of build tasks to generate types where each builder simply have indices
// pointing to another builder or a completed type. Stack may also have just a set of completed inferred-type.
// `merge_task_stack.complete()` will finally do the job of converting indices to proper types
// while also deduplicating leaf nodes by selecting the one with maximum `TypeOrigin` information.
#[derive(Debug, Clone, PartialEq)]
pub struct MergeTaskStack<'a> {
    tasks: Vec<MergeTask<'a>>,
}

impl<'a> MergeTaskStack<'a> {
    pub fn complete(self) -> InferredType {
        let mut types = HashMap::new();

        let mut used_index = HashSet::new();

        let iter = self.tasks.into_iter().rev();

        for task in iter {
            match task {
                MergeTask::Complete(index, task) => {
                    types.insert(index, task.clone());
                }

                MergeTask::RecordBuilder(builder) => {
                    let mut fields = vec![];

                    for (field, task_indices) in builder.field_and_pointers {
                        let mut field_types = vec![];

                        for task_index in task_indices {
                            if let Some(typ) = types.get(&task_index) {
                                used_index.insert(task_index);
                                field_types.push(typ.clone());
                            }
                        }

                        let merged = flatten_all_of(field_types);

                        fields.push((field, merged));
                    }

                    let inferred_type =
                        InferredType::new(TypeInternal::Record(fields), TypeOrigin::NoOrigin);

                    types.insert(builder.task_index, inferred_type);
                }

                MergeTask::VariantBuilder(builder) => {
                    let mut variants = vec![];

                    for (variant_name, task_indices) in builder.variants {
                        if let Some(task_indices) = task_indices {
                            let mut variant_types = vec![];

                            for task_index in task_indices {
                                if let Some(typ) = types.get(&task_index) {
                                    used_index.insert(task_index);
                                    variant_types.push(typ.clone());
                                }
                            }

                            let merged = flatten_all_of(variant_types);

                            variants.push((variant_name, Some(merged)));
                        } else {
                            variants.push((variant_name, None));
                        }
                    }

                    let inferred_type =
                        InferredType::new(TypeInternal::Variant(variants), TypeOrigin::NoOrigin);

                    types.insert(builder.task_index, inferred_type);
                }

                MergeTask::TupleBuilder(tuple_builder) => {
                    let mut tuple = vec![];

                    for task_indices in &tuple_builder.tuple {
                        let mut tuple_types = vec![];

                        for task_index in task_indices {
                            if let Some(typ) = types.get(&task_index) {
                                used_index.insert(*task_index);
                                tuple_types.push(typ.clone());
                            }
                        }

                        let merged = flatten_all_of(tuple_types);

                        tuple.push(merged);
                    }

                    let inferred_type =
                        InferredType::new(TypeInternal::Tuple(tuple), TypeOrigin::NoOrigin);

                    types.insert(tuple_builder.task_index, inferred_type);
                }

                MergeTask::ResultBuilder(result_builder) => {
                    let mut ok: Option<InferredType> = None;
                    let mut error: Option<InferredType> = None;

                    if let Some(task_indices) = &result_builder.ok {
                        let mut ok_types = vec![];
                        for task_index in task_indices {
                            if let Some(typ) = types.get(&task_index) {
                                used_index.insert(*task_index);
                                ok_types.push(typ.clone());
                            }
                        }

                        let merged = flatten_all_of(ok_types);

                        ok = Some(merged);
                    }

                    if let Some(task_indices) = &result_builder.error {
                        let mut error_types = vec![];

                        for task_index in task_indices {
                            if let Some(typ) = types.get(&task_index) {
                                used_index.insert(*task_index);
                                error_types.push(typ.clone());
                            }
                        }

                        let merged = flatten_all_of(error_types);

                        error = Some(merged);
                    }

                    let inferred_type =
                        InferredType::new(TypeInternal::Result { ok, error }, TypeOrigin::NoOrigin);

                    types.insert(result_builder.task_index, inferred_type);
                }

                MergeTask::AllOfBuilder(all_of_builder) => {
                    let mut all_of_types = vec![];

                    for task_index in &all_of_builder.pointers {
                        if let Some(typ) = types.get(&task_index) {
                            used_index.insert(*task_index);
                            all_of_types.push(typ.clone());
                        }
                    }

                    let merged = flatten_all_of(all_of_types);

                    types.insert(all_of_builder.task_index, merged);
                }

                MergeTask::Inspect(_, _, _) => {}
            }
        }

        let mut final_types = vec![];
        for (index, typ) in types.into_iter() {
            if used_index.contains(&index) {
                continue;
            }

            final_types.push(typ);
        }

        if final_types.len() == 1 {
            final_types[0].clone()
        } else {
            InferredType::new(TypeInternal::AllOf(final_types), TypeOrigin::NoOrigin)
        }
    }

    pub fn get(&self, task_index: TaskIndex) -> Option<&MergeTask> {
        self.tasks.get(task_index)
    }

    pub fn extend(&mut self, other: MergeTaskStack<'a>) {
        self.tasks.extend(other.tasks);
    }

    pub fn update_build_task(&mut self, task: MergeTask<'a>) {
        // does it exist before
        let index = task.get_index_in_stack();

        if let Some(_) = self.tasks.get(index) {
            self.tasks[index] = task;
        } else {
            self.tasks.push(task);
        }
    }

    pub fn update(&mut self, index: &TaskIndex, task: MergeTask<'a>) {
        if index < &self.tasks.len() {
            self.tasks[*index] = task;
        } else {
            self.tasks.push(task);
        }
    }

    pub fn next_index(&self) -> TaskIndex {
        self.tasks.len()
    }

    pub fn new() -> MergeTaskStack<'a> {
        MergeTaskStack { tasks: vec![] }
    }

    pub fn init(stack: Vec<MergeTask>) -> MergeTaskStack {
        MergeTaskStack { tasks: stack }
    }

    pub fn get_tuple_mut(
        &mut self,
        tuple_identifier: &TupleIdentifier,
    ) -> Option<&mut TupleBuilder> {
        for task in self.tasks.iter_mut().rev() {
            match task {
                MergeTask::TupleBuilder(builder)
                    if builder.tuple.len() == tuple_identifier.length
                        && builder.path == tuple_identifier.path =>
                {
                    return Some(builder);
                }

                _ => {}
            }
        }

        None
    }

    pub fn get_record_mut(
        &mut self,
        record_fields: &RecordIdentifier,
    ) -> Option<&mut RecordBuilder> {
        for task in self.tasks.iter_mut().rev() {
            match task {
                MergeTask::RecordBuilder(builder)
                    if builder.field_names() == record_fields.fields
                        && builder.path == record_fields.path =>
                {
                    return Some(builder);
                }

                _ => {}
            }
        }

        None
    }

    pub fn get_variant_mut(
        &mut self,
        variant_identifier: &VariantIdentifier,
    ) -> Option<&mut VariantBuilder> {
        for task in self.tasks.iter_mut().rev() {
            match task {
                MergeTask::VariantBuilder(builder) => {
                    let builder_variants = &builder.variants;

                    if builder_variants.len() != variant_identifier.variants.len() {
                        continue;
                    } else {
                        let found = variant_identifier.variants.iter().all(
                            |(variant_name, variant_type)| {
                                builder_variants.iter().any(|(name, type_)| {
                                    name == variant_name
                                        && match variant_type {
                                            VariantType::WithArgs => type_.is_some(),
                                            VariantType::WithoutArgs => type_.is_none(),
                                        }
                                })
                            },
                        );

                        if found {
                            return Some(builder);
                        }
                    }
                }

                _ => {}
            }
        }

        None
    }

    pub fn get_result_mut(&mut self, result_key: &ResultIdentifier) -> Option<&mut ResultBuilder> {
        for task in self.tasks.iter_mut().rev() {
            match task {
                MergeTask::ResultBuilder(builder) => match (result_key.ok, result_key.error) {
                    (true, true) => {
                        if builder.ok.is_some()
                            && builder.error.is_some()
                            && builder.path == result_key.path
                        {
                            return Some(builder);
                        }
                    }
                    (true, false) => {
                        if builder.ok.is_some()
                            && builder.error.is_none()
                            && builder.path == result_key.path
                        {
                            return Some(builder);
                        }
                    }
                    (false, true) => {
                        if builder.ok.is_none()
                            && builder.error.is_some()
                            && builder.path == result_key.path
                        {
                            return Some(builder);
                        }
                    }
                    (false, false) => {
                        if builder.ok.is_none()
                            && builder.error.is_none()
                            && builder.path == result_key.path
                        {
                            return Some(builder);
                        }
                    }
                },

                _ => {}
            }
        }

        None
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum MergeTask<'a> {
    RecordBuilder(RecordBuilder),
    VariantBuilder(VariantBuilder),
    TupleBuilder(TupleBuilder),
    Inspect(Path, TaskIndex, &'a InferredType),
    AllOfBuilder(AllOfBuilder),
    ResultBuilder(ResultBuilder),
    Complete(TaskIndex, &'a InferredType),
}

impl MergeTask<'_> {
    pub fn get_index_in_stack(&self) -> TaskIndex {
        match self {
            MergeTask::Inspect(_, index, _) => *index,
            MergeTask::RecordBuilder(builder) => builder.task_index,
            MergeTask::AllOfBuilder(builder) => builder.task_index,
            MergeTask::ResultBuilder(builder) => builder.task_index,
            MergeTask::Complete(index, _) => *index,
            MergeTask::VariantBuilder(builder) => builder.task_index,
            MergeTask::TupleBuilder(builder) => builder.task_index,
        }
    }
}

pub type TaskIndex = usize;

#[derive(Clone, Debug, PartialEq)]
pub struct TupleBuilder {
    path: Path,
    task_index: TaskIndex,
    tuple: Vec<Vec<TaskIndex>>,
}

impl TupleBuilder {
    pub fn init(path: &Path, index: TaskIndex, elems: &Vec<InferredType>) -> TupleBuilder {
        let mut tuple: Vec<Vec<TaskIndex>> = vec![];

        elems.iter().for_each(|_| {
            tuple.push(vec![]);
        });

        TupleBuilder {
            path: path.clone(),
            task_index: index,
            tuple,
        }
    }

    pub fn insert(&mut self, indices: Vec<TaskIndex>) {
        self.tuple
            .iter_mut()
            .zip(indices.iter())
            .for_each(|(tuple, index)| {
                tuple.push(*index);
            });
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct VariantBuilder {
    task_index: TaskIndex,
    variants: Vec<(String, Option<Vec<TaskIndex>>)>,
}

impl VariantBuilder {
    pub fn init(
        index: TaskIndex,
        variants: &Vec<(String, Option<InferredType>)>,
    ) -> VariantBuilder {
        let mut default_values: Vec<(String, Option<Vec<TaskIndex>>)> = vec![];

        for (variant, inferred_type) in variants.iter() {
            match inferred_type {
                Some(_) => {
                    default_values.push((variant.clone(), Some(vec![])));
                }
                None => {
                    default_values.push((variant.clone(), None));
                }
            }
        }

        VariantBuilder {
            task_index: index,
            variants: default_values,
        }
    }

    pub fn insert(&mut self, variant_name: String, task_index: TaskIndex) {
        let mut found = false;
        self.variants
            .iter_mut()
            .find(|(name, _)| name == &variant_name)
            .map(|(_, task_indices)| {
                found = true;
                if let Some(task_indices) = task_indices {
                    task_indices.push(task_index);
                } else {
                    *task_indices = Some(vec![task_index]);
                }
            });

        if !found {
            self.variants.push((variant_name, Some(vec![task_index])));
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ResultBuilder {
    path: Path,
    task_index: TaskIndex,
    ok: Option<Vec<TaskIndex>>,
    error: Option<Vec<TaskIndex>>,
}

impl ResultBuilder {
    pub fn insert_ok(&mut self, task_index: TaskIndex) {
        if self.ok.is_none() {
            self.ok = Some(vec![task_index]);
        }

        if let Some(ok) = &mut self.ok {
            ok.push(task_index);
        }
    }

    pub fn insert_error(&mut self, task_index: TaskIndex) {
        if self.error.is_none() {
            self.error = Some(vec![task_index]);
        }

        if let Some(error) = &mut self.error {
            error.push(task_index);
        }
    }

    pub fn init(
        path: &Path,
        index: TaskIndex,
        ok: &Option<InferredType>,
        error: &Option<InferredType>,
    ) -> ResultBuilder {
        ResultBuilder::new(
            path.clone(),
            index,
            ok.as_ref().map(|_| vec![]),
            error.as_ref().map(|_| vec![]),
        )
    }

    pub fn new(
        path: Path,
        index: TaskIndex,
        ok: Option<Vec<TaskIndex>>,
        error: Option<Vec<TaskIndex>>,
    ) -> ResultBuilder {
        ResultBuilder {
            path,
            task_index: index,
            ok,
            error,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct AllOfBuilder {
    task_index: TaskIndex,
    pointers: Vec<TaskIndex>,
}

impl AllOfBuilder {
    pub fn new(index: TaskIndex, pointers: Vec<TaskIndex>) -> AllOfBuilder {
        AllOfBuilder {
            task_index: index,
            pointers,
        }
    }
}

#[derive(Default, Clone, Debug, PartialEq)]
pub struct RecordBuilder {
    path: Path,
    task_index: TaskIndex, // The index in the task stack to which this builder belongs
    field_and_pointers: Vec<(String, Vec<TaskIndex>)>,
}

impl RecordBuilder {
    pub fn field_names(&self) -> Vec<String> {
        self.field_and_pointers
            .iter()
            .map(|(name, _)| name.clone())
            .collect()
    }

    pub fn init(
        path: &Path,
        task_index: TaskIndex,
        fields: &Vec<(String, InferredType)>,
    ) -> RecordBuilder {
        let mut default_values: Vec<(String, Vec<TaskIndex>)> = vec![];

        for (field, _) in fields.iter() {
            default_values.push((field.clone(), vec![]));
        }

        RecordBuilder {
            path: path.clone(),
            task_index,
            field_and_pointers: default_values,
        }
    }

    pub fn insert(&mut self, field_name: String, task_index: TaskIndex) {
        let mut found = false;
        self.field_and_pointers
            .iter_mut()
            .find(|(name, _)| name == &field_name)
            .map(|(_, task_indices)| {
                found = true;
                task_indices.push(task_index)
            });

        if !found {
            self.field_and_pointers.push((field_name, vec![task_index]));
        }
    }
}

fn get_merge_task<'a>(inferred_types: &'a Vec<InferredType>) -> MergeTaskStack<'a> {
    let mut temp_task_queue = VecDeque::new();

    let merge_tasks: Vec<MergeTask<'a>> = inferred_types
        .iter()
        .enumerate()
        .map(|(i, inf)| MergeTask::Inspect(Path::default(), i, inf))
        .collect::<Vec<_>>();

    temp_task_queue.extend(merge_tasks.clone());

    let mut final_task_stack: MergeTaskStack = MergeTaskStack::new();

    while let Some(ref task) = temp_task_queue.pop_front() {
        match task {
            MergeTask::Inspect(path, task_index, inferred_type) => {
                match inferred_type.internal_type() {
                    TypeInternal::Record(fields) => {
                        let next_available_index = final_task_stack.next_index();

                        let record_identifier: RecordIdentifier =
                            RecordIdentifier::from(&path, fields);

                        let builder = final_task_stack.get_record_mut(&record_identifier);

                        let mut tasks_for_final_stack = vec![];

                        if let Some(builder) = builder {
                            update_record_builder_and_update_tasks(
                                &path,
                                next_available_index - 1,
                                builder,
                                fields,
                                &mut tasks_for_final_stack,
                                &mut temp_task_queue,
                            );
                        } else {
                            let (task_index, field_index) =
                                if final_task_stack.get(*task_index) == Some(&task) {
                                    (*task_index, next_available_index - 1)
                                } else {
                                    (next_available_index, next_available_index)
                                };

                            let mut builder = RecordBuilder::init(path, task_index, fields);

                            update_record_builder_and_update_tasks(
                                path,
                                field_index,
                                &mut builder,
                                fields,
                                &mut tasks_for_final_stack,
                                &mut temp_task_queue,
                            );

                            final_task_stack.update_build_task(MergeTask::RecordBuilder(builder));
                        }

                        let new_field_task_stack = MergeTaskStack::init(tasks_for_final_stack);
                        final_task_stack.extend(new_field_task_stack);
                    }

                    TypeInternal::Variant(variants) => {
                        let next_available_index = final_task_stack.next_index();

                        let record_identifier: VariantIdentifier =
                            VariantIdentifier::from(variants);

                        let builder = final_task_stack.get_variant_mut(&record_identifier);

                        let mut tasks_for_final_stack = vec![];

                        if let Some(builder) = builder {
                            update_variant_builder_and_update_tasks(
                                path,
                                next_available_index - 1,
                                builder,
                                variants,
                                &mut tasks_for_final_stack,
                                &mut temp_task_queue,
                            );
                        } else {
                            let (task_index, field_index) =
                                if final_task_stack.get(*task_index) == Some(&task) {
                                    (*task_index, next_available_index - 1)
                                } else {
                                    (next_available_index, next_available_index)
                                };

                            let mut builder = VariantBuilder::init(task_index, variants);

                            update_variant_builder_and_update_tasks(
                                path,
                                field_index,
                                &mut builder,
                                variants,
                                &mut tasks_for_final_stack,
                                &mut temp_task_queue,
                            );

                            final_task_stack.update_build_task(MergeTask::VariantBuilder(builder));
                        }

                        let new_field_task_stack = MergeTaskStack::init(tasks_for_final_stack);
                        final_task_stack.extend(new_field_task_stack);
                    }

                    TypeInternal::Result { ok, error } => {
                        let next_available_index = final_task_stack.next_index();

                        let result_identifier: ResultIdentifier =
                            ResultIdentifier::from(path, ok, error);

                        let builder = final_task_stack.get_result_mut(&result_identifier);

                        let mut tasks_for_final_stack = vec![];

                        if let Some(builder) = builder {
                            update_result_builder_and_update_tasks(
                                path,
                                next_available_index - 1,
                                builder,
                                ok,
                                error,
                                &mut tasks_for_final_stack,
                                &mut temp_task_queue,
                            );
                        } else {
                            let (task_index, field_index) =
                                if final_task_stack.get(*task_index) == Some(&task) {
                                    (*task_index, next_available_index - 1)
                                } else {
                                    (next_available_index, next_available_index)
                                };

                            let mut builder = ResultBuilder::init(path, task_index, ok, error);

                            update_result_builder_and_update_tasks(
                                path,
                                field_index,
                                &mut builder,
                                ok,
                                error,
                                &mut tasks_for_final_stack,
                                &mut temp_task_queue,
                            );

                            final_task_stack.update_build_task(MergeTask::ResultBuilder(builder));
                        }

                        let new_field_task_stack = MergeTaskStack::init(tasks_for_final_stack);
                        final_task_stack.extend(new_field_task_stack);
                    }

                    TypeInternal::Tuple(elems) => {
                        let next_available_index = final_task_stack.next_index();

                        let tuple_identifier: TupleIdentifier = TupleIdentifier::from(path, elems);

                        let builder = final_task_stack.get_tuple_mut(&tuple_identifier);

                        let mut tasks_for_final_stack = vec![];

                        if let Some(builder) = builder {
                            update_tuple_builder_and_update_tasks(
                                path,
                                next_available_index - 1,
                                builder,
                                elems,
                                &mut tasks_for_final_stack,
                                &mut temp_task_queue,
                            );
                        } else {
                            let (task_index, field_index) =
                                if final_task_stack.get(*task_index) == Some(&task) {
                                    (*task_index, next_available_index - 1)
                                } else {
                                    (next_available_index, next_available_index)
                                };

                            let mut builder = TupleBuilder::init(path, task_index, elems);

                            update_tuple_builder_and_update_tasks(
                                path,
                                field_index,
                                &mut builder,
                                elems,
                                &mut tasks_for_final_stack,
                                &mut temp_task_queue,
                            );

                            final_task_stack.update_build_task(MergeTask::TupleBuilder(builder));
                        }

                        let new_field_task_stack = MergeTaskStack::init(tasks_for_final_stack);
                        final_task_stack.extend(new_field_task_stack);
                    }

                    TypeInternal::AllOf(inferred_types) => {
                        // was this part of an inspection task? if yes then we in-place update
                        // the inspection task with all_of_builder
                        let existing_or_new = final_task_stack.get(*task_index);

                        let all_of_builder_index = match existing_or_new {
                            Some(_) => *task_index,
                            None => final_task_stack.next_index(),
                        };

                        let mut task_index = match existing_or_new {
                            // already exists
                            Some(_) => final_task_stack.next_index() - 1,
                            None => final_task_stack.next_index(),
                        };

                        let mut pointers = vec![];
                        let mut tasks_for_final_stack = vec![];

                        for inf in inferred_types.iter() {
                            task_index += 1;
                            pointers.push(task_index);
                            tasks_for_final_stack.push(MergeTask::Inspect(
                                path.clone(),
                                task_index,
                                inf,
                            ));

                            // We push the inspection task
                            temp_task_queue.push_back(MergeTask::Inspect(
                                path.clone(),
                                task_index,
                                inf,
                            ));
                        }

                        let new_all_of_builder = AllOfBuilder::new(all_of_builder_index, pointers);

                        final_task_stack.update(
                            &all_of_builder_index,
                            MergeTask::AllOfBuilder(new_all_of_builder),
                        );

                        final_task_stack.extend(MergeTaskStack::init(tasks_for_final_stack));
                    }

                    // When it finds primitives it stop pushing more tasks into the ephemeral queue
                    // and only updates to the persistent task stack.
                    TypeInternal::Flags(_)
                    | TypeInternal::Enum(_)
                    | TypeInternal::Bool
                    | TypeInternal::S8
                    | TypeInternal::U8
                    | TypeInternal::S16
                    | TypeInternal::U16
                    | TypeInternal::S32
                    | TypeInternal::U32
                    | TypeInternal::S64
                    | TypeInternal::U64
                    | TypeInternal::F32
                    | TypeInternal::F64
                    | TypeInternal::Chr
                    | TypeInternal::Resource { .. }
                    | TypeInternal::Str => final_task_stack
                        .update(&task_index, MergeTask::Complete(*task_index, inferred_type)),
                    _ => {}
                }
            }

            MergeTask::TupleBuilder(_) => {}
            MergeTask::VariantBuilder(_) => {}
            MergeTask::ResultBuilder(_) => {}
            MergeTask::RecordBuilder(_) => {}
            MergeTask::AllOfBuilder(_) => {}
            MergeTask::Complete(index, task) => {
                final_task_stack.update(&index, MergeTask::Complete(*index, task));
            }
        }
    }

    final_task_stack
}

mod type_identifiers {
    use crate::{InferredType, Path};

    pub struct TupleIdentifier {
        pub path: Path,
        pub length: usize,
    }

    impl TupleIdentifier {
        pub fn from(path: &Path, tuple: &Vec<InferredType>) -> TupleIdentifier {
            TupleIdentifier {
                path: path.clone(),
                length: tuple.len(),
            }
        }
    }

    #[derive(Debug)]
    pub struct RecordIdentifier {
        pub path: Path,
        pub fields: Vec<String>,
    }

    impl RecordIdentifier {
        pub fn from(path: &Path, fields: &Vec<(String, InferredType)>) -> RecordIdentifier {
            let mut keys = vec![];

            for (field, _) in fields.iter() {
                keys.push(field.clone());
            }

            RecordIdentifier {
                path: path.clone(),
                fields: keys,
            }
        }
    }

    pub struct ResultIdentifier {
        pub path: Path,
        pub ok: bool,
        pub error: bool,
    }

    impl ResultIdentifier {
        pub fn from(
            path: &Path,
            ok: &Option<InferredType>,
            error: &Option<InferredType>,
        ) -> ResultIdentifier {
            match (ok, error) {
                (Some(_), Some(_)) => ResultIdentifier {
                    path: path.clone(),
                    ok: true,
                    error: true,
                },
                (Some(_), None) => ResultIdentifier {
                    path: path.clone(),
                    ok: true,
                    error: false,
                },
                (None, Some(_)) => ResultIdentifier {
                    path: path.clone(),
                    ok: false,
                    error: true,
                },
                (None, None) => ResultIdentifier {
                    path: path.clone(),
                    ok: false,
                    error: false,
                },
            }
        }
    }

    pub struct VariantIdentifier {
        pub variants: Vec<(String, VariantType)>,
    }
    impl VariantIdentifier {
        pub fn from(variants: &Vec<(String, Option<InferredType>)>) -> VariantIdentifier {
            let mut keys = vec![];

            for (variant, inferred_type) in variants.iter() {
                match inferred_type {
                    Some(_) => keys.push((variant.clone(), VariantType::WithArgs)),
                    None => keys.push((variant.clone(), VariantType::WithoutArgs)),
                }
            }

            VariantIdentifier { variants: keys }
        }
    }

    pub enum VariantType {
        WithArgs,
        WithoutArgs,
    }
}

pub fn flatten_all_of(types: Vec<InferredType>) -> InferredType {
    let mut result_map: HashMap<InferredType, InferredType> = HashMap::new();
    let mut queue: VecDeque<InferredType> = VecDeque::from(types);

    while let Some(typ) = queue.pop_front() {
        match typ.internal_type() {
            TypeInternal::AllOf(nested) => {
                queue.extend(nested.clone());
            }
            _ => {
                let key = typ.clone();
                let current_origins = typ.total_origins();

                result_map
                    .entry(key)
                    .and_modify(|existing| {
                        if current_origins > existing.total_origins() {
                            *existing = typ.clone();
                        }
                    })
                    .or_insert(typ);
            }
        }
    }

    let result = result_map.into_values().collect::<Vec<_>>();

    if result.len() == 1 {
        result[0].clone()
    } else {
        InferredType::new(TypeInternal::AllOf(result), TypeOrigin::NoOrigin)
    }
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

mod internal {
    use crate::inferred_type::{
        MergeTask, RecordBuilder, ResultBuilder, TaskIndex, TupleBuilder, VariantBuilder,
    };
    use crate::{InferredType, Path, PathElem};
    use std::collections::VecDeque;

    pub fn update_record_builder_and_update_tasks<'a>(
        current_path: &Path,
        field_task_index: TaskIndex,
        builder: &mut RecordBuilder,
        fields: &'a Vec<(String, InferredType)>,
        tasks_for_final_stack: &mut Vec<MergeTask<'a>>,
        temp_task_queue: &mut VecDeque<MergeTask<'a>>,
    ) {
        let mut field_task_index = field_task_index;

        for (field, inferred_type) in fields.into_iter() {
            field_task_index += 1;

            builder.insert(field.to_string(), field_task_index);

            let mut current_path = current_path.clone();
            current_path.push_back(PathElem::Field(field.to_string()));

            tasks_for_final_stack.push(MergeTask::Inspect(
                current_path.clone(),
                field_task_index,
                inferred_type,
            ));

            temp_task_queue.push_back(MergeTask::Inspect(
                current_path,
                field_task_index,
                inferred_type,
            ));
        }
    }

    pub fn update_variant_builder_and_update_tasks<'a>(
        path: &Path,
        field_task_index: TaskIndex,
        builder: &mut VariantBuilder,
        variants: &'a Vec<(String, Option<InferredType>)>,
        tasks_for_final_stack: &mut Vec<MergeTask<'a>>,
        temp_task_queue: &mut VecDeque<MergeTask<'a>>,
    ) {
        let mut field_task_index = field_task_index;

        for (variant_name, inferred_type) in variants.into_iter() {
            if let Some(inferred_type) = inferred_type {
                field_task_index += 1;

                let path = path.clone();

                builder.insert(variant_name.to_string(), field_task_index);

                tasks_for_final_stack.push(MergeTask::Inspect(
                    path.clone(),
                    field_task_index,
                    inferred_type,
                ));

                temp_task_queue.push_back(MergeTask::Inspect(
                    path,
                    field_task_index,
                    inferred_type,
                ));
            }
        }
    }

    pub fn update_result_builder_and_update_tasks<'a>(
        path: &Path,
        field_task_index: TaskIndex,
        builder: &mut ResultBuilder,
        ok: &'a Option<InferredType>,
        error: &'a Option<InferredType>,
        tasks_for_final_stack: &mut Vec<MergeTask<'a>>,
        temp_task_queue: &mut VecDeque<MergeTask<'a>>,
    ) {
        let mut field_task_index = field_task_index;

        if let Some(inferred_type) = ok {
            field_task_index += 1;

            builder.insert_ok(field_task_index);

            let mut path = path.clone();

            path.push_back(PathElem::Field("result::ok".to_string()));

            tasks_for_final_stack.push(MergeTask::Inspect(
                path.clone(),
                field_task_index,
                inferred_type,
            ));

            temp_task_queue.push_back(MergeTask::Inspect(
                path.clone(),
                field_task_index,
                inferred_type,
            ));
        }

        if let Some(inferred_type) = error {
            field_task_index += 1;

            builder.insert_error(field_task_index);

            let mut path = path.clone();

            path.push_back(PathElem::Field("result::error".to_string()));

            tasks_for_final_stack.push(MergeTask::Inspect(
                path.clone(),
                field_task_index,
                inferred_type,
            ));

            temp_task_queue.push_back(MergeTask::Inspect(
                path.clone(),
                field_task_index,
                inferred_type,
            ));
        }
    }

    pub fn update_tuple_builder_and_update_tasks<'a>(
        path: &Path,
        field_task_index: TaskIndex,
        builder: &mut TupleBuilder,
        inferred_types: &'a Vec<InferredType>,
        tasks_for_final_stack: &mut Vec<MergeTask<'a>>,
        temp_task_queue: &mut VecDeque<MergeTask<'a>>,
    ) {
        let mut field_task_index = field_task_index;

        let mut indices = vec![];

        for (i, inferred_type) in inferred_types.into_iter().enumerate() {
            field_task_index += 1;

            indices.push(field_task_index);

            let mut path = path.clone();
            path.push_back(PathElem::Field(format!("tuple::{}", i)));

            tasks_for_final_stack.push(MergeTask::Inspect(
                path.clone(),
                field_task_index,
                inferred_type,
            ));

            temp_task_queue.push_back(MergeTask::Inspect(
                path.clone(),
                field_task_index,
                inferred_type,
            ));
        }

        builder.insert(indices);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inferred_type::TypeOrigin;
    use crate::rib_source_span::SourceSpan;
    use crate::PathElem;
    use test_r::test;

    #[test]
    fn test_get_task_stack_record_1() {
        let inferred_types = vec![
            InferredType::record(vec![
                (
                    "foo".to_string(),
                    InferredType::s8().add_origin(TypeOrigin::OriginatedAt(SourceSpan::default())),
                ),
                ("bar".to_string(), InferredType::u8()),
            ]),
            InferredType::record(vec![
                ("foo".to_string(), InferredType::s8()),
                (
                    "bar".to_string(),
                    InferredType::u8().add_origin(TypeOrigin::OriginatedAt(SourceSpan::default())),
                ),
            ]),
        ];

        let task_stack = get_merge_task(&inferred_types);

        let s8 = InferredType::s8();
        let u8 = InferredType::u8();

        let expected = MergeTaskStack {
            tasks: vec![
                MergeTask::RecordBuilder(RecordBuilder {
                    path: Path::default(),
                    task_index: 0,
                    field_and_pointers: vec![
                        ("foo".to_string(), vec![1, 3]),
                        ("bar".to_string(), vec![2, 4]),
                    ],
                }),
                MergeTask::Complete(1, &s8),
                MergeTask::Complete(2, &u8),
                MergeTask::Complete(3, &s8),
                MergeTask::Complete(4, &u8),
            ],
        };

        assert_eq!(&task_stack, &expected);

        let completed_task = task_stack.complete();

        let expected_type = InferredType::record(vec![
            (
                "foo".to_string(),
                InferredType::s8().add_origin(TypeOrigin::OriginatedAt(SourceSpan::default())),
            ),
            (
                "bar".to_string(),
                InferredType::u8().add_origin(TypeOrigin::OriginatedAt(SourceSpan::default())),
            ),
        ]);

        assert_eq!(completed_task, expected_type);
    }

    #[test]
    fn test_get_task_stack_record_2() {
        let inferred_types = vec![InferredType::record(vec![(
            "foo".to_string(),
            InferredType::record(vec![("bar".to_string(), InferredType::s8())]),
        )])];

        let merge_task_stack = get_merge_task(&inferred_types);

        let s8 = InferredType::s8();

        let expected_stack = MergeTaskStack {
            tasks: vec![
                MergeTask::RecordBuilder(RecordBuilder {
                    path: Path::default(),
                    task_index: 0,
                    field_and_pointers: vec![("foo".to_string(), vec![1])],
                }),
                MergeTask::RecordBuilder(RecordBuilder {
                    path: Path::from_elems(vec!["foo"]),
                    task_index: 1,
                    field_and_pointers: vec![("bar".to_string(), vec![2])],
                }),
                MergeTask::Complete(2, &s8),
            ],
        };

        assert_eq!(&merge_task_stack, &expected_stack);

        let completed_stack = merge_task_stack.complete();

        let expected_type = InferredType::record(vec![(
            "foo".to_string(),
            InferredType::record(vec![("bar".to_string(), InferredType::s8())]),
        )]);

        assert_eq!(completed_stack, expected_type);
    }

    #[test]
    fn test_get_task_stack_record_3() {
        let inferred_types = vec![InferredType::record(vec![(
            "foo".to_string(),
            InferredType::record(vec![("foo".to_string(), InferredType::s8())]),
        )])];

        let merge_task_stack = get_merge_task(&inferred_types);

        let s8 = InferredType::s8();

        let expected_stack = MergeTaskStack {
            tasks: vec![
                MergeTask::RecordBuilder(RecordBuilder {
                    path: Path::default(),
                    task_index: 0,
                    field_and_pointers: vec![("foo".to_string(), vec![1])],
                }),
                MergeTask::RecordBuilder(RecordBuilder {
                    path: {
                        let mut path = Path::default();
                        path.push_back(PathElem::Field("foo".to_string()));
                        path
                    },
                    task_index: 1,
                    field_and_pointers: vec![("foo".to_string(), vec![2])],
                }),
                MergeTask::Complete(2, &s8),
            ],
        };

        assert_eq!(&merge_task_stack, &expected_stack);

        let completed_stack = merge_task_stack.complete();

        let expected_type = InferredType::record(vec![(
            "foo".to_string(),
            InferredType::record(vec![("foo".to_string(), InferredType::s8())]),
        )]);

        assert_eq!(completed_stack, expected_type);
    }
    #[test]
    fn test_get_task_stack_record_4() {
        let inferred_type1 =
            InferredType::record(vec![("foo".to_string(), InferredType::string())]);

        let inferred_type2 = InferredType::record(vec![("foo".to_string(), InferredType::u8())]);

        let inferred_type3 = InferredType::record(vec![(
            "foo".to_string(),
            InferredType::new(
                TypeInternal::AllOf(vec![InferredType::string(), InferredType::u8()]),
                TypeOrigin::NoOrigin,
            ),
        )]);

        let inferred_types = vec![inferred_type1, inferred_type2, inferred_type3];
        let merge_task_stack = get_merge_task(&inferred_types);

        let string = InferredType::string();
        let u8 = InferredType::u8();

        let expected = MergeTaskStack {
            tasks: vec![
                MergeTask::RecordBuilder(RecordBuilder {
                    path: Path::default(),
                    task_index: 0,
                    field_and_pointers: vec![("foo".to_string(), vec![1, 2, 3])],
                }),
                MergeTask::Complete(1, &string),
                MergeTask::Complete(2, &u8),
                MergeTask::AllOfBuilder(AllOfBuilder {
                    task_index: 3,
                    pointers: vec![4, 5],
                }),
                MergeTask::Complete(4, &string),
                MergeTask::Complete(5, &u8),
            ],
        };

        assert_eq!(&merge_task_stack, &expected);

        let completed = merge_task_stack.complete();

        let expected_type = InferredType::record(vec![(
            "foo".to_string(),
            InferredType::new(
                TypeInternal::AllOf(vec![InferredType::string(), InferredType::u8()]),
                TypeOrigin::NoOrigin,
            ),
        )]);

        assert_eq!(completed, expected_type);
    }

    #[test]
    fn test_get_task_record_6() {
        let inferred_types = vec![
            InferredType::record(vec![
                ("foo".to_string(), InferredType::s8()),
                ("bar".to_string(), InferredType::u8()),
            ]),
            InferredType::record(vec![("foo".to_string(), InferredType::string())]),
        ];

        let result = get_merge_task(&inferred_types);

        let s8 = InferredType::s8();
        let u8 = InferredType::u8();
        let string = InferredType::string();

        let expected = MergeTaskStack {
            tasks: vec![
                MergeTask::RecordBuilder(RecordBuilder {
                    path: Path::default(),
                    task_index: 0,
                    field_and_pointers: vec![
                        ("foo".to_string(), vec![1]),
                        ("bar".to_string(), vec![2]),
                    ],
                }),
                MergeTask::Complete(1, &s8),
                MergeTask::Complete(2, &u8),
                MergeTask::RecordBuilder(RecordBuilder {
                    path: Path::default(),
                    task_index: 3,
                    field_and_pointers: vec![("foo".to_string(), vec![4])],
                }),
                MergeTask::Complete(4, &string),
            ],
        };

        assert_eq!(result, expected);
    }

    #[test]
    fn test_get_task_stack_record_7() {
        let all_of_internal = TypeInternal::AllOf(vec![InferredType::s8(), InferredType::u8()]);

        let all_of = InferredType::new(all_of_internal, TypeOrigin::NoOrigin);

        let inferred_types = vec![
            InferredType::record(vec![("foo".to_string(), InferredType::s8())]),
            InferredType::record(vec![("foo".to_string(), all_of)]),
        ];

        let merge_task_stack = get_merge_task(&inferred_types);

        let s8 = InferredType::s8();
        let u8 = InferredType::u8();

        let expected_stack = MergeTaskStack {
            tasks: vec![
                MergeTask::RecordBuilder(RecordBuilder {
                    path: Path::default(),
                    task_index: 0,
                    field_and_pointers: vec![("foo".to_string(), vec![1, 2])],
                }),
                MergeTask::Complete(1, &s8),
                MergeTask::AllOfBuilder(AllOfBuilder {
                    task_index: 2,
                    pointers: vec![3, 4],
                }),
                MergeTask::Complete(3, &s8),
                MergeTask::Complete(4, &u8),
            ],
        };

        assert_eq!(&merge_task_stack, &expected_stack);

        let completed = merge_task_stack.complete();

        let expected_type = InferredType::record(vec![(
            "foo".to_string(),
            InferredType::new(
                TypeInternal::AllOf(vec![InferredType::s8(), InferredType::u8()]),
                TypeOrigin::NoOrigin,
            ),
        )]);

        assert_eq!(completed, expected_type);
    }

    #[test]
    fn test_get_stack_result_1() {
        let inferred_types = vec![
            InferredType::result(Some(InferredType::s8()), Some(InferredType::u8())),
            InferredType::result(Some(InferredType::string()), Some(InferredType::s32())),
        ];

        let merge_task_stack = get_merge_task(&inferred_types);

        let s8 = InferredType::s8();
        let u8 = InferredType::u8();
        let string = InferredType::string();
        let s32 = InferredType::s32();

        let expected = MergeTaskStack {
            tasks: vec![
                MergeTask::ResultBuilder(ResultBuilder {
                    path: Path::default(),
                    task_index: 0,
                    ok: Some(vec![1, 3]),
                    error: Some(vec![2, 4]),
                }),
                MergeTask::Complete(1, &s8),
                MergeTask::Complete(2, &u8),
                MergeTask::Complete(3, &string),
                MergeTask::Complete(4, &s32),
            ],
        };

        assert_eq!(&merge_task_stack, &expected);

        let completed_task = merge_task_stack.complete();
        let expected_type = InferredType::result(
            Some(InferredType::new(
                TypeInternal::AllOf(vec![InferredType::s8(), InferredType::string()]),
                TypeOrigin::NoOrigin,
            )),
            Some(InferredType::new(
                TypeInternal::AllOf(vec![InferredType::u8(), InferredType::s32()]),
                TypeOrigin::NoOrigin,
            )),
        );

        assert_eq!(completed_task, expected_type);
    }

    #[test]
    fn test_get_stack_result_2() {
        let inner_result =
            InferredType::result(Some(InferredType::string()), Some(InferredType::u8()));

        let inferred_types = vec![InferredType::result(
            Some(inner_result),
            Some(InferredType::u8()),
        )];

        let merge_task_stack = get_merge_task(&inferred_types);

        let string = InferredType::string();
        let u8 = InferredType::u8();

        let expected = MergeTaskStack {
            tasks: vec![
                MergeTask::ResultBuilder(ResultBuilder {
                    path: Path::default(),
                    task_index: 0,
                    ok: Some(vec![1]),
                    error: Some(vec![2]),
                }),
                MergeTask::ResultBuilder(ResultBuilder {
                    path: Path::from_elems(vec!["result::ok"]),
                    task_index: 1,
                    ok: Some(vec![3]),
                    error: Some(vec![4]),
                }),
                MergeTask::Complete(2, &u8),
                MergeTask::Complete(3, &string),
                MergeTask::Complete(4, &u8),
            ],
        };

        assert_eq!(&merge_task_stack, &expected);

        let completed_task = merge_task_stack.complete();

        let expected_type = InferredType::result(
            Some(InferredType::result(
                Some(InferredType::string()),
                Some(InferredType::u8()),
            )),
            Some(InferredType::u8()),
        );

        assert_eq!(completed_task, expected_type);
    }

    #[test]
    fn test_get_stack_result_3() {
        let inner_result =
            InferredType::result(Some(InferredType::s32()), Some(InferredType::u64()));

        let inner_result = InferredType::result(Some(inner_result), Some(InferredType::u8()));

        let inferred_types = vec![InferredType::result(
            Some(InferredType::u8()),
            Some(inner_result),
        )];

        let merge_task_stack = get_merge_task(&inferred_types);

        let u8 = InferredType::u8();
        let s32 = InferredType::s32();
        let u64 = InferredType::u64();

        let expected_stack = MergeTaskStack {
            tasks: vec![
                MergeTask::ResultBuilder(ResultBuilder {
                    path: Path::default(),
                    task_index: 0,
                    ok: Some(vec![1]),
                    error: Some(vec![2]),
                }),
                MergeTask::Complete(1, &u8),
                MergeTask::ResultBuilder(ResultBuilder {
                    path: Path::from_elems(vec!["result::error"]),
                    task_index: 2,
                    ok: Some(vec![3]),
                    error: Some(vec![4]),
                }),
                MergeTask::ResultBuilder(ResultBuilder {
                    path: Path::from_elems(vec!["result::error", "result::ok"]),
                    task_index: 3,
                    ok: Some(vec![5]),
                    error: Some(vec![6]),
                }),
                MergeTask::Complete(4, &u8),
                MergeTask::Complete(5, &s32),
                MergeTask::Complete(6, &u64),
            ],
        };

        assert_eq!(&merge_task_stack, &expected_stack);

        let completed_task = merge_task_stack.complete();

        let expected_type = InferredType::result(
            Some(InferredType::u8()),
            Some(InferredType::result(
                Some(InferredType::result(
                    Some(InferredType::s32()),
                    Some(InferredType::u64()),
                )),
                Some(InferredType::u8()),
            )),
        );

        assert_eq!(completed_task, expected_type);
    }

    #[test]
    fn test_get_stack_result_4() {
        let result1 = InferredType::result(Some(InferredType::string()), Some(InferredType::u8()));

        let result2 = InferredType::result(Some(InferredType::u8()), Some(InferredType::s32()));

        let result3 = InferredType::result(
            Some(InferredType::new(
                TypeInternal::AllOf(vec![InferredType::u32(), InferredType::u64()]),
                TypeOrigin::NoOrigin,
            )),
            Some(InferredType::u8()),
        );

        let result4 = InferredType::result(
            Some(InferredType::s8()),
            Some(InferredType::new(
                TypeInternal::AllOf(vec![InferredType::u32(), InferredType::u64()]),
                TypeOrigin::NoOrigin,
            )),
        );

        let inferred_types = vec![result1, result2, result3, result4];

        let merge_task_stack = get_merge_task(&inferred_types);

        let string = InferredType::string();
        let u8 = InferredType::u8();
        let s32 = InferredType::s32();
        let s8 = InferredType::s8();
        let u32 = InferredType::u32();
        let u64 = InferredType::u64();

        let expected_stack = MergeTaskStack {
            tasks: vec![
                MergeTask::ResultBuilder(ResultBuilder {
                    path: Path::default(),
                    task_index: 0,
                    ok: Some(vec![1, 3, 5, 7]),
                    error: Some(vec![2, 4, 6, 8]),
                }),
                MergeTask::Complete(1, &string),
                MergeTask::Complete(2, &u8),
                MergeTask::Complete(3, &u8),
                MergeTask::Complete(4, &s32),
                MergeTask::AllOfBuilder(AllOfBuilder {
                    task_index: 5,
                    pointers: vec![9, 10],
                }),
                MergeTask::Complete(6, &u8),
                MergeTask::Complete(7, &s8),
                MergeTask::AllOfBuilder(AllOfBuilder {
                    task_index: 8,
                    pointers: vec![11, 12],
                }),
                MergeTask::Complete(9, &u32),
                MergeTask::Complete(10, &u64),
                MergeTask::Complete(11, &u32),
                MergeTask::Complete(12, &u64),
            ],
        };

        assert_eq!(&merge_task_stack, &expected_stack);

        let completed_task = merge_task_stack.complete();

        let expected_type = InferredType::result(
            Some(InferredType::new(
                TypeInternal::AllOf(vec![
                    InferredType::string(),
                    InferredType::u64(),
                    InferredType::u32(),
                    InferredType::u8(),
                    InferredType::s8(),
                ]),
                TypeOrigin::NoOrigin,
            )),
            Some(InferredType::new(
                TypeInternal::AllOf(vec![
                    InferredType::s32(),
                    InferredType::u32(),
                    InferredType::u8(),
                    InferredType::u64(),
                ]),
                TypeOrigin::NoOrigin,
            )),
        );

        assert_eq!(completed_task, expected_type);
    }

    #[test]
    fn test_get_task_stack_variant_1() {
        let inferred_types = vec![
            InferredType::variant(vec![
                ("with_arg".to_string(), Some(InferredType::s8())),
                ("without_arg".to_string(), None),
            ]),
            InferredType::variant(vec![
                ("with_arg".to_string(), Some(InferredType::string())),
                ("without_arg".to_string(), None),
            ]),
        ];

        let inferred_type_s8 = InferredType::s8();
        let inferred_type_string = InferredType::string();

        let merge_task_stack = get_merge_task(&inferred_types);

        let expected_stack = MergeTaskStack {
            tasks: vec![
                MergeTask::VariantBuilder(VariantBuilder {
                    task_index: 0,
                    variants: vec![
                        ("with_arg".to_string(), Some(vec![1, 2])),
                        ("without_arg".to_string(), None),
                    ],
                }),
                MergeTask::Complete(1, &inferred_type_s8),
                MergeTask::Complete(2, &inferred_type_string),
            ],
        };
        assert_eq!(&merge_task_stack, &expected_stack);

        let completed_task = merge_task_stack.complete();

        let expected_type = InferredType::variant(vec![
            (
                "with_arg".to_string(),
                Some(InferredType::new(
                    TypeInternal::AllOf(vec![InferredType::string(), InferredType::s8()]),
                    TypeOrigin::NoOrigin,
                )),
            ),
            ("without_arg".to_string(), None),
        ]);

        assert_eq!(completed_task, expected_type);
    }

    #[test]
    fn test_get_task_stack_tuple_1() {
        let inferred_types = vec![
            InferredType::tuple(vec![InferredType::s8(), InferredType::u8()]),
            InferredType::tuple(vec![InferredType::string(), InferredType::s8()]),
        ];

        let inferred_type_s8 = InferredType::s8();
        let inferred_type_u8 = InferredType::u8();
        let inferred_type_string = InferredType::string();

        let merge_task_stack = get_merge_task(&inferred_types);

        dbg!(&merge_task_stack);

        let expected_stack = MergeTaskStack {
            tasks: vec![
                MergeTask::TupleBuilder(TupleBuilder {
                    path: Path::default(),
                    task_index: 0,
                    tuple: vec![vec![1, 3], vec![2, 4]],
                }),
                MergeTask::Complete(1, &inferred_type_s8),
                MergeTask::Complete(2, &inferred_type_u8),
                MergeTask::Complete(3, &inferred_type_string),
                MergeTask::Complete(4, &inferred_type_s8),
            ],
        };

        assert_eq!(&merge_task_stack, &expected_stack);

        let completed_task = merge_task_stack.complete();

        let expected_type = InferredType::tuple(vec![
            InferredType::new(
                TypeInternal::AllOf(vec![InferredType::s8(), InferredType::string()]),
                TypeOrigin::NoOrigin,
            ),
            InferredType::new(
                TypeInternal::AllOf(vec![InferredType::u8(), InferredType::s8()]),
                TypeOrigin::NoOrigin,
            ),
        ]);

        assert_eq!(completed_task, expected_type);
    }

    #[test]
    fn test_get_task_stack_tuple_2() {
        let inferred_types = vec![InferredType::tuple(vec![
            InferredType::s8(),
            InferredType::tuple(vec![InferredType::s8()]),
        ])];

        let inferred_type_s8 = InferredType::s8();

        let merge_task_stack = get_merge_task(&inferred_types);

        let expected_stack = MergeTaskStack {
            tasks: vec![
                MergeTask::TupleBuilder(TupleBuilder {
                    path: Path::default(),
                    task_index: 0,
                    tuple: vec![vec![1], vec![2]],
                }),
                MergeTask::Complete(1, &inferred_type_s8),
                MergeTask::TupleBuilder(TupleBuilder {
                    path: Path::from_elems(vec!["tuple::1"]),
                    task_index: 2,
                    tuple: vec![vec![3]],
                }),
                MergeTask::Complete(3, &inferred_type_s8),
            ],
        };

        assert_eq!(&merge_task_stack, &expected_stack);

        let completed_task = merge_task_stack.complete();

        let expected = InferredType::tuple(vec![
            inferred_type_s8,
            InferredType::tuple(vec![InferredType::s8()]),
        ]);

        assert_eq!(completed_task, expected);
    }
}
