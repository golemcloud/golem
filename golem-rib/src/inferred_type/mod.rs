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

pub use type_internal::*;

pub(crate) use flatten::*;
pub(crate) use type_origin::*;
pub(crate) use unification::*;

mod flatten;
mod type_internal;
mod type_origin;
mod unification;

use crate::instance_type::InstanceType;
use crate::rib_source_span::SourceSpan;
use crate::type_inference::GetTypeHint;
use crate::TypeName;
use bigdecimal::BigDecimal;
use golem_wasm_ast::analysis::*;
use std::collections::{HashSet, VecDeque};
use std::fmt::{Display, Formatter};
use std::hash::{Hash, Hasher};
use std::ops::Deref;
use log::warn;

#[derive(Debug, Clone, Eq, PartialOrd, Ord)]
pub struct InferredType {
    pub inner: Box<TypeInternal>,
    pub origin: TypeOrigin,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Task {
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
pub struct TaskStack {
    tasks: Vec<Task>,
}

impl TaskStack {

    pub fn extend(&mut self, other: TaskStack) {
        self.tasks.extend(other.tasks);
    }

    pub fn update_record_builder(&mut self, record_builder: RecordBuilder) {
        // does it exist before
        let index = record_builder.task_index;

        if let Some(_) = self.tasks.get(index) {
            self.tasks[index] = Task::RecordBuilder(record_builder);
        } else {
            self.tasks.push(Task::RecordBuilder(record_builder));
        }

    }
    pub fn update(&mut self, index: &TaskIndex, task: Task) {
        if index < &self.tasks.len() {
            self.tasks[*index] = task;
        } else {
            self.tasks.push(task);
        }
    }

    pub fn next_index(&self) -> TaskIndex {
       self.tasks.len()
    }

    pub fn new() -> TaskStack {
        TaskStack { tasks: vec![] }
    }

    pub fn init(stack: Vec<Task>) -> TaskStack {
        TaskStack { tasks: stack }
    }

    pub fn get_record(&self, record_fields: Vec<&String>) -> Option<RecordBuilder> {
        for task in self.tasks.iter().rev(){
            match task {
                Task::RecordBuilder(builder) if builder.field_names() == record_fields =>
                    return Some(builder.clone()),

                _ => {}
            }
        }

        None
    }
}

// {foo: string}
// {foo: string}
// {foo: string}
// {foo: vec(1, 2, 3)}, inspect(string), inspect(string)
// {foo : all_of(string, u8)}
// [{foo:
fn process_inferred_type(inferred_types: Vec<InferredType>) -> TaskStack {
    let mut ephemeral_tasks = VecDeque::new();
    let tasks =
        inferred_types.iter()
            .enumerate().map(|(i, inf)| Task::Inspect(i, inf.clone())).collect::<Vec<_>>();

    ephemeral_tasks.extend(tasks.clone());

    let mut task_stack: TaskStack = TaskStack::new();

    while let Some(task) = ephemeral_tasks.pop_front() {
        match task {
            Task::Inspect(index, inferred_type) => {
                match inferred_type.internal_type() {
                    TypeInternal::Record(fields) => {
                        let mut new_builder = false;

                        let mut next_available_index = task_stack.next_index();

                        // When we accumulate two records only if the match exact. We don't take any
                        // decision on merging fields in AllOf. The same goes to other types.
                        // We are able to merge only if we fully get the type - otherwise they are kept the same
                        let record_identifier: Vec<&String> = fields.iter()
                            .map(|(field, _)| field)
                            .collect::<Vec<_>>();

                        let builder = task_stack.get_record(record_identifier).unwrap_or_else(||{
                            new_builder = true;
                            RecordBuilder::new(next_available_index, vec![])
                        });

                        let mut field_task_index = if new_builder {
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
                                Task::Inspect(field_task_index, inferred_type.clone())
                            );

                            ephemeral_tasks.push_back(Task::Inspect(field_task_index, inferred_type.clone()));

                        }

                        task_stack.update_record_builder(new_builder);

                        let new_field_task_stack = TaskStack::init(tasks);

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
                        task_stack.update(&index, Task::Complete(index.clone(), inferred_type.clone()))
                    },
                    _ => {}
                }
            }

            Task::RecordBuilder(_) => {}
            Task::AllOfBuilder(_, _) => {}
            Task::Complete(index, task) => {
                task_stack.update(&index, Task::Complete(index.clone(), task.clone()));
            }
        }
    }

   task_stack
}

impl InferredType {
    pub fn total_origins(&self) -> usize {
        let mut visitor = VecDeque::new();
        visitor.push_back(self);
        let mut total_count = 0;

        while let Some(inferred_type) = visitor.pop_front() {
            match inferred_type.inner.deref() {
                TypeInternal::AllOf(types) => {
                    for typ in types {
                        visitor.push_back(typ);
                    }
                }

                TypeInternal::Bool => {}
                TypeInternal::S8 => {}
                TypeInternal::U8 => {}
                TypeInternal::S16 => {}
                TypeInternal::U16 => {}
                TypeInternal::S32 => {}
                TypeInternal::U32 => {}
                TypeInternal::S64 => {}
                TypeInternal::U64 => {}
                TypeInternal::F32 => {}
                TypeInternal::F64 => {}
                TypeInternal::Chr => {}
                TypeInternal::Str => {}
                TypeInternal::List(inferred_type) => {
                    visitor.push_back(inferred_type);
                }
                TypeInternal::Tuple(inferred_types) => {
                    visitor.extend(inferred_types.iter());
                }
                TypeInternal::Record(fields) => {
                    for (_, inferred_type) in fields.iter() {
                        visitor.push_back(inferred_type);
                    }
                }
                TypeInternal::Flags(_) => {}
                TypeInternal::Enum(_) => {}
                TypeInternal::Option(inferred_type) => {
                    visitor.push_back(inferred_type);
                }
                TypeInternal::Result { ok, error } => {
                    if let Some(inferred_type) = ok {
                        visitor.push_back(inferred_type);
                    }
                    if let Some(inferred_type) = error {
                        visitor.push_back(inferred_type);
                    }
                }
                TypeInternal::Variant(variants) => {
                    for (_, inferred_type) in variants.iter() {
                        if let Some(inferred_type) = inferred_type {
                            visitor.push_back(inferred_type);
                        }
                    }
                }
                TypeInternal::Resource { .. } => {}
                TypeInternal::Range { from, to } => {
                    visitor.push_back(from);
                    if let Some(inferred_type) = to {
                        visitor.push_back(inferred_type);
                    }
                }
                TypeInternal::Instance { .. } => {}
                TypeInternal::Unknown => {}
                TypeInternal::Sequence(_) => {}
            }

            total_count += inferred_type.origin.total_origins();
        }

        total_count
    }
    pub fn description(&self) -> Option<String> {
        match self.critical_origin() {
            TypeOrigin::OriginatedAt(_) => None,
            TypeOrigin::Default(_) => {
                Some(format!("inferred as {} by default", self.get_type_hint()))
            }
            TypeOrigin::NoOrigin => None,
            TypeOrigin::Declared(source_location) => Some(format!(
                "inferred as {} due to the declaration at {}",
                self.get_type_hint(),
                source_location
            )),
            TypeOrigin::Multiple(_) => None,
            TypeOrigin::PatternMatch(source_location) => Some(format!(
                "inferred as {} due to the pattern match at {}",
                self.get_type_hint(),
                source_location
            )),
        }
    }
    pub fn originated_at(&self, source_span: &SourceSpan) -> InferredType {
        self.add_origin(TypeOrigin::OriginatedAt(source_span.clone()))
    }

    pub fn origin(&self) -> TypeOrigin {
        self.origin.clone()
    }

    pub fn critical_origin(&self) -> TypeOrigin {
        self.origin.critical_origin()
    }

    pub fn source_span(&self) -> Option<SourceSpan> {
        let origin = self.origin();

        match origin {
            TypeOrigin::Default(_) => None,
            TypeOrigin::NoOrigin => None,
            TypeOrigin::Declared(_) => None,
            TypeOrigin::Multiple(origins) => {
                let mut source_span = None;
                for origin in origins {
                    match origin {
                        TypeOrigin::OriginatedAt(loc) => {
                            source_span = Some(loc.clone());
                            break;
                        }
                        _ => {}
                    }
                }
                source_span
            }
            TypeOrigin::OriginatedAt(_) => None,
            TypeOrigin::PatternMatch(_) => None,
        }
    }

    pub fn as_number(&self) -> Result<InferredNumber, String> {
        fn go(with_origin: &InferredType, found: &mut Vec<InferredNumber>) -> Result<(), String> {
            match with_origin.inner.deref() {
                TypeInternal::S8 => {
                    found.push(InferredNumber::S8);
                    Ok(())
                }
                TypeInternal::U8 => {
                    found.push(InferredNumber::U8);
                    Ok(())
                }
                TypeInternal::S16 => {
                    found.push(InferredNumber::S16);
                    Ok(())
                }
                TypeInternal::U16 => {
                    found.push(InferredNumber::U16);
                    Ok(())
                }
                TypeInternal::S32 => {
                    found.push(InferredNumber::S32);
                    Ok(())
                }
                TypeInternal::U32 => {
                    found.push(InferredNumber::U32);
                    Ok(())
                }
                TypeInternal::S64 => {
                    found.push(InferredNumber::S64);
                    Ok(())
                }
                TypeInternal::U64 => {
                    found.push(InferredNumber::U64);
                    Ok(())
                }
                TypeInternal::F32 => {
                    found.push(InferredNumber::F32);
                    Ok(())
                }
                TypeInternal::F64 => {
                    found.push(InferredNumber::F64);
                    Ok(())
                }
                TypeInternal::AllOf(all_variables) => {
                    let mut previous: Option<InferredNumber> = None;
                    for variable in all_variables {
                        go(variable, found)?;

                        if let Some(current) = found.first() {
                            match &previous {
                                None => {
                                    previous = Some(current.clone());
                                    found.push(current.clone());
                                }
                                Some(previous) => {
                                    if previous != current {
                                        return Err(format!(
                                            "expected the same type of number. But found {}, {}",
                                            current, previous
                                        ));
                                    }

                                    found.push(current.clone());
                                }
                            }
                        } else {
                            return Err("failed to get a number".to_string());
                        }
                    }

                    Ok(())
                }
                TypeInternal::Range { .. } => Err("used as range".to_string()),
                TypeInternal::Bool => Err(format!("used as {}", "bool")),
                TypeInternal::Chr => Err(format!("used as {}", "char")),
                TypeInternal::Str => Err(format!("used as {}", "string")),
                TypeInternal::List(_) => Err(format!("used as {}", "list")),
                TypeInternal::Tuple(_) => Err(format!("used as {}", "tuple")),
                TypeInternal::Record(_) => Err(format!("used as {}", "record")),
                TypeInternal::Flags(_) => Err(format!("used as {}", "flags")),
                TypeInternal::Enum(_) => Err(format!("used as {}", "enum")),
                TypeInternal::Option(_) => Err(format!("used as {}", "option")),
                TypeInternal::Result { .. } => Err(format!("used as {}", "result")),
                TypeInternal::Variant(_) => Err(format!("used as {}", "variant")),
                TypeInternal::Unknown => Err("found unknown".to_string()),
                TypeInternal::Sequence(_) => {
                    Err(format!("used as {}", "function-multi-parameter-return"))
                }
                TypeInternal::Resource { .. } => Err(format!("used as {}", "resource")),
                TypeInternal::Instance { .. } => Err(format!("used as {}", "instance")),
            }
        }

        let mut found: Vec<InferredNumber> = vec![];
        go(self, &mut found)?;
        found.first().cloned().ok_or("Failed".to_string())
    }

    pub fn bool() -> InferredType {
        InferredType {
            inner: Box::new(TypeInternal::Bool),
            origin: TypeOrigin::NoOrigin,
        }
    }

    pub fn char() -> InferredType {
        InferredType {
            inner: Box::new(TypeInternal::Chr),
            origin: TypeOrigin::NoOrigin,
        }
    }

    pub fn contains_only_number(&self) -> bool {
        match self.inner.deref() {
            TypeInternal::S8
            | TypeInternal::U8
            | TypeInternal::S16
            | TypeInternal::U16
            | TypeInternal::S32
            | TypeInternal::U32
            | TypeInternal::S64
            | TypeInternal::U64
            | TypeInternal::F32
            | TypeInternal::F64 => true,
            TypeInternal::Bool => false,
            TypeInternal::Chr => false,
            TypeInternal::Str => false,
            TypeInternal::List(_) => false,
            TypeInternal::Tuple(_) => false,
            TypeInternal::Record(_) => false,
            TypeInternal::Flags(_) => false,
            TypeInternal::Enum(_) => false,
            TypeInternal::Option(_) => false,
            TypeInternal::Result { .. } => false,
            TypeInternal::Variant(_) => false,
            TypeInternal::Resource { .. } => false,
            TypeInternal::Range { .. } => false,
            TypeInternal::Instance { .. } => false,
            TypeInternal::Unknown => false,
            TypeInternal::Sequence(_) => false,
            TypeInternal::AllOf(types) => types.iter().all(|t| t.contains_only_number()),
        }
    }

    pub fn declared_at(&self, source_span: SourceSpan) -> InferredType {
        self.add_origin(TypeOrigin::Declared(source_span.clone()))
    }

    pub fn as_default(&self, default_type: DefaultType) -> InferredType {
        let new_origin = TypeOrigin::Default(default_type);

        InferredType {
            inner: self.inner.clone(),
            origin: new_origin,
        }
    }

    pub fn enum_(cases: Vec<String>) -> InferredType {
        InferredType {
            inner: Box::new(TypeInternal::Enum(cases)),
            origin: TypeOrigin::NoOrigin,
        }
    }

    pub fn f32() -> InferredType {
        InferredType {
            inner: Box::new(TypeInternal::F32),
            origin: TypeOrigin::NoOrigin,
        }
    }

    pub fn f64() -> InferredType {
        InferredType {
            inner: Box::new(TypeInternal::F64),
            origin: TypeOrigin::NoOrigin,
        }
    }

    pub fn flags(flags: Vec<String>) -> InferredType {
        InferredType {
            inner: Box::new(TypeInternal::Flags(flags)),
            origin: TypeOrigin::NoOrigin,
        }
    }

    pub fn instance(instance_type: InstanceType) -> InferredType {
        InferredType {
            inner: Box::new(TypeInternal::Instance {
                instance_type: Box::new(instance_type),
            }),
            origin: TypeOrigin::NoOrigin,
        }
    }

    pub fn internal_type(&self) -> &TypeInternal {
        self.inner.as_ref()
    }

    pub fn list(inner: InferredType) -> InferredType {
        InferredType {
            inner: Box::new(TypeInternal::List(inner)),
            origin: TypeOrigin::NoOrigin,
        }
    }

    pub fn new(inferred_type: TypeInternal, origin: TypeOrigin) -> InferredType {
        InferredType {
            inner: Box::new(inferred_type),
            origin,
        }
    }

    pub fn option(inner: InferredType) -> InferredType {
        InferredType {
            inner: Box::new(TypeInternal::Option(inner)),
            origin: TypeOrigin::NoOrigin,
        }
    }

    pub fn range(from: InferredType, to: Option<InferredType>) -> InferredType {
        InferredType {
            inner: Box::new(TypeInternal::Range { from, to }),
            origin: TypeOrigin::NoOrigin,
        }
    }

    pub fn eliminate_default(inferred_types: Vec<&InferredType>) -> Vec<&InferredType> {
        inferred_types
            .into_iter()
            .filter(|&t| !t.origin.is_default())
            .collect::<Vec<_>>()
    }

    pub fn record(fields: Vec<(String, InferredType)>) -> InferredType {
        InferredType {
            inner: Box::new(TypeInternal::Record(fields)),
            origin: TypeOrigin::NoOrigin,
        }
    }

    pub fn resolved(inferred_type: TypeInternal) -> InferredType {
        InferredType {
            inner: Box::new(inferred_type),
            origin: TypeOrigin::NoOrigin,
        }
    }

    pub fn resource(resource_id: u64, resource_mode: u8) -> InferredType {
        InferredType {
            inner: Box::new(TypeInternal::Resource {
                resource_id,
                resource_mode,
            }),
            origin: TypeOrigin::NoOrigin,
        }
    }

    pub fn result(ok: Option<InferredType>, error: Option<InferredType>) -> InferredType {
        InferredType {
            inner: Box::new(TypeInternal::Result { ok, error }),
            origin: TypeOrigin::NoOrigin,
        }
    }

    pub fn sequence(inferred_types: Vec<InferredType>) -> InferredType {
        InferredType {
            inner: Box::new(TypeInternal::Sequence(inferred_types)),
            origin: TypeOrigin::NoOrigin,
        }
    }

    pub fn string() -> InferredType {
        InferredType {
            inner: Box::new(TypeInternal::Str),
            origin: TypeOrigin::NoOrigin,
        }
    }

    pub fn s8() -> InferredType {
        InferredType {
            inner: Box::new(TypeInternal::S8),
            origin: TypeOrigin::NoOrigin,
        }
    }

    pub fn s16() -> InferredType {
        InferredType {
            inner: Box::new(TypeInternal::S16),
            origin: TypeOrigin::NoOrigin,
        }
    }

    pub fn s32() -> InferredType {
        InferredType {
            inner: Box::new(TypeInternal::S32),
            origin: TypeOrigin::NoOrigin,
        }
    }

    pub fn s64() -> InferredType {
        InferredType {
            inner: Box::new(TypeInternal::S64),
            origin: TypeOrigin::NoOrigin,
        }
    }

    pub fn tuple(inner: Vec<InferredType>) -> InferredType {
        InferredType {
            inner: Box::new(TypeInternal::Tuple(inner)),
            origin: TypeOrigin::NoOrigin,
        }
    }

    pub fn u8() -> InferredType {
        InferredType {
            inner: Box::new(TypeInternal::U8),
            origin: TypeOrigin::NoOrigin,
        }
    }

    pub fn unit() -> InferredType {
        InferredType::tuple(vec![])
    }

    pub fn unknown() -> InferredType {
        InferredType {
            inner: Box::new(TypeInternal::Unknown),
            origin: TypeOrigin::NoOrigin,
        }
    }

    pub fn u16() -> InferredType {
        InferredType {
            inner: Box::new(TypeInternal::U16),
            origin: TypeOrigin::NoOrigin,
        }
    }

    pub fn u32() -> InferredType {
        InferredType {
            inner: Box::new(TypeInternal::U32),
            origin: TypeOrigin::NoOrigin,
        }
    }

    pub fn u64() -> InferredType {
        InferredType {
            inner: Box::new(TypeInternal::U64),
            origin: TypeOrigin::NoOrigin,
        }
    }

    pub fn variant(fields: Vec<(String, Option<InferredType>)>) -> InferredType {
        InferredType {
            inner: Box::new(TypeInternal::Variant(fields)),
            origin: TypeOrigin::NoOrigin,
        }
    }

    pub fn override_origin(&self, origin: TypeOrigin) -> InferredType {
        InferredType {
            inner: self.inner.clone(),
            origin,
        }
    }

    pub fn add_origin(&self, origin: TypeOrigin) -> InferredType {
        let mut inferred_type = self.clone();
        inferred_type.add_origin_mut(origin.clone());
        inferred_type
    }

    pub fn add_origin_mut(&mut self, origin: TypeOrigin) {
        self.origin = self.origin.add_origin(origin);
    }

    pub fn without_origin(inferred_type: TypeInternal) -> InferredType {
        InferredType {
            inner: Box::new(inferred_type),
            origin: TypeOrigin::NoOrigin,
        }
    }

    pub fn printable(&self) -> String {
        // Try a fully blown type name or if it fails,
        // get the `kind` of inferred type
        TypeName::try_from(self.clone())
            .map(|tn| tn.to_string())
            .unwrap_or(self.get_type_hint().to_string())
    }

    pub fn all_of(types: Vec<InferredType>) -> Option<InferredType> {
        let flattened = InferredType::flatten_all_of_inferred_types(&types);

        let mut types: Vec<InferredType> =
            flattened.into_iter().filter(|t| !t.is_unknown()).collect();

        let mut type_map: std::collections::HashMap<InferredType, InferredType> =
            std::collections::HashMap::new();

        for t in types {
            type_map
                .entry(t.clone())
                .and_modify(|existing| {
                    if t.total_origins() > existing.total_origins() {
                        *existing = t.clone();
                    }
                })
                .or_insert(t);
        }

        if type_map.is_empty() {
            None
        } else if type_map.len() == 1 {
            type_map.into_iter().next().map(|(_, t)| t)
        } else {
            let mut unique_all_of_types: Vec<InferredType> = type_map.into_values().collect();
            unique_all_of_types.sort(); // Assuming InferredType implements Ord

            let mut origin = TypeOrigin::NoOrigin;

            Some(InferredType {
                inner: Box::new(TypeInternal::AllOf(unique_all_of_types)),
                origin,
            })
        }
    }

    pub fn is_unit(&self) -> bool {
        match self.inner.deref() {
            TypeInternal::Sequence(types) => types.is_empty(),
            _ => false,
        }
    }
    pub fn is_unknown(&self) -> bool {
        matches!(self.inner.deref(), TypeInternal::Unknown)
    }

    pub fn is_valid_wit_type(&self) -> bool {
        AnalysedType::try_from(self).is_ok()
    }

    pub fn is_all_of(&self) -> bool {
        matches!(self.inner.deref(), TypeInternal::AllOf(_))
    }

    pub fn is_number(&self) -> bool {
        matches!(
            self.inner.deref(),
            TypeInternal::S8
                | TypeInternal::U8
                | TypeInternal::S16
                | TypeInternal::U16
                | TypeInternal::S32
                | TypeInternal::U32
                | TypeInternal::S64
                | TypeInternal::U64
                | TypeInternal::F32
                | TypeInternal::F64
        )
    }

    pub fn is_string(&self) -> bool {
        matches!(self.inner.deref(), TypeInternal::Str)
    }

    pub fn flatten_all_of_inferred_types(types: &Vec<InferredType>) -> Vec<InferredType> {
        flatten_all_of_list(types)
    }

    // Here unification returns an inferred type, but it doesn't necessarily imply
    // its valid type, which can be converted to a wasm type.
    pub fn unify(&self) -> Result<InferredType, UnificationFailureInternal> {
        unify(self).map(|x| x.inferred_type())
    }

    // There is only one way to merge types. If they are different, they are merged into AllOf
    pub fn merge(&self, new_inferred_type: InferredType) -> InferredType {
        match (self.inner.deref(), new_inferred_type.inner.deref()) {
            (TypeInternal::Unknown, _) => new_inferred_type,

            (TypeInternal::AllOf(existing_types), TypeInternal::AllOf(new_types)) => {
                let mut all_types = new_types.clone();
                all_types.extend(existing_types.clone());

                InferredType::all_of(all_types).unwrap_or(InferredType::unknown())
            }

            (TypeInternal::AllOf(existing_types), _) => {
                let mut all_types = existing_types.clone();
                all_types.push(new_inferred_type);

                InferredType::all_of(all_types).unwrap_or(InferredType::unknown())
            }

            (_, TypeInternal::AllOf(new_types)) => {
                let mut all_types = new_types.clone();
                all_types.push(self.clone());

                InferredType::all_of(all_types).unwrap_or(InferredType::unknown())
            }

            (_, _) => {
                if self != &new_inferred_type && !new_inferred_type.is_unknown() {
                    InferredType::all_of(vec![self.clone(), new_inferred_type.clone()])
                        .unwrap_or(InferredType::unknown())
                } else {
                    self.clone()
                }
            }
        }
    }

    pub fn from_type_variant(type_variant: &TypeVariant) -> InferredType {
        let cases = type_variant
            .cases
            .iter()
            .map(|name_type_pair| {
                (
                    name_type_pair.name.clone(),
                    name_type_pair.typ.as_ref().map(|t| t.into()),
                )
            })
            .collect();

        InferredType::from_variant_cases(cases)
    }

    pub fn from_variant_cases(cases: Vec<(String, Option<InferredType>)>) -> InferredType {
        InferredType::without_origin(TypeInternal::Variant(cases))
    }

    pub fn from_enum_cases(type_enum: &TypeEnum) -> InferredType {
        InferredType::without_origin(TypeInternal::Enum(type_enum.cases.clone()))
    }
}

impl PartialEq for InferredType {
    fn eq(&self, other: &Self) -> bool {
        self.inner == other.inner
    }
}

impl Hash for InferredType {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.inner.hash(state);
    }
}

#[derive(PartialEq, Clone, Debug)]
pub enum InferredNumber {
    S8,
    U8,
    S16,
    U16,
    S32,
    U32,
    S64,
    U64,
    F32,
    F64,
}

impl From<&InferredNumber> for InferredType {
    fn from(inferred_number: &InferredNumber) -> Self {
        match inferred_number {
            InferredNumber::S8 => InferredType::s8(),
            InferredNumber::U8 => InferredType::u8(),
            InferredNumber::S16 => InferredType::s16(),
            InferredNumber::U16 => InferredType::u16(),
            InferredNumber::S32 => InferredType::s32(),
            InferredNumber::U32 => InferredType::u32(),
            InferredNumber::S64 => InferredType::s64(),
            InferredNumber::U64 => InferredType::u64(),
            InferredNumber::F32 => InferredType::f32(),
            InferredNumber::F64 => InferredType::f64(),
        }
    }
}

impl From<&DefaultType> for InferredType {
    fn from(default_type: &DefaultType) -> Self {
        match default_type {
            DefaultType::String => InferredType::string().as_default(default_type.clone()),
            DefaultType::F64 => InferredType::f64().as_default(default_type.clone()),
            DefaultType::S32 => InferredType::s32().as_default(default_type.clone()),
        }
    }
}

impl From<&BigDecimal> for InferredType {
    fn from(value: &BigDecimal) -> Self {
        if value.fractional_digit_count() <= 0 {
            // Rust inspired
            // https://github.com/rust-lang/rfcs/blob/master/text/0212-restore-int-fallback.md#rationale-for-the-choice-of-defaulting-to-i32
            InferredType::s32()
        } else {
            // more precision, almost same perf as f32
            InferredType::f64()
        }
    }
}

#[derive(Debug, Clone, Hash, Eq, PartialEq, Ord, PartialOrd)]
pub struct RangeType {
    from: Box<TypeInternal>,
    to: Option<Box<TypeInternal>>,
}

impl Display for InferredNumber {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let type_name = TypeName::from(self);
        write!(f, "{}", type_name)
    }
}

impl From<&AnalysedType> for InferredType {
    fn from(analysed_type: &AnalysedType) -> Self {
        match analysed_type {
            AnalysedType::Bool(_) => InferredType::bool(),
            AnalysedType::S8(_) => InferredType::s8(),
            AnalysedType::U8(_) => InferredType::u8(),
            AnalysedType::S16(_) => InferredType::s16(),
            AnalysedType::U16(_) => InferredType::u16(),
            AnalysedType::S32(_) => InferredType::s32(),
            AnalysedType::U32(_) => InferredType::u32(),
            AnalysedType::S64(_) => InferredType::s64(),
            AnalysedType::U64(_) => InferredType::u64(),
            AnalysedType::F32(_) => InferredType::f32(),
            AnalysedType::F64(_) => InferredType::f64(),
            AnalysedType::Chr(_) => InferredType::char(),
            AnalysedType::Str(_) => InferredType::string(),
            AnalysedType::List(t) => InferredType::list(t.inner.as_ref().into()),
            AnalysedType::Tuple(ts) => {
                InferredType::tuple(ts.items.iter().map(|t| t.into()).collect())
            }
            AnalysedType::Record(fs) => InferredType::record(
                fs.fields
                    .iter()
                    .map(|name_type| (name_type.name.clone(), (&name_type.typ).into()))
                    .collect(),
            ),
            AnalysedType::Flags(vs) => InferredType::flags(vs.names.clone()),
            AnalysedType::Enum(vs) => InferredType::from_enum_cases(vs),
            AnalysedType::Option(t) => InferredType::option(t.inner.as_ref().into()),
            AnalysedType::Result(golem_wasm_ast::analysis::TypeResult { ok, err, .. }) => {
                InferredType::result(
                    ok.as_ref().map(|t| t.as_ref().into()),
                    err.as_ref().map(|t| t.as_ref().into()),
                )
            }
            AnalysedType::Variant(vs) => InferredType::from_type_variant(vs),
            AnalysedType::Handle(golem_wasm_ast::analysis::TypeHandle { resource_id, mode }) => {
                InferredType::resource(
                    resource_id.0,
                    match mode {
                        AnalysedResourceMode::Owned => 0,
                        AnalysedResourceMode::Borrowed => 1,
                    },
                )
            }
        }
    }
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

       let result = process_inferred_type(inferred_types);

        let expected = TaskStack {
            tasks: vec![
                Task::RecordBuilder(
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
                Task::Complete(
                    1,
                    InferredType::s8(),
                ),
                Task::Complete(
                    2,
                    InferredType::u8()
                ),
                Task::RecordBuilder(
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
                Task::Complete(
                    4,
                    InferredType::string(),
                ),
            ],
        };

        assert_eq!(result, expected);
    }

    #[test]
    fn test_inferred_number() {
        let inferred_number = InferredNumber::S8;
        assert_eq!(inferred_number.to_string(), "s8");
    }
}
