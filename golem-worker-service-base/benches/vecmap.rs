use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use golem_worker_service_base::http::vecmap::VecMap;
use std::collections::{BTreeMap, HashMap};
use std::iter::repeat_with;

criterion_group!(benches, lookup_vecmap, iter_vecmap, iter_hashmap);
criterion_main!(benches);

const NUM_ENTRIES: &'static [usize] = &[5, 10, 20];
const KEY_SIZE: &'static [usize] = &[5, 10, 20];
const HIT_CHANCE: &'static [u8] = &[25, 50];

fn lookup_vecmap(c: &mut Criterion) {
    bench_lookup::<u32, u32, VecMap<u32, u32>>(
        "lookup_u32_vecmap",
        c,
        |_| fastrand::u32(0..1000),
        |_| fastrand::u32(0..1000),
    );

    bench_lookup::<String, String, VecMap<String, String>>(
        "lookup_string_vecmap",
        c,
        rand_string,
        rand_string,
    );
}

fn iter_vecmap(c: &mut Criterion) {
    // Roughly 19ns, with very little change between sizes.
    bench_iter::<u32, u32, VecMap<u32, u32>>("iter_u32_vecmap", c, rand_num, rand_num);

    bench_iter::<String, String, VecMap<String, String>>(
        "iter_string_vecmap",
        c,
        rand_string,
        rand_string,
    );
}

fn iter_hashmap(c: &mut Criterion) {
    bench_iter::<u32, u32, HashMap<u32, u32>>("iter_u32_hashmap", c, rand_num, rand_num);

    bench_iter::<String, String, HashMap<String, String>>(
        "iter_string_hashmap",
        c,
        rand_string,
        rand_string,
    );
}

trait MapTest<K, V>: Default + Clone
where
    K: 'static,
    V: 'static,
{
    fn get(&self, key: K) -> Option<&V>;
    fn insert(&mut self, key: K, value: V);
    fn iter(&self) -> impl Iterator<Item = (&K, &V)>;
}

impl<K, V> MapTest<K, V> for VecMap<K, V>
where
    K: Ord + Clone + 'static,
    V: Clone + 'static,
{
    fn get(&self, key: K) -> Option<&V> {
        self.get(&key)
    }

    fn insert(&mut self, key: K, value: V) {
        self.insert(key, value);
    }

    fn iter(&self) -> impl Iterator<Item = (&K, &V)> {
        self.iter().map(|(k, v)| (k, v))
    }
}

impl<K, V> MapTest<K, V> for HashMap<K, V>
where
    K: std::hash::Hash + Eq + Clone + 'static,
    V: Clone + 'static,
{
    fn get(&self, key: K) -> Option<&V> {
        self.get(&key)
    }

    fn insert(&mut self, key: K, value: V) {
        self.insert(key, value);
    }

    fn iter(&self) -> impl Iterator<Item = (&K, &V)> {
        self.iter()
    }
}

impl<K, V> MapTest<K, V> for BTreeMap<K, V>
where
    K: Ord + Clone + 'static,
    V: Clone + 'static,
{
    fn get(&self, key: K) -> Option<&V> {
        self.get(&key)
    }

    fn insert(&mut self, key: K, value: V) {
        self.insert(key, value);
    }

    fn iter(&self) -> impl Iterator<Item = (&K, &V)> {
        self.iter()
    }
}

fn bench_lookup<'a, K, V, M>(
    label: &'static str,
    c: &'a mut Criterion,
    key_generator: impl Fn(usize) -> K,
    value_generator: impl Fn(usize) -> V,
) -> &'a mut Criterion
where
    K: Clone + Ord + std::hash::Hash + 'static,
    V: 'static,
    M: MapTest<K, V>,
{
    let mut group = c.benchmark_group(label);
    group.sample_size(10);

    for key_size in KEY_SIZE {
        for hit_chance in HIT_CHANCE {
            for &len in NUM_ENTRIES {
                group.bench_with_input(
                    BenchmarkId::new(
                        format!(
                            "len_{}/key_size_{}/hit_chance_{}%",
                            len, key_size, hit_chance
                        ),
                        len,
                    ),
                    &len,
                    |b, &len| {
                        let mut map: M = Default::default();
                        let mut keys = Vec::new();

                        for _ in 0..len {
                            let key: K = key_generator(*key_size);
                            let value: V = value_generator(50);
                            map.insert(key.clone(), value);
                            keys.push(key);
                        }

                        b.iter_with_setup(
                            || {
                                let key: K = if fastrand::u8(0..100) < *hit_chance {
                                    fastrand::choice(&keys).unwrap().clone()
                                } else {
                                    key_generator(*key_size)
                                };
                                black_box(key)
                            },
                            |key| {
                                map.get(key);
                            },
                        );
                    },
                );
            }
        }
    }

    group.finish();
    c
}

fn bench_iter<'a, K, V, M>(
    label: &'static str,
    c: &'a mut Criterion,
    key_generator: impl Fn(usize) -> K,
    value_generator: impl Fn(usize) -> V,
) -> &'a mut Criterion
where
    K: Clone + Ord + std::hash::Hash + 'static,
    V: 'static,
    M: MapTest<K, V>,
{
    let mut group = c.benchmark_group(label);
    group.sample_size(10);

    for &len in NUM_ENTRIES {
        group.bench_with_input(
            BenchmarkId::new(format!("len_{}", len), len),
            &len,
            |b, &len| {
                let mut vecmap: M = Default::default();

                for _ in 0..len {
                    let key: K = key_generator(10);
                    let value: V = value_generator(50);
                    vecmap.insert(key, value);
                }

                b.iter_with_setup(
                    || black_box(vecmap.clone()),
                    |vecmap| {
                        for entry in vecmap.iter() {
                            black_box(entry);
                        }
                    },
                );
            },
        );
    }

    group.finish();

    c
}

fn rand_string(len: usize) -> String {
    repeat_with(fastrand::alphanumeric).take(len).collect()
}

fn rand_num(_: usize) -> u32 {
    fastrand::u32(0..1000000)
}
