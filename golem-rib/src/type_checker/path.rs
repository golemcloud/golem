use std::fmt;
use std::fmt::Display;

#[derive(Clone, Debug, Default)]
pub struct Path(Vec<PathElem>);

impl Path {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn current(&self) -> Option<&PathElem> {
        self.0.first()
    }

    pub fn progress(&mut self) {
        if !self.0.is_empty() {
            self.0.remove(0);
        }
    }

    pub fn from_elem(elem: PathElem) -> Self {
        Path(vec![elem])
    }

    pub fn from_elems(elems: Vec<&str>) -> Self {
        Path(
            elems
                .iter()
                .map(|x| PathElem::Field(x.to_string()))
                .collect(),
        )
    }

    pub fn push_front(&mut self, elem: PathElem) {
        self.0.insert(0, elem);
    }
}

pub enum PathType {
    RecordPath(Path),
    IndexPath(Path),
}

impl PathType {
    pub fn from_path(path: &Path) -> Option<PathType> {
        if path.0.first().map(|elem| elem.is_field()).unwrap_or(false) {
            Some(PathType::RecordPath(path.clone()))
        } else if path.0.first().map(|elem| elem.is_index()).unwrap_or(false) {
            Some(PathType::IndexPath(path.clone()))
        } else {
            None
        }
    }
}

impl Display for Path {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut is_first = true;

        for elem in &self.0 {
            match elem {
                PathElem::Field(name) => {
                    if is_first {
                        write!(f, "{}", name)?;
                        is_first = false;
                    } else {
                        write!(f, ".{}", name)?;
                    }
                }
                PathElem::Index(index) => {
                    if is_first {
                        write!(f, "index: {}", index)?;
                        is_first = false;
                    } else {
                        write!(f, "[{}]", index)?;
                    }
                }
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub enum PathElem {
    Field(String),
    Index(usize),
}

impl PathElem {
    pub fn is_field(&self) -> bool {
        matches!(self, PathElem::Field(_))
    }

    pub fn is_index(&self) -> bool {
        matches!(self, PathElem::Index(_))
    }
}
