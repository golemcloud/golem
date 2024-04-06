use std::ops::{Index, IndexMut};
use std::{borrow::Borrow, fmt::Debug};

/// A K,V map implemented as a sorted Vec.
/// Gives better performance for lookup than a HashMap for small sizes (~10).
/// Equal performance starting around 20 elements, depending on the cost of the comparator.
/// For instance, longer String keys will quickly benefit from Hashing.
#[derive(Clone)]
pub struct VecMap<K, V> {
    values: Vec<(K, V)>,
}

impl<K, V> VecMap<K, V>
where
    K: Ord,
{
    pub fn new() -> Self {
        Self::default()
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.values.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    #[inline]
    pub fn get<Q>(&self, key: &Q) -> Option<&V>
    where
        K: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        let index = self
            .values
            .binary_search_by(|(k, _)| k.borrow().cmp(key))
            .ok()?;

        Some(&self.values[index].1)
    }

    #[inline]
    pub fn get_mut<Q>(&mut self, key: &Q) -> Option<&mut V>
    where
        K: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        let index = self
            .values
            .binary_search_by(|(k, _)| k.borrow().cmp(key))
            .ok()?;

        Some(&mut self.values[index].1)
    }

    /// Returns whether the value was inserted.
    #[inline]
    pub fn insert(&mut self, key: K, value: V) -> bool {
        let index = self.values.binary_search_by(|(k, _)| k.cmp(&key)).err();
        if let Some(index) = index {
            self.values.insert(index, (key, value));
            true
        } else {
            false
        }
    }

    #[inline]
    pub fn insert_or_update(&mut self, key: K, value: V) -> Option<(K, V)> {
        let index = self.values.binary_search_by(|(k, _)| k.cmp(&key));

        match index {
            Ok(index) => Some(std::mem::replace(&mut self.values[index], (key, value))),
            Err(index) => {
                self.values.insert(index, (key, value));
                None
            }
        }
    }
}

impl<K, V> VecMap<K, V> {
    #[inline]
    pub fn iter(&self) -> std::slice::Iter<'_, (K, V)> {
        self.values.iter()
    }

    #[inline]
    pub fn iter_mut(&mut self) -> std::slice::IterMut<'_, (K, V)> {
        self.values.iter_mut()
    }
}

impl<K, V> VecMap<K, V>
where
    K: Ord,
    V: Default,
{
    #[inline]
    pub fn get_or_default(&mut self, key: K) -> &mut V {
        let index = self.values.binary_search_by(|(k, _)| k.cmp(&key));

        match index {
            Ok(index) => &mut self.values[index].1,
            Err(index) => {
                self.values.insert(index, (key, V::default()));
                &mut self.values[index].1
            }
        }
    }
}

impl<K, V> Default for VecMap<K, V> {
    fn default() -> Self {
        Self { values: Vec::new() }
    }
}

impl<K, V> Debug for VecMap<K, V>
where
    K: Debug,
    V: Debug,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_map()
            .entries(self.values.iter().map(|(k, v)| (k, v)))
            .finish()
    }
}

impl<K, V> IntoIterator for VecMap<K, V> {
    type Item = (K, V);
    type IntoIter = std::vec::IntoIter<Self::Item>;

    fn into_iter(self) -> Self::IntoIter {
        self.values.into_iter()
    }
}

impl<K, V> Index<usize> for VecMap<K, V> {
    type Output = (K, V);

    fn index(&self, index: usize) -> &Self::Output {
        &self.values[index]
    }
}

impl<K, V> IndexMut<usize> for VecMap<K, V> {
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        &mut self.values[index]
    }
}
