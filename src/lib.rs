use mappable_rc::Mrc;
use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt::Debug;
use std::hash::Hash;
use std::marker::PhantomData;

#[cfg(feature = "analysis")]
pub mod analysis;
#[cfg(feature = "component")]
pub mod component;
pub mod core;
#[cfg(feature = "metadata")]
pub mod metadata;

pub trait IndexSpace: Debug + PartialEq + Eq + PartialOrd + Ord {
    type Index: From<u32> + Into<u32> + Copy + Eq + Hash;
}

pub trait SectionType: Debug + PartialEq + Eq + PartialOrd + Ord {
    fn allow_grouping(&self) -> bool;
}

pub trait Section<IS: IndexSpace, ST: SectionType>: Debug + Clone + PartialEq {
    fn index_space(&self) -> IS;
    fn section_type(&self) -> ST;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sections<IS: IndexSpace, ST: SectionType, S: Section<IS, ST> + 'static> {
    sections: Vec<Mrc<S>>,
    phantom_is: PhantomData<IS>,
    phantom_st: PhantomData<ST>,
}

impl<IS: IndexSpace, ST: SectionType, S: Section<IS, ST>> Default for Sections<IS, ST, S> {
    fn default() -> Self {
        Self::new()
    }
}

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
