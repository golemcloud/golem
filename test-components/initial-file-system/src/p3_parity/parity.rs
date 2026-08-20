use golem_rust::wasip3::filesystem::preopens as p3_preopens;
use golem_rust::wasip3::filesystem::types as p3_types;
use wasi::filesystem::preopens as p2_preopens;
use wasi::filesystem::types as p2_types;
use wasip3::wit_stream;

const P2_RECONSTRUCTION_TIMESTAMP_SECONDS: u64 = 946_684_800;
const P3_RECONSTRUCTION_TIMESTAMP_SECONDS: i64 = 978_307_200;

fn p2_err(error: p2_types::ErrorCode) -> String {
    match error {
        p2_types::ErrorCode::NotPermitted => "not-permitted".to_string(),
        other => format!("{other:?}"),
    }
}

fn p3_err(error: p3_types::ErrorCode) -> String {
    match error {
        p3_types::ErrorCode::NotPermitted => "not-permitted".to_string(),
        other => format!("{other:?}"),
    }
}

fn p2_result(result: Result<(), p2_types::ErrorCode>) -> String {
    match result {
        Ok(()) => "ok".to_string(),
        Err(error) => format!("err:{}", p2_err(error)),
    }
}

fn p3_result(result: Result<(), p3_types::ErrorCode>) -> String {
    match result {
        Ok(()) => "ok".to_string(),
        Err(error) => format!("err:{}", p3_err(error)),
    }
}

pub(crate) async fn run() -> Vec<String> {
    let mut results = Vec::new();

    let (root_p2, _) = p2_preopens::get_directories()
        .into_iter()
        .next()
        .expect("no P2 preopened directory");
    let (root_p3, _) = p3_preopens::get_directories()
        .into_iter()
        .next()
        .expect("no P3 preopened directory");

    let ro_p2 = root_p2
        .open_at(
            p2_types::PathFlags::empty(),
            "foo.txt",
            p2_types::OpenFlags::empty(),
            p2_types::DescriptorFlags::READ,
        )
        .expect("P2 open of read-only file failed");
    let ro_p3 = root_p3
        .open_at(
            p3_types::PathFlags::empty(),
            "foo.txt".to_string(),
            p3_types::OpenFlags::empty(),
            p3_types::DescriptorFlags::READ,
        )
        .await
        .expect("P3 open of read-only file failed");

    // get-flags must mask the write bit for read-only initial files
    let ro_flags_p2 = ro_p2.get_flags().expect("P2 get_flags failed");
    let ro_flags_p3 = ro_p3.get_flags().await.expect("P3 get_flags failed");
    results.push(format!(
        "ro_flags_p2_write={}",
        ro_flags_p2.contains(p2_types::DescriptorFlags::WRITE)
    ));
    results.push(format!(
        "ro_flags_p3_write={}",
        ro_flags_p3.contains(p3_types::DescriptorFlags::WRITE)
    ));

    // metadata-hash parity between P2 and P3 for the same unchanged file
    let ro_hash_p2 = ro_p2.metadata_hash().expect("P2 metadata_hash failed");
    let ro_hash_p3 = ro_p3
        .metadata_hash()
        .await
        .expect("P3 metadata_hash failed");
    results.push(format!(
        "ro_hash_parity={}",
        ro_hash_p2.lower == ro_hash_p3.lower && ro_hash_p2.upper == ro_hash_p3.upper
    ));
    let ro_hash_p3_again = ro_p3
        .metadata_hash()
        .await
        .expect("P3 metadata_hash (2nd) failed");
    results.push(format!(
        "ro_hash_p3_deterministic={}",
        ro_hash_p3.lower == ro_hash_p3_again.lower && ro_hash_p3.upper == ro_hash_p3_again.upper
    ));

    let ro_hash_at_p2 = root_p2
        .metadata_hash_at(p2_types::PathFlags::empty(), "foo.txt")
        .expect("P2 metadata_hash_at failed");
    let ro_hash_at_p3 = root_p3
        .metadata_hash_at(p3_types::PathFlags::empty(), "foo.txt".to_string())
        .await
        .expect("P3 metadata_hash_at failed");
    results.push(format!(
        "ro_hash_at_parity={}",
        ro_hash_at_p2.lower == ro_hash_at_p3.lower && ro_hash_at_p2.upper == ro_hash_at_p3.upper
    ));

    // mutations through a read-only file descriptor must be rejected identically
    results.push(format!(
        "ro_set_times_p2={}",
        p2_result(ro_p2.set_times(p2_types::NewTimestamp::Now, p2_types::NewTimestamp::Now))
    ));
    results.push(format!(
        "ro_set_times_p3={}",
        p3_result(
            ro_p3
                .set_times(p3_types::NewTimestamp::Now, p3_types::NewTimestamp::Now)
                .await
        )
    ));
    results.push(format!(
        "ro_set_times_at_p2={}",
        p2_result(ro_p2.set_times_at(
            p2_types::PathFlags::empty(),
            "x",
            p2_types::NewTimestamp::Now,
            p2_types::NewTimestamp::Now
        ))
    ));
    results.push(format!(
        "ro_set_times_at_p3={}",
        p3_result(
            ro_p3
                .set_times_at(
                    p3_types::PathFlags::empty(),
                    "x".to_string(),
                    p3_types::NewTimestamp::Now,
                    p3_types::NewTimestamp::Now
                )
                .await
        )
    ));
    results.push(format!(
        "ro_rename_at_p2={}",
        p2_result(ro_p2.rename_at("x", &root_p2, "y"))
    ));
    results.push(format!(
        "ro_rename_at_p3={}",
        p3_result(
            ro_p3
                .rename_at("x".to_string(), &root_p3, "y".to_string())
                .await
        )
    ));
    results.push(format!(
        "ro_symlink_at_p2={}",
        p2_result(ro_p2.symlink_at("x", "y"))
    ));
    results.push(format!(
        "ro_symlink_at_p3={}",
        p3_result(ro_p3.symlink_at("x".to_string(), "y".to_string()).await)
    ));
    results.push(format!(
        "ro_unlink_file_at_p2={}",
        p2_result(ro_p2.unlink_file_at("x"))
    ));
    results.push(format!(
        "ro_unlink_file_at_p3={}",
        p3_result(ro_p3.unlink_file_at("x".to_string()).await)
    ));
    results.push(format!(
        "ro_parent_open_write_p2={}",
        match root_p2.open_at(
            p2_types::PathFlags::empty(),
            "foo.txt",
            p2_types::OpenFlags::empty(),
            p2_types::DescriptorFlags::WRITE,
        ) {
            Ok(_) => "ok".to_string(),
            Err(error) => format!("err:{}", p2_err(error)),
        }
    ));
    results.push(format!(
        "ro_parent_open_write_p3={}",
        match root_p3
            .open_at(
                p3_types::PathFlags::empty(),
                "foo.txt".to_string(),
                p3_types::OpenFlags::empty(),
                p3_types::DescriptorFlags::WRITE,
            )
            .await
        {
            Ok(_) => "ok".to_string(),
            Err(error) => format!("err:{}", p3_err(error)),
        }
    ));
    results.push(format!(
        "ro_parent_unlink_p2={}",
        p2_result(root_p2.unlink_file_at("foo.txt"))
    ));
    results.push(format!(
        "ro_parent_unlink_p3={}",
        p3_result(root_p3.unlink_file_at("foo.txt".to_string()).await)
    ));
    results.push(format!(
        "ro_parent_rename_p2={}",
        p2_result(root_p2.rename_at("foo.txt", &root_p2, "foo-moved.txt"))
    ));
    results.push(format!(
        "ro_parent_rename_p3={}",
        p3_result(
            root_p3
                .rename_at("foo.txt".to_string(), &root_p3, "foo-moved.txt".to_string(),)
                .await
        )
    ));
    results.push(format!(
        "ro_parent_link_p2={}",
        p2_result(root_p2.link_at(
            p2_types::PathFlags::empty(),
            "foo.txt",
            &root_p2,
            "foo-alias.txt",
        ))
    ));
    results.push(format!(
        "ro_parent_link_p3={}",
        p3_result(
            root_p3
                .link_at(
                    p3_types::PathFlags::empty(),
                    "foo.txt".to_string(),
                    &root_p3,
                    "foo-alias.txt".to_string(),
                )
                .await
        )
    ));
    results.push(format!(
        "ro_alias_create_p2={}",
        p2_result(root_p2.symlink_at("foo.txt", "foo-link-p2"))
    ));
    results.push(format!(
        "ro_alias_open_write_p2={}",
        match root_p2.open_at(
            p2_types::PathFlags::SYMLINK_FOLLOW,
            "foo-link-p2",
            p2_types::OpenFlags::empty(),
            p2_types::DescriptorFlags::WRITE,
        ) {
            Ok(_) => "ok".to_string(),
            Err(error) => format!("err:{}", p2_err(error)),
        }
    ));
    results.push(format!(
        "ro_alias_unlink_p2={}",
        p2_result(root_p2.unlink_file_at("foo-link-p2"))
    ));
    results.push(format!(
        "ro_alias_create_p3={}",
        p3_result(
            root_p3
                .symlink_at("foo.txt".to_string(), "foo-link-p3".to_string())
                .await
        )
    ));
    results.push(format!(
        "ro_alias_open_write_p3={}",
        match root_p3
            .open_at(
                p3_types::PathFlags::SYMLINK_FOLLOW,
                "foo-link-p3".to_string(),
                p3_types::OpenFlags::empty(),
                p3_types::DescriptorFlags::WRITE,
            )
            .await
        {
            Ok(_) => "ok".to_string(),
            Err(error) => format!("err:{}", p3_err(error)),
        }
    ));
    results.push(format!(
        "ro_alias_unlink_p3={}",
        p3_result(root_p3.unlink_file_at("foo-link-p3".to_string()).await)
    ));

    let rw_p2 = root_p2
        .open_at(
            p2_types::PathFlags::empty(),
            "bar/baz.txt",
            p2_types::OpenFlags::empty(),
            p2_types::DescriptorFlags::READ | p2_types::DescriptorFlags::WRITE,
        )
        .expect("P2 open of read-write file failed");
    let rw_p3 = root_p3
        .open_at(
            p3_types::PathFlags::empty(),
            "bar/baz.txt".to_string(),
            p3_types::OpenFlags::empty(),
            p3_types::DescriptorFlags::READ | p3_types::DescriptorFlags::WRITE,
        )
        .await
        .expect("P3 open of read-write file failed");

    let rw_flags_p2 = rw_p2.get_flags().expect("P2 get_flags (rw) failed");
    let rw_flags_p3 = rw_p3.get_flags().await.expect("P3 get_flags (rw) failed");
    results.push(format!(
        "rw_flags_p2_write={}",
        rw_flags_p2.contains(p2_types::DescriptorFlags::WRITE)
    ));
    results.push(format!(
        "rw_flags_p3_write={}",
        rw_flags_p3.contains(p3_types::DescriptorFlags::WRITE)
    ));

    let rw_hash_p2 = rw_p2.metadata_hash().expect("P2 metadata_hash (rw) failed");
    let rw_hash_p3 = rw_p3
        .metadata_hash()
        .await
        .expect("P3 metadata_hash (rw) failed");
    results.push(format!(
        "rw_hash_parity={}",
        rw_hash_p2.lower == rw_hash_p3.lower && rw_hash_p2.upper == rw_hash_p3.upper
    ));

    // set-times on a read-write file must succeed through both versions
    results.push(format!(
        "rw_set_times_p2={}",
        p2_result(rw_p2.set_times(p2_types::NewTimestamp::Now, p2_types::NewTimestamp::Now))
    ));
    results.push(format!(
        "rw_set_times_p3={}",
        p3_result(
            rw_p3
                .set_times(p3_types::NewTimestamp::Now, p3_types::NewTimestamp::Now)
                .await
        )
    ));

    rw_p2.write(b"p2-to-p3", 0).expect("P2 write failed");
    let (p3_read, p3_read_result) = rw_p3.read_via_stream(0);
    let p3_bytes = p3_read.collect().await;
    p3_read_result.await.expect("P3 read after P2 write failed");
    results.push(format!(
        "p2_write_p3_read={}",
        String::from_utf8(p3_bytes).expect("P3 read was not UTF-8")
    ));

    let (mut p3_write, p3_write_data) = wit_stream::new();
    let p3_write_result = rw_p3.write_via_stream(p3_write_data, 0);
    let unwritten = p3_write.write_all(b"p3-to-p2".to_vec()).await;
    assert!(unwritten.is_empty(), "P3 stream did not accept all bytes");
    drop(p3_write);
    p3_write_result.await.expect("P3 write failed");
    let (p2_bytes, _) = rw_p2.read(8, 0).expect("P2 read after P3 write failed");
    results.push(format!(
        "p3_write_p2_read={}",
        String::from_utf8(p2_bytes).expect("P2 read was not UTF-8")
    ));

    results
}

pub(crate) async fn run_writable() -> Vec<String> {
    let (root_p2, _) = p2_preopens::get_directories()
        .into_iter()
        .next()
        .expect("no P2 preopened directory");
    let (root_p3, _) = p3_preopens::get_directories()
        .into_iter()
        .next()
        .expect("no P3 preopened directory");

    let file_p2 = root_p2
        .open_at(
            p2_types::PathFlags::empty(),
            "managed-parity.txt",
            p2_types::OpenFlags::CREATE | p2_types::OpenFlags::TRUNCATE,
            p2_types::DescriptorFlags::READ | p2_types::DescriptorFlags::WRITE,
        )
        .expect("P2 file creation failed");
    let file_p3 = root_p3
        .open_at(
            p3_types::PathFlags::empty(),
            "managed-parity.txt".to_string(),
            p3_types::OpenFlags::empty(),
            p3_types::DescriptorFlags::READ | p3_types::DescriptorFlags::WRITE,
        )
        .await
        .expect("P3 file open failed");

    file_p2.write(b"p2-to-p3", 0).expect("P2 write failed");
    let (p3_read, p3_read_result) = file_p3.read_via_stream(0);
    let p3_bytes = p3_read.collect().await;
    p3_read_result.await.expect("P3 read after P2 write failed");

    let (mut p3_write, p3_write_data) = wit_stream::new();
    let p3_write_result = file_p3.write_via_stream(p3_write_data, 0);
    let unwritten = p3_write.write_all(b"p3-to-p2".to_vec()).await;
    assert!(unwritten.is_empty(), "P3 stream did not accept all bytes");
    drop(p3_write);
    p3_write_result.await.expect("P3 write failed");
    let (p2_bytes, _) = file_p2.read(8, 0).expect("P2 read after P3 write failed");

    vec![
        format!(
            "p2_write_p3_read={}",
            String::from_utf8(p3_bytes).expect("P3 read was not UTF-8")
        ),
        format!(
            "p3_write_p2_read={}",
            String::from_utf8(p2_bytes).expect("P2 read was not UTF-8")
        ),
    ]
}

pub(crate) async fn inspect_file(path: &str) -> Vec<String> {
    let (root_p2, _) = p2_preopens::get_directories()
        .into_iter()
        .next()
        .expect("no P2 preopened directory");
    let (root_p3, _) = p3_preopens::get_directories()
        .into_iter()
        .next()
        .expect("no P3 preopened directory");
    let file_p2 = root_p2
        .open_at(
            p2_types::PathFlags::empty(),
            path,
            p2_types::OpenFlags::empty(),
            p2_types::DescriptorFlags::READ,
        )
        .expect("P2 file open failed");
    let file_p3 = root_p3
        .open_at(
            p3_types::PathFlags::empty(),
            path.to_string(),
            p3_types::OpenFlags::empty(),
            p3_types::DescriptorFlags::READ,
        )
        .await
        .expect("P3 file open failed");

    let (p2_bytes, _) = file_p2.read(64, 0).expect("P2 read failed");
    let (p3_read, p3_read_result) = file_p3.read_via_stream(0);
    let p3_bytes = p3_read.collect().await;
    p3_read_result.await.expect("P3 read failed");

    vec![
        format!(
            "p2_read={}",
            String::from_utf8(p2_bytes).expect("P2 read was not UTF-8")
        ),
        format!(
            "p3_read={}",
            String::from_utf8(p3_bytes).expect("P3 read was not UTF-8")
        ),
    ]
}

pub(crate) async fn inspect_run() -> Vec<String> {
    inspect_file("bar/baz.txt").await
}

pub(crate) async fn inspect_writable() -> Vec<String> {
    inspect_file("managed-parity.txt").await
}

pub(crate) async fn abandon_p3_write_completion() -> bool {
    let (root, _) = p3_preopens::get_directories()
        .into_iter()
        .next()
        .expect("no P3 preopened directory");
    let file = root
        .open_at(
            p3_types::PathFlags::empty(),
            "abandoned-completion.bin".to_string(),
            p3_types::OpenFlags::CREATE | p3_types::OpenFlags::TRUNCATE,
            p3_types::DescriptorFlags::READ | p3_types::DescriptorFlags::WRITE,
        )
        .await
        .expect("create abandoned-completion file");
    let (mut writer, data) = wit_stream::new();
    let completion = file.write_via_stream(data, 0);
    drop(completion);
    let expected = b"input-stream-still-drives-write".to_vec();
    assert!(writer.write_all(expected.clone()).await.is_empty());
    drop(writer);
    file.sync_data()
        .await
        .expect("synchronize abandoned-completion write");

    let (reader, completion) = file.read_via_stream(0);
    let actual = reader.collect().await;
    completion.await.expect("read abandoned-completion file");
    actual == expected
}

pub(crate) async fn run_cross_preview_append() -> bool {
    let (root_p2, _) = p2_preopens::get_directories()
        .into_iter()
        .next()
        .expect("no P2 preopened directory");
    let (root_p3, _) = p3_preopens::get_directories()
        .into_iter()
        .next()
        .expect("no P3 preopened directory");
    let path = "cross-preview-append.bin";
    let p2_file = root_p2
        .open_at(
            p2_types::PathFlags::empty(),
            path,
            p2_types::OpenFlags::CREATE | p2_types::OpenFlags::TRUNCATE,
            p2_types::DescriptorFlags::READ | p2_types::DescriptorFlags::WRITE,
        )
        .expect("create cross-preview append file through P2");
    let p3_file = root_p3
        .open_at(
            p3_types::PathFlags::empty(),
            path.to_string(),
            p3_types::OpenFlags::empty(),
            p3_types::DescriptorFlags::READ | p3_types::DescriptorFlags::WRITE,
        )
        .await
        .expect("open cross-preview append file through P3");
    let p2_bytes = vec![b'2'; 1024 * 1024];
    let p3_bytes = vec![b'3'; 1024 * 1024];
    let p2_stream = p2_file
        .append_via_stream()
        .expect("open P2 cross-preview append stream");
    p2_stream
        .write(&p2_bytes)
        .expect("start P2 cross-preview append");

    let (mut p3_writer, p3_data) = wit_stream::new();
    let p3_completion = p3_file.append_via_stream(p3_data);
    assert!(p3_writer.write_all(p3_bytes).await.is_empty());
    drop(p3_writer);
    p3_completion.await.expect("P3 cross-preview append failed");
    p2_stream
        .blocking_flush()
        .expect("finish P2 cross-preview append");

    inspect_cross_preview_append().await
}

pub(crate) async fn inspect_cross_preview_append() -> bool {
    let (root, _) = p3_preopens::get_directories()
        .into_iter()
        .next()
        .expect("no P3 preopened directory");
    let file = root
        .open_at(
            p3_types::PathFlags::empty(),
            "cross-preview-append.bin".to_string(),
            p3_types::OpenFlags::empty(),
            p3_types::DescriptorFlags::READ,
        )
        .await
        .expect("open cross-preview append result");
    let (reader, completion) = file.read_via_stream(0);
    let bytes = reader.collect().await;
    completion.await.expect("read cross-preview append result");
    let split = 1024 * 1024;
    bytes.len() == split * 2
        && ((bytes[..split].iter().all(|byte| *byte == b'2')
            && bytes[split..].iter().all(|byte| *byte == b'3'))
            || (bytes[..split].iter().all(|byte| *byte == b'3')
                && bytes[split..].iter().all(|byte| *byte == b'2')))
}

async fn write_p3(file: &p3_types::Descriptor, bytes: &[u8], offset: u64) {
    let (mut writer, data) = wit_stream::new();
    let result = file.write_via_stream(data, offset);
    assert!(
        writer.write_all(bytes.to_vec()).await.is_empty(),
        "P3 stream did not accept all bytes"
    );
    drop(writer);
    result.await.expect("P3 write failed");
}

pub(crate) async fn run_reconstruction_matrix() -> Vec<String> {
    let (root_p2, _) = p2_preopens::get_directories()
        .into_iter()
        .next()
        .expect("no P2 preopened directory");
    let (root_p3, _) = p3_preopens::get_directories()
        .into_iter()
        .next()
        .expect("no P3 preopened directory");

    let resized_p2 = root_p2
        .open_at(
            p2_types::PathFlags::empty(),
            "replay-p2-resize.bin",
            p2_types::OpenFlags::CREATE | p2_types::OpenFlags::TRUNCATE,
            p2_types::DescriptorFlags::READ | p2_types::DescriptorFlags::WRITE,
        )
        .expect("create P2 resize file");
    resized_p2
        .write(b"abcdefghijkl", 0)
        .expect("write P2 resize file");
    resized_p2.set_size(10).expect("resize P2 file");
    resized_p2.set_size(6).expect("truncate P2 file");
    resized_p2
        .set_times(
            p2_types::NewTimestamp::Timestamp(wasi::clocks::wall_clock::Datetime {
                seconds: P2_RECONSTRUCTION_TIMESTAMP_SECONDS,
                nanoseconds: 0,
            }),
            p2_types::NewTimestamp::Timestamp(wasi::clocks::wall_clock::Datetime {
                seconds: P2_RECONSTRUCTION_TIMESTAMP_SECONDS,
                nanoseconds: 0,
            }),
        )
        .expect("set P2 reconstruction timestamps");

    let appended_p2 = root_p2
        .open_at(
            p2_types::PathFlags::empty(),
            "replay-p2-append.bin",
            p2_types::OpenFlags::CREATE | p2_types::OpenFlags::TRUNCATE,
            p2_types::DescriptorFlags::READ | p2_types::DescriptorFlags::WRITE,
        )
        .expect("create P2 append file");
    appended_p2
        .write(b"p2-", 0)
        .expect("write P2 append prefix");
    let append_stream = appended_p2
        .append_via_stream()
        .expect("open P2 append stream");
    append_stream.write(b"append").expect("append P2 bytes");
    while append_stream
        .check_write()
        .expect("check P2 append readiness")
        == 0
    {
        append_stream.subscribe().block();
    }
    append_stream
        .blocking_flush()
        .expect("flush P2 append bytes");

    root_p2
        .create_directory_at("replay-p2-directory")
        .expect("create P2 directory");
    root_p2
        .create_directory_at("replay-p2-directory/removed")
        .expect("create removable P2 directory");
    root_p2
        .remove_directory_at("replay-p2-directory/removed")
        .expect("remove P2 directory");

    let splice_source = root_p2
        .open_at(
            p2_types::PathFlags::empty(),
            "replay-splice-source.bin",
            p2_types::OpenFlags::CREATE | p2_types::OpenFlags::TRUNCATE,
            p2_types::DescriptorFlags::READ | p2_types::DescriptorFlags::WRITE,
        )
        .expect("create P2 splice source");
    splice_source
        .write(b"splice-data", 0)
        .expect("write P2 splice source");
    let splice_target = root_p2
        .open_at(
            p2_types::PathFlags::empty(),
            "replay-splice-target.bin",
            p2_types::OpenFlags::CREATE | p2_types::OpenFlags::TRUNCATE,
            p2_types::DescriptorFlags::READ | p2_types::DescriptorFlags::WRITE,
        )
        .expect("create P2 splice target");
    let splice_input = splice_source
        .read_via_stream(0)
        .expect("open P2 splice input");
    let splice_output = splice_target
        .write_via_stream(0)
        .expect("open P2 splice output");
    assert_eq!(
        splice_output
            .blocking_splice(&splice_input, 11)
            .expect("splice P2 bytes"),
        11
    );
    splice_output.blocking_flush().expect("flush P2 splice");

    let hard_p2 = root_p2
        .open_at(
            p2_types::PathFlags::empty(),
            "replay-p2-hard.bin",
            p2_types::OpenFlags::CREATE | p2_types::OpenFlags::TRUNCATE,
            p2_types::DescriptorFlags::READ | p2_types::DescriptorFlags::WRITE,
        )
        .expect("create P2 hard-link source");
    hard_p2
        .write(b"hard-p2", 0)
        .expect("write P2 hard-link source");
    root_p2
        .link_at(
            p2_types::PathFlags::empty(),
            "replay-p2-hard.bin",
            &root_p2,
            "replay-p2-hard-link.bin",
        )
        .expect("create P2 hard link");
    root_p2
        .symlink_at("replay-p2-hard.bin", "replay-p2-symlink.bin")
        .expect("create P2 symlink");

    let replacement_p2 = root_p2
        .open_at(
            p2_types::PathFlags::empty(),
            "replay-p2-replacement.bin",
            p2_types::OpenFlags::CREATE | p2_types::OpenFlags::TRUNCATE,
            p2_types::DescriptorFlags::WRITE,
        )
        .expect("create P2 replacement");
    replacement_p2
        .write(b"old", 0)
        .expect("write P2 replacement");
    let replacement_source_p2 = root_p2
        .open_at(
            p2_types::PathFlags::empty(),
            "replay-p2-replacement-source.bin",
            p2_types::OpenFlags::CREATE | p2_types::OpenFlags::TRUNCATE,
            p2_types::DescriptorFlags::WRITE,
        )
        .expect("create P2 replacement source");
    replacement_source_p2
        .write(b"new-p2", 0)
        .expect("write P2 replacement source");
    drop((replacement_p2, replacement_source_p2));
    root_p2
        .rename_at(
            "replay-p2-replacement-source.bin",
            &root_p2,
            "replay-p2-replacement.bin",
        )
        .expect("replace P2 file by rename");

    let unlinked_p2 = root_p2
        .open_at(
            p2_types::PathFlags::empty(),
            "replay-p2-unlinked.bin",
            p2_types::OpenFlags::CREATE | p2_types::OpenFlags::TRUNCATE,
            p2_types::DescriptorFlags::READ | p2_types::DescriptorFlags::WRITE,
        )
        .expect("create P2 open-unlinked file");
    unlinked_p2
        .write(b"hidden-p2", 0)
        .expect("write P2 open-unlinked file");
    root_p2
        .unlink_file_at("replay-p2-unlinked.bin")
        .expect("unlink open P2 file");
    assert_eq!(
        unlinked_p2
            .read(9, 0)
            .expect("read open-unlinked P2 file")
            .0,
        b"hidden-p2"
    );
    drop(unlinked_p2);

    let resized_p3 = root_p3
        .open_at(
            p3_types::PathFlags::empty(),
            "replay-p3-resize.bin".to_string(),
            p3_types::OpenFlags::CREATE | p3_types::OpenFlags::TRUNCATE,
            p3_types::DescriptorFlags::READ | p3_types::DescriptorFlags::WRITE,
        )
        .await
        .expect("create P3 resize file");
    write_p3(&resized_p3, b"uvwxyzABCDEF", 0).await;
    resized_p3.set_size(10).await.expect("resize P3 file");
    resized_p3.set_size(6).await.expect("truncate P3 file");
    resized_p3
        .set_times(
            p3_types::NewTimestamp::Timestamp(wasip3::clocks::system_clock::Instant {
                seconds: P3_RECONSTRUCTION_TIMESTAMP_SECONDS,
                nanoseconds: 0,
            }),
            p3_types::NewTimestamp::Timestamp(wasip3::clocks::system_clock::Instant {
                seconds: P3_RECONSTRUCTION_TIMESTAMP_SECONDS,
                nanoseconds: 0,
            }),
        )
        .await
        .expect("set P3 reconstruction timestamps");

    let appended_p3 = root_p3
        .open_at(
            p3_types::PathFlags::empty(),
            "replay-p3-append.bin".to_string(),
            p3_types::OpenFlags::CREATE | p3_types::OpenFlags::TRUNCATE,
            p3_types::DescriptorFlags::READ | p3_types::DescriptorFlags::WRITE,
        )
        .await
        .expect("create P3 append file");
    write_p3(&appended_p3, b"p3-", 0).await;
    let (mut append_writer, append_data) = wit_stream::new();
    let append_result = appended_p3.append_via_stream(append_data);
    assert!(append_writer.write_all(b"append".to_vec()).await.is_empty());
    drop(append_writer);
    append_result.await.expect("append P3 bytes");

    root_p3
        .create_directory_at("replay-p3-directory".to_string())
        .await
        .expect("create P3 directory");
    root_p3
        .create_directory_at("replay-p3-directory/removed".to_string())
        .await
        .expect("create removable P3 directory");
    root_p3
        .remove_directory_at("replay-p3-directory/removed".to_string())
        .await
        .expect("remove P3 directory");

    let hard_p3 = root_p3
        .open_at(
            p3_types::PathFlags::empty(),
            "replay-p3-hard.bin".to_string(),
            p3_types::OpenFlags::CREATE | p3_types::OpenFlags::TRUNCATE,
            p3_types::DescriptorFlags::READ | p3_types::DescriptorFlags::WRITE,
        )
        .await
        .expect("create P3 hard-link source");
    write_p3(&hard_p3, b"hard-p3", 0).await;
    root_p3
        .link_at(
            p3_types::PathFlags::empty(),
            "replay-p3-hard.bin".to_string(),
            &root_p3,
            "replay-p3-hard-link.bin".to_string(),
        )
        .await
        .expect("create P3 hard link");
    root_p3
        .symlink_at(
            "replay-p3-hard.bin".to_string(),
            "replay-p3-symlink.bin".to_string(),
        )
        .await
        .expect("create P3 symlink");

    let replacement_p3 = root_p3
        .open_at(
            p3_types::PathFlags::empty(),
            "replay-p3-replacement.bin".to_string(),
            p3_types::OpenFlags::CREATE | p3_types::OpenFlags::TRUNCATE,
            p3_types::DescriptorFlags::WRITE,
        )
        .await
        .expect("create P3 replacement");
    write_p3(&replacement_p3, b"old", 0).await;
    let replacement_source_p3 = root_p3
        .open_at(
            p3_types::PathFlags::empty(),
            "replay-p3-replacement-source.bin".to_string(),
            p3_types::OpenFlags::CREATE | p3_types::OpenFlags::TRUNCATE,
            p3_types::DescriptorFlags::WRITE,
        )
        .await
        .expect("create P3 replacement source");
    write_p3(&replacement_source_p3, b"new-p3", 0).await;
    drop((replacement_p3, replacement_source_p3));
    root_p3
        .rename_at(
            "replay-p3-replacement-source.bin".to_string(),
            &root_p3,
            "replay-p3-replacement.bin".to_string(),
        )
        .await
        .expect("replace P3 file by rename");

    let unlinked_p3 = root_p3
        .open_at(
            p3_types::PathFlags::empty(),
            "replay-p3-unlinked.bin".to_string(),
            p3_types::OpenFlags::CREATE | p3_types::OpenFlags::TRUNCATE,
            p3_types::DescriptorFlags::READ | p3_types::DescriptorFlags::WRITE,
        )
        .await
        .expect("create P3 open-unlinked file");
    write_p3(&unlinked_p3, b"hidden-p3", 0).await;
    root_p3
        .unlink_file_at("replay-p3-unlinked.bin".to_string())
        .await
        .expect("unlink open P3 file");
    assert_eq!(
        unlinked_p3
            .stat()
            .await
            .expect("stat open-unlinked P3 file")
            .size,
        9
    );
    drop(unlinked_p3);

    inspect_reconstruction_matrix().await
}

pub(crate) async fn inspect_reconstruction_matrix() -> Vec<String> {
    let (root_p2, _) = p2_preopens::get_directories()
        .into_iter()
        .next()
        .expect("no P2 preopened directory");
    let (root_p3, _) = p3_preopens::get_directories()
        .into_iter()
        .next()
        .expect("no P3 preopened directory");

    let inspect_p2 = |path: &str| {
        let file = root_p2
            .open_at(
                p2_types::PathFlags::empty(),
                path,
                p2_types::OpenFlags::empty(),
                p2_types::DescriptorFlags::READ,
            )
            .expect("open reconstructed P2 file");
        let stat = file.stat().expect("stat reconstructed P2 file");
        let (bytes, _) = file.read(64, 0).expect("read reconstructed P2 file");
        (
            String::from_utf8(bytes).expect("P2 bytes were not UTF-8"),
            stat.link_count,
            stat.data_modification_timestamp,
        )
    };
    let p2_resize = inspect_p2("replay-p2-resize.bin");
    let p2_append = inspect_p2("replay-p2-append.bin");
    let p2_splice = inspect_p2("replay-splice-target.bin");
    let p2_hard = inspect_p2("replay-p2-hard.bin");
    let p2_hard_link = inspect_p2("replay-p2-hard-link.bin");
    let p2_replacement = inspect_p2("replay-p2-replacement.bin");
    let p2_symlink = root_p2
        .readlink_at("replay-p2-symlink.bin")
        .expect("read reconstructed P2 symlink");
    let p2_symlink_bytes = root_p2
        .open_at(
            p2_types::PathFlags::SYMLINK_FOLLOW,
            "replay-p2-symlink.bin",
            p2_types::OpenFlags::empty(),
            p2_types::DescriptorFlags::READ,
        )
        .expect("follow reconstructed P2 symlink")
        .read(64, 0)
        .expect("read reconstructed P2 symlink target")
        .0;

    async fn inspect_p3(
        root: &p3_types::Descriptor,
        path: &str,
    ) -> (String, u64, Option<wasip3::clocks::system_clock::Instant>) {
        let file = root
            .open_at(
                p3_types::PathFlags::empty(),
                path.to_string(),
                p3_types::OpenFlags::empty(),
                p3_types::DescriptorFlags::READ,
            )
            .await
            .expect("open reconstructed P3 file");
        let stat = file.stat().await.expect("stat reconstructed P3 file");
        let (reader, result) = file.read_via_stream(0);
        let bytes = reader.collect().await;
        result.await.expect("read reconstructed P3 file");
        (
            String::from_utf8(bytes).expect("P3 bytes were not UTF-8"),
            stat.link_count,
            stat.data_modification_timestamp,
        )
    }
    let p3_resize = inspect_p3(&root_p3, "replay-p3-resize.bin").await;
    let p3_append = inspect_p3(&root_p3, "replay-p3-append.bin").await;
    let p3_hard = inspect_p3(&root_p3, "replay-p3-hard.bin").await;
    let p3_hard_link = inspect_p3(&root_p3, "replay-p3-hard-link.bin").await;
    let p3_replacement = inspect_p3(&root_p3, "replay-p3-replacement.bin").await;
    let p3_symlink = root_p3
        .readlink_at("replay-p3-symlink.bin".to_string())
        .await
        .expect("read reconstructed P3 symlink");
    let p3_symlink_file = root_p3
        .open_at(
            p3_types::PathFlags::SYMLINK_FOLLOW,
            "replay-p3-symlink.bin".to_string(),
            p3_types::OpenFlags::empty(),
            p3_types::DescriptorFlags::READ,
        )
        .await
        .expect("follow reconstructed P3 symlink");
    let (p3_symlink_reader, p3_symlink_result) = p3_symlink_file.read_via_stream(0);
    let p3_symlink_bytes = p3_symlink_reader.collect().await;
    p3_symlink_result
        .await
        .expect("read reconstructed P3 symlink target");

    vec![
        format!("p2-resize={}", p2_resize.0),
        format!(
            "p2-times={}:{}",
            p2_resize
                .2
                .expect("P2 modification timestamp missing")
                .seconds,
            p2_resize
                .2
                .expect("P2 modification timestamp missing")
                .nanoseconds
        ),
        format!("p2-append={}", p2_append.0),
        format!(
            "p2-directory={}:removed={}",
            root_p2
                .open_at(
                    p2_types::PathFlags::empty(),
                    "replay-p2-directory",
                    p2_types::OpenFlags::DIRECTORY,
                    p2_types::DescriptorFlags::READ,
                )
                .is_ok(),
            root_p2
                .open_at(
                    p2_types::PathFlags::empty(),
                    "replay-p2-directory/removed",
                    p2_types::OpenFlags::DIRECTORY,
                    p2_types::DescriptorFlags::READ,
                )
                .is_err()
        ),
        format!("p2-splice={}", p2_splice.0),
        format!("p2-hard={}:{}", p2_hard.0, p2_hard.1),
        format!("p2-hard-link={}:{}", p2_hard_link.0, p2_hard_link.1),
        format!(
            "p2-symlink={}:{}",
            p2_symlink,
            String::from_utf8(p2_symlink_bytes).expect("P2 symlink bytes were not UTF-8")
        ),
        format!("p2-replacement={}", p2_replacement.0),
        format!(
            "p2-open-unlinked-absent={}",
            root_p2
                .open_at(
                    p2_types::PathFlags::empty(),
                    "replay-p2-unlinked.bin",
                    p2_types::OpenFlags::empty(),
                    p2_types::DescriptorFlags::READ,
                )
                .is_err()
        ),
        format!("p3-resize={}", p3_resize.0),
        format!(
            "p3-times={}:{}",
            p3_resize
                .2
                .expect("P3 modification timestamp missing")
                .seconds,
            p3_resize
                .2
                .expect("P3 modification timestamp missing")
                .nanoseconds
        ),
        format!("p3-append={}", p3_append.0),
        format!(
            "p3-directory={}:removed={}",
            root_p3
                .open_at(
                    p3_types::PathFlags::empty(),
                    "replay-p3-directory".to_string(),
                    p3_types::OpenFlags::DIRECTORY,
                    p3_types::DescriptorFlags::READ,
                )
                .await
                .is_ok(),
            root_p3
                .open_at(
                    p3_types::PathFlags::empty(),
                    "replay-p3-directory/removed".to_string(),
                    p3_types::OpenFlags::DIRECTORY,
                    p3_types::DescriptorFlags::READ,
                )
                .await
                .is_err()
        ),
        format!("p3-hard={}:{}", p3_hard.0, p3_hard.1),
        format!("p3-hard-link={}:{}", p3_hard_link.0, p3_hard_link.1),
        format!(
            "p3-symlink={p3_symlink}:{}",
            String::from_utf8(p3_symlink_bytes).expect("P3 symlink bytes were not UTF-8")
        ),
        format!("p3-replacement={}", p3_replacement.0),
        format!(
            "p3-open-unlinked-absent={}",
            root_p3
                .open_at(
                    p3_types::PathFlags::empty(),
                    "replay-p3-unlinked.bin".to_string(),
                    p3_types::OpenFlags::empty(),
                    p3_types::DescriptorFlags::READ,
                )
                .await
                .is_err()
        ),
    ]
}

pub(crate) fn write_replay_target(value: &str) {
    let (root, _) = p2_preopens::get_directories()
        .into_iter()
        .next()
        .expect("no P2 preopened directory");
    let file = root
        .open_at(
            p2_types::PathFlags::empty(),
            "replay-target.txt",
            p2_types::OpenFlags::CREATE | p2_types::OpenFlags::TRUNCATE,
            p2_types::DescriptorFlags::WRITE,
        )
        .expect("open replay target");
    file.write(value.as_bytes(), 0)
        .expect("write replay target");
}
