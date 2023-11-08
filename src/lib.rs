use std::collections::HashMap;
use std::fmt::Debug;
use std::hash::Hash;
use std::marker::PhantomData;
use mappable_rc::Mrc;

#[cfg(feature = "component")]
pub mod component;
pub mod core;

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
                .map(|(_, sections)| sections)
                .flatten()
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
        if self.sections.len() > 0 {
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
        if self.sections.len() == 0 {
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
        std::mem::replace(&mut self.sections, Vec::new())
    }
}
