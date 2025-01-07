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

use mappable_rc::Mrc;
use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt::Debug;
use std::hash::Hash;
use std::marker::PhantomData;

/// The analysis module contains functionality built on top of the WASM AST to analyze components.
///
/// Currently the only functionality it provides is gathering a concise representation of all the
/// exported instances and functions with their signatures.
///
/// For this analysis first parse a [component::Component] with [component::Component::from_bytes],
/// then create an [analysis::AnalysisContext] with [analysis::AnalysisContext::new] and finally call
/// [analysis::AnalysisContext::get_top_level_exports] on the newly created context.
///
/// This module is optional and can be enabled with the `analysis` feature flag. It is enabled by default.
#[cfg(feature = "analysis")]
pub mod analysis;

/// The component module contains the AST definition of the [WASM Component Model](https://github.com/WebAssembly/component-model).
///
/// This module is optional and can be enabled with the `component` feature flag. It is enabled by default.
/// When disabled the library can only work with core WASM modules.
#[cfg(feature = "component")]
pub mod component;

/// The core module contains the AST definition of a core WebAssembly module.
pub mod core;

/// The customization module defines a type for customizing various parts of the WASM AST.
///
/// There are three parts of the AST defined in the [core] module that can be replaced by user
/// defined types:
/// - [core::Expr], the node holding a WASM expression (sequence of instructions)
/// - [core::Data], the node holding a WASM data segment
/// - [core::Custom], the node holding a custom section
///
/// Replacing these with custom nodes can reduce the memory footprint of the AST if there is no need to
/// write it back to a WASM binary.
///
/// There are three predefined modes, each type can be used in the `Ast` type parameter of both [core::Module] and [component::Component]:
/// - [DefaultAst] uses the default types for all three nodes
/// - [IgnoreAll] uses replaces all three nodes with empty structures, loosing all information
/// - [IgnoreAllButMetadata] replaces all three nodes with empty structures, except those [core::Custom] nodes which are intepreted as metadata.
mod customization;

/// The metadata module defines data structures for representing various metadata extracted from WASM binaries.
///
/// This module is optional and can be enabled with the `metadata` feature flag. It is enabled by default.
#[cfg(feature = "metadata")]
pub mod metadata;

pub use customization::*;

#[cfg(test)]
test_r::enable!();

/// An index space defines one of the possible indexes various WASM nodes can belong to.
///
/// In many cases, especially in the core WASM AST, each top-level WASM node (such as data, memory, type, etc.) has its own index space.
/// Indexes to these nodes are represented by unsigned integers, and each index space are independent from each other.
///
/// In the component model many top-level AST nodes are mapped to multiple index spaces depending on their contents. For example a [component::ComponentImport] node
/// can import a module, a function, a value, a type, an instance or a component - each of these defining an entity in a different index space.
pub trait IndexSpace: Debug + PartialEq + Eq + PartialOrd + Ord {
    type Index: From<u32> + Into<u32> + Copy + Eq + Hash;
}

/// Section type defines the type of a section in a WASM binary
///
/// This is used to group sections by their type (for example to get all the functions in a module) and also to determine
/// whether a given section type supports grouping.
pub trait SectionType: Debug + PartialEq + Eq + PartialOrd + Ord {
    /// If a section type supports grouping, then sections of the same type next to each other will be serialized into a single WASM section
    /// containing multiple elements when writing out a WASM binary.
    ///
    /// Some section types does not support this encoding (such as the [core::Start] or [core::Custom] sections), in these cases they are all
    /// serialized into their own section.
    fn allow_grouping(&self) -> bool;
}

/// A section is one top level element of a WASM binary, each having an associated [IndexSpace] and [SectionType].
///
/// There are two families of sections, core WASM module sections are defined in the [core] module, while component model sections are defined in the [component] module.
pub trait Section<IS: IndexSpace, ST: SectionType>: Debug + Clone + PartialEq {
    fn index_space(&self) -> IS;
    fn section_type(&self) -> ST;
}

/// Internal representation of modules and components as a sequence of sections
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Sections<IS: IndexSpace, ST: SectionType, S: Section<IS, ST> + 'static> {
    sections: Vec<Mrc<S>>,
    phantom_is: PhantomData<IS>,
    phantom_st: PhantomData<ST>,
}

impl<IS: IndexSpace, ST: SectionType, S: Section<IS, ST>> Default for Sections<IS, ST, S> {
    fn default() -> Self {
        Self::new()
    }
}

#[allow(unused)]
impl<IS: IndexSpace, ST: SectionType, S: Section<IS, ST>> Sections<IS, ST, S> {
    pub fn new() -> Self {
        Self {
            sections: Vec::new(),
            phantom_is: PhantomData,
            phantom_st: PhantomData,
        }
    }

    pub fn from_flat(sections: Vec<S>) -> Self {
        Self {
            sections: sections.into_iter().map(|s| Mrc::new(s)).collect(),
            phantom_is: PhantomData,
            phantom_st: PhantomData,
        }
    }

    pub fn from_grouped(groups: Vec<(ST, Vec<S>)>) -> Self {
        Self {
            sections: groups
                .into_iter()
                .flat_map(|(_, sections)| sections)
                .map(|s| Mrc::new(s))
                .collect(),
            phantom_is: PhantomData,
            phantom_st: PhantomData,
        }
    }

    pub fn add_to_beginning(&mut self, section: S) {
        self.sections.insert(0, Mrc::new(section));
    }

    pub fn add_to_end(&mut self, section: S) {
        self.sections.push(Mrc::new(section));
    }

    pub fn add_to_first_group_start(&mut self, section: S) {
        let mut i = 0;
        while i < self.sections.len() {
            if self.sections[i].section_type() == section.section_type() {
                break;
            }
            i += 1;
        }
        self.sections.insert(i, Mrc::new(section));
    }

    pub fn add_to_last_group(&mut self, section: S) {
        if !self.sections.is_empty() {
            let mut i = self.sections.len() - 1;
            while i > 0 {
                if self.sections[i].section_type() == section.section_type() {
                    break;
                }
                i -= 1;
            }
            if i == 0 && self.sections[0].section_type() != section.section_type() {
                self.add_to_end(section);
            } else {
                self.sections.insert(i + 1, Mrc::new(section));
            }
        } else {
            self.add_to_end(section);
        }
    }

    pub fn clear_group(&mut self, section_type: &ST) {
        self.sections.retain(|s| s.section_type() != *section_type);
    }

    pub fn indexed(&self, index_space: &IS) -> HashMap<IS::Index, Mrc<S>> {
        self.filter_by_index_space(index_space)
            .into_iter()
            .enumerate()
            .map(|(idx, section)| (IS::Index::from(idx as u32), section))
            .collect()
    }

    pub fn filter_by_index_space(&self, index_space: &IS) -> Vec<Mrc<S>> {
        self.sections
            .iter()
            .filter(|&section| section.index_space() == *index_space)
            .cloned()
            .collect()
    }

    pub fn filter_by_section_type(&self, section_type: &ST) -> Vec<Mrc<S>> {
        self.sections
            .iter()
            .filter(|&section| section.section_type() == *section_type)
            .cloned()
            .collect()
    }

    pub fn move_to_end(&mut self, section: S) {
        self.sections.retain(|s| **s != section);
        self.sections.push(Mrc::new(section));
    }

    pub fn replace(&mut self, index_space: IS, idx: IS::Index, section: S) {
        let mut curr_idx = 0;
        let mut i = 0;
        while i < self.sections.len() {
            if self.sections[i].index_space() == index_space {
                if curr_idx == idx.into() {
                    break;
                }
                curr_idx += 1;
            }
            i += 1;
        }
        self.sections[i] = Mrc::new(section);
    }

    pub fn into_grouped(self) -> Vec<(ST, Vec<Mrc<S>>)> {
        if self.sections.is_empty() {
            Vec::new()
        } else {
            let mut grouped = Vec::new();
            let mut current_type = self.sections[0].section_type();
            let mut current_sections = Vec::new();
            for section in self.sections {
                if section.section_type() == current_type {
                    current_sections.push(section);
                } else {
                    grouped.push((current_type, current_sections));
                    current_sections = Vec::new();
                    current_type = section.section_type();
                    current_sections.push(section);
                }
            }
            grouped.push((current_type, current_sections));

            grouped
        }
    }

    pub fn take_all(&mut self) -> Vec<Mrc<S>> {
        std::mem::take(&mut self.sections)
    }
}

/// Internal structure holding references to all the items of a given section type in a [Sections] structure.
struct SectionCache<T: 'static, IS: IndexSpace, ST: SectionType, S: Section<IS, ST>> {
    cell: RefCell<Option<Vec<Mrc<T>>>>,
    section_type: ST,
    get: fn(&S) -> &T,
    index_space: PhantomData<IS>,
}

impl<T, IS: IndexSpace, ST: SectionType, S: Section<IS, ST>> SectionCache<T, IS, ST, S> {
    // TODO: helper macro
    pub fn new(section_type: ST, get: fn(&S) -> &T) -> Self {
        Self {
            cell: RefCell::new(None),
            section_type,
            get,
            index_space: PhantomData,
        }
    }

    pub fn count(&self) -> usize {
        self.cell
            .borrow()
            .as_ref()
            .map_or(0, |sections| sections.len())
    }

    pub fn invalidate(&self) {
        self.cell.replace(None);
    }

    pub fn all(&self) -> Vec<Mrc<T>> {
        self.cell
            .borrow()
            .as_ref()
            .map_or_else(Vec::new, |sections| sections.clone())
    }

    pub fn populate(&self, sections: &Sections<IS, ST, S>) {
        let mut cell = self.cell.borrow_mut();
        match cell.take() {
            Some(inner) => {
                *cell = Some(inner);
            }
            None => {
                let inner = sections
                    .filter_by_section_type(&self.section_type)
                    .into_iter()
                    .map(|section| Mrc::map(section, self.get))
                    .collect();
                *cell = Some(inner);
            }
        }
    }
}

/// Internal structure holding indexed references to all the items of a [Sections] structure belonging to a given [IndexSpace].
#[derive(Clone)]
struct SectionIndex<IS: IndexSpace, ST: SectionType, S: Section<IS, ST> + 'static> {
    cell: RefCell<Option<HashMap<IS::Index, Mrc<S>>>>,
    index_space: IS,
    section_type: PhantomData<ST>,
}

impl<IS: IndexSpace, ST: SectionType, S: Section<IS, ST>> SectionIndex<IS, ST, S> {
    pub fn new(index_space: IS) -> Self {
        Self {
            cell: RefCell::new(None),
            index_space,
            section_type: PhantomData,
        }
    }

    #[allow(unused)]
    pub fn count(&self) -> usize {
        self.cell
            .borrow()
            .as_ref()
            .map_or(0, |sections| sections.len())
    }

    pub fn get(&self, index: &IS::Index) -> Option<Mrc<S>> {
        self.cell
            .borrow()
            .as_ref()
            .and_then(|sections| sections.get(index).cloned())
    }

    pub fn invalidate(&self) {
        self.cell.replace(None);
    }

    pub fn populate(&self, sections: &Sections<IS, ST, S>) {
        let mut cell = self.cell.borrow_mut();
        match cell.take() {
            Some(inner) => {
                *cell = Some(inner);
            }
            None => {
                let inner = sections.indexed(&self.index_space);
                *cell = Some(inner);
            }
        }
    }
}

#[macro_export]
macro_rules! new_core_section_cache {
    ($tpe:ident) => {
        $crate::SectionCache::new($crate::core::CoreSectionType::$tpe, |section| {
            if let $crate::core::CoreSection::$tpe(inner) = section {
                inner
            } else {
                unreachable!()
            }
        })
    };
}

#[cfg(feature = "component")]
#[macro_export]
macro_rules! new_component_section_cache {
    ($tpe:ident) => {
        $crate::SectionCache::new($crate::component::ComponentSectionType::$tpe, |section| {
            if let $crate::component::ComponentSection::$tpe(inner) = section {
                inner
            } else {
                unreachable!()
            }
        })
    };
}
