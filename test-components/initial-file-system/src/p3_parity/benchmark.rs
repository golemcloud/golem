use golem_rust::wasip3::filesystem::preopens as p3_preopens;
use golem_rust::wasip3::filesystem::types as p3_types;
use std::time::Instant;
use wasi::filesystem::preopens as p2_preopens;
use wasi::filesystem::types as p2_types;
use wasip3::wit_stream;

async fn execute(
    operation: &str,
    p2_file: &p2_types::Descriptor,
    p3_file: &p3_types::Descriptor,
    payload: &[u8],
) {
    match operation {
        "clock" => std::hint::black_box(()),
        "p2-read" => {
            let mut offset = 0usize;
            while offset < payload.len() {
                let (bytes, _) = p2_file
                    .read((payload.len() - offset) as u64, offset as u64)
                    .expect("P2 benchmark read failed");
                assert!(!bytes.is_empty(), "P2 benchmark read ended early");
                offset += bytes.len();
                std::hint::black_box(bytes);
            }
            assert_eq!(offset, payload.len());
        }
        "p2-write" => {
            let mut offset = 0usize;
            while offset < payload.len() {
                let written = p2_file
                    .write(&payload[offset..], offset as u64)
                    .expect("P2 benchmark write failed");
                assert!(written > 0, "P2 benchmark write made no progress");
                offset += written as usize;
            }
            assert_eq!(offset, payload.len());
        }
        "p2-read-stream" => {
            let stream = p2_file
                .read_via_stream(0)
                .expect("open P2 benchmark input stream");
            let mut offset = 0usize;
            while offset < payload.len() {
                let bytes = stream
                    .blocking_read((payload.len() - offset) as u64)
                    .expect("P2 benchmark stream read failed");
                assert!(!bytes.is_empty(), "P2 benchmark stream read ended early");
                offset += bytes.len();
                std::hint::black_box(bytes);
            }
            assert_eq!(offset, payload.len());
        }
        "p2-write-stream" => {
            let stream = p2_file
                .write_via_stream(0)
                .expect("open P2 benchmark output stream");
            stream
                .blocking_write_and_flush(payload)
                .expect("P2 benchmark stream write failed");
        }
        "p3-read-stream" => {
            let (reader, completion) = p3_file.read_via_stream(0);
            let bytes = reader.collect().await;
            completion.await.expect("P3 benchmark read failed");
            assert_eq!(bytes.len(), payload.len());
            std::hint::black_box(bytes);
        }
        "p3-write-stream" => {
            let (mut writer, data) = wit_stream::new();
            let completion = p3_file.write_via_stream(data, 0);
            let unwritten = writer.write_all(payload.to_vec()).await;
            assert!(unwritten.is_empty(), "P3 benchmark write was incomplete");
            drop(writer);
            completion.await.expect("P3 benchmark write failed");
        }
        other => panic!("unknown filesystem benchmark operation: {other}"),
    }
}

pub(crate) async fn run(
    operation: String,
    payload_size: u32,
    samples: u32,
    batch_size: u32,
) -> Vec<u64> {
    assert!(samples > 0, "benchmark samples must be positive");
    assert!(batch_size > 0, "benchmark batch size must be positive");
    assert!(samples <= 4096, "benchmark sample count is too large");
    assert!(batch_size <= 64, "benchmark batch size is too large");

    let payload = vec![0x5a; payload_size.max(1) as usize];
    let seed = if operation.contains("write") {
        vec![0xa5; payload.len()]
    } else {
        payload.clone()
    };
    let (root_p2, _) = p2_preopens::get_directories()
        .into_iter()
        .next()
        .expect("no P2 preopened directory");
    let p2_file = root_p2
        .open_at(
            p2_types::PathFlags::empty(),
            "filesystem-benchmark.bin",
            p2_types::OpenFlags::CREATE | p2_types::OpenFlags::TRUNCATE,
            p2_types::DescriptorFlags::READ | p2_types::DescriptorFlags::WRITE,
        )
        .expect("create filesystem benchmark file through P2");
    let written = p2_file
        .write(&seed, 0)
        .expect("seed filesystem benchmark file");
    assert_eq!(written, seed.len() as u64);

    let (root_p3, _) = p3_preopens::get_directories()
        .into_iter()
        .next()
        .expect("no P3 preopened directory");
    let p3_file = root_p3
        .open_at(
            p3_types::PathFlags::empty(),
            "filesystem-benchmark.bin".to_string(),
            p3_types::OpenFlags::empty(),
            p3_types::DescriptorFlags::READ | p3_types::DescriptorFlags::WRITE,
        )
        .await
        .expect("open filesystem benchmark file through P3");

    for _ in 0..4 {
        execute(&operation, &p2_file, &p3_file, &payload).await;
    }

    let mut durations = Vec::with_capacity(samples as usize);
    for _ in 0..samples {
        let started = Instant::now();
        for _ in 0..batch_size {
            execute(&operation, &p2_file, &p3_file, &payload).await;
        }
        let elapsed_per_operation = started.elapsed().as_nanos() / u128::from(batch_size);
        durations.push(u64::try_from(elapsed_per_operation).unwrap_or(u64::MAX));
    }

    let mut observed = Vec::with_capacity(payload.len());
    let mut offset = 0usize;
    while offset < payload.len() {
        let (bytes, _) = p2_file
            .read((payload.len() - offset) as u64, offset as u64)
            .expect("verify filesystem benchmark file");
        assert!(!bytes.is_empty(), "filesystem benchmark file ended early");
        offset += bytes.len();
        observed.extend(bytes);
    }
    assert_eq!(observed, payload);
    durations
}
