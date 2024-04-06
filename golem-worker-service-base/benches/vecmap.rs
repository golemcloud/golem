use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use golem_worker_service_base::http::vec_map::VecMap;
use std::collections::HashMap;
use std::iter::repeat_with;

criterion_group!(benches, vecmap_vs_hashmap_benchmark);
criterion_main!(benches);

fn vecmap_vs_hashmap_benchmark(c: &mut Criterion) {
    let num_entries = &[5, 10, 20, 40];
    let key_size = &[5, 10, 20];
    let hit_chance = &[25, 50, 75];

    let mut group = c.benchmark_group("comparison");
    group.sample_size(10);

    for key_size in key_size {
        for hit_chance in hit_chance {
            for &len in num_entries {
                group.bench_with_input(
                    BenchmarkId::new(
                        format!(
                            "VecMap/len_{}/key_size_{}/hit_chance_{}%",
                            len, key_size, hit_chance
                        ),
                        len,
                    ),
                    &len,
                    |b, &len| {
                        let mut vecmap = VecMap::new();
                        let mut keys = Vec::new();

                        for _ in 0..len {
                            let key: String = rand_string(*key_size);
                            let value: String = rand_string(50);
                            vecmap.insert(key.clone(), value);
                            keys.push(key);
                        }

                        b.iter_with_setup(
                            || {
                                let key: String = if fastrand::u8(0..100) < *hit_chance {
                                    fastrand::choice(&keys).unwrap().clone()
                                } else {
                                    rand_string(*key_size)
                                };
                                black_box(key)
                            },
                            |key| {
                                vecmap.get(&key);
                            },
                        );
                    },
                );

                group.bench_with_input(
                    BenchmarkId::new(
                        format!(
                            "HashMap/len_{}/key_size_{}/hit_chance_{}%",
                            len, key_size, hit_chance
                        ),
                        len,
                    ),
                    &len,
                    |b, &len| {
                        let mut hashmap = HashMap::new();
                        let mut keys = Vec::new();

                        for _ in 0..len {
                            let key: String = rand_string(*key_size);
                            let value: String = rand_string(50);
                            hashmap.insert(key.clone(), value);
                            keys.push(key);
                        }

                        b.iter_with_setup(
                            || {
                                let key: String = if fastrand::u8(0..100) < *hit_chance {
                                    fastrand::choice(&keys).unwrap().clone()
                                } else {
                                    rand_string(*key_size)
                                };
                                black_box(key)
                            },
                            |key| {
                                hashmap.get(&key);
                            },
                        );
                    },
                );
            }
        }
    }

    group.finish();
}

fn rand_string(len: usize) -> String {
    repeat_with(fastrand::alphanumeric).take(len).collect()
}
