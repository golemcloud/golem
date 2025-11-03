use golem_rust::wasm_rpc::wasi::clocks::wall_clock::now;
use golem_rust::{with_persistence_level, PersistenceLevel};

pub fn cpu_intensive(length: u32) -> u32 {
    let length = length as usize;
    let a = vec![vec![1u32; length]; length];
    let b = vec![vec![2u32; length]; length];
    let mut c = vec![vec![0u32; length]; length];
    for i in 0..length {
        for j in 0..length {
            let mut sum = 0u32;
            for k in 0..length {
                sum = sum.wrapping_add(a[i][k].wrapping_mul(b[k][j]));
            }
            c[i][j] = sum;
        }
    }
    let mut result = 0;
    for i in 0..length {
        for j in 0..length {
            result ^= c[i][j];
        }
    }
    result
}

pub fn echo(input: String) -> String {
    input
}

pub fn large_input(input: Vec<u8>) -> u32 {
    input.len() as u32
}

pub fn oplog_heavy(length: u32, persistence_on: bool, commit: bool) -> u32 {
    let level = if persistence_on {
        PersistenceLevel::Smart
    } else {
        PersistenceLevel::PersistNothing
    };
    with_persistence_level(level, || {
        let mut result: u32 = 0;

        for _i in 0..length {
            let nanos = now().nanoseconds;
            result ^= nanos;

            if commit {
                golem_rust::oplog_commit(1);
            }
        }

        result
    })
}
