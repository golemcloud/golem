use golem_rust::wasip3::filesystem::preopens as p3_preopens;
use golem_rust::wasip3::filesystem::types as p3_types;
use wasi::filesystem::preopens as p2_preopens;
use wasi::filesystem::types as p2_types;
use wasi::io::streams::StreamError as P2StreamError;
use wasip3::wit_stream;

fn p3_result(result: Result<(), p3_types::ErrorCode>) -> String {
    match result {
        Ok(()) => "ok".to_string(),
        Err(p3_types::ErrorCode::NotPermitted) => "err:not-permitted".to_string(),
        Err(p3_types::ErrorCode::Quota) => "err:quota".to_string(),
        Err(p3_types::ErrorCode::InsufficientSpace) => "err:insufficient-space".to_string(),
        Err(error) => format!("err:{error:?}"),
    }
}

fn p2_stream_result(result: Result<(), P2StreamError>) -> String {
    match result {
        Ok(()) => "ok".to_string(),
        Err(P2StreamError::Closed) => "err:closed".to_string(),
        Err(P2StreamError::LastOperationFailed(error)) => {
            match p2_types::filesystem_error_code(&error) {
                Some(p2_types::ErrorCode::Quota) => "err:quota".to_string(),
                Some(p2_types::ErrorCode::InsufficientSpace) => {
                    "err:insufficient-space".to_string()
                }
                Some(error) => format!("err:{error:?}"),
                None => "err:unclassified".to_string(),
            }
        }
    }
}

pub(crate) async fn run_p2_quota_surface() -> Vec<String> {
    let (root, _) = p2_preopens::get_directories()
        .into_iter()
        .next()
        .expect("no P2 preopened directory");
    let block = vec![0x5a; 4096];
    let file = root
        .open_at(
            p2_types::PathFlags::empty(),
            "quota-p2.bin",
            p2_types::OpenFlags::CREATE | p2_types::OpenFlags::TRUNCATE,
            p2_types::DescriptorFlags::READ | p2_types::DescriptorFlags::WRITE,
        )
        .expect("P2 quota file creation failed");
    let mut written_blocks = 0;
    let mut growth_denied = false;
    for index in 0..512 {
        match file.write(&block, index * block.len() as u64) {
            Ok(written) if written == block.len() as u64 => written_blocks += 1,
            Ok(_) | Err(p2_types::ErrorCode::Quota) => {
                growth_denied = true;
                break;
            }
            Err(error) => panic!("unexpected P2 quota error: {error:?}"),
        }
    }
    drop(file);
    let _ = root.unlink_file_at("quota-p2.bin");
    vec![
        format!("p2_wrote_before_limit={}", written_blocks > 0),
        format!("p2_growth_denied={growth_denied}"),
    ]
}

pub(crate) async fn run_p3_with_quota() -> bool {
    let (root, _) = p3_preopens::get_directories()
        .into_iter()
        .next()
        .expect("no P3 preopened directory");
    let block = vec![0x5a; 4096];
    let file = root
        .open_at(
            p3_types::PathFlags::empty(),
            "quota-p3.bin".to_string(),
            p3_types::OpenFlags::CREATE | p3_types::OpenFlags::TRUNCATE,
            p3_types::DescriptorFlags::READ | p3_types::DescriptorFlags::WRITE,
        )
        .await
        .expect("P3 quota file creation failed");
    let (mut stream, data) = wit_stream::new();
    let result = file.write_via_stream(data, 0);
    let unwritten = stream.write_all(block).await;
    drop(stream);
    let written = unwritten.is_empty() && result.await.is_ok();
    drop(file);
    let _ = root.unlink_file_at("quota-p3.bin".to_string()).await;
    written
}

pub(crate) async fn exhaust_p3_quota() -> Vec<String> {
    let (root, _) = p3_preopens::get_directories()
        .into_iter()
        .next()
        .expect("no P3 preopened directory");
    let file = root
        .open_at(
            p3_types::PathFlags::empty(),
            "quota-p3-exhaustion.bin".to_string(),
            p3_types::OpenFlags::CREATE | p3_types::OpenFlags::TRUNCATE,
            p3_types::DescriptorFlags::WRITE,
        )
        .await
        .expect("P3 quota file creation failed");
    let (mut stream, data) = wit_stream::new();
    let result = file.write_via_stream(data, 0);
    let mut written_blocks = 0;
    let mut unwritten = Vec::new();
    for _ in 0..512 {
        unwritten = stream.write_all(vec![0x5a; 4096]).await;
        if unwritten.is_empty() {
            written_blocks += 1;
        } else {
            break;
        }
    }
    drop(stream);
    vec![
        format!("completion={}", p3_result(result.await)),
        format!("prefix-persisted={}", written_blocks > 0),
        format!("unwritten-bytes={}", unwritten.len()),
    ]
}

pub(crate) fn exhaust_p2_quota() -> Vec<String> {
    let (root, _) = p2_preopens::get_directories()
        .into_iter()
        .next()
        .expect("no P2 preopened directory");
    let file = root
        .open_at(
            p2_types::PathFlags::empty(),
            "quota-p2-exhaustion.bin",
            p2_types::OpenFlags::CREATE | p2_types::OpenFlags::TRUNCATE,
            p2_types::DescriptorFlags::WRITE,
        )
        .expect("P2 quota file creation failed");
    let output = file
        .write_via_stream(0)
        .expect("P2 quota output stream creation failed");
    let mut written_blocks = 0;
    let completion = loop {
        let result = output.blocking_write_and_flush(&vec![0x4a; 4096]);
        if result.is_ok() {
            written_blocks += 1;
            assert!(written_blocks < 512, "P2 project quota was not enforced");
        } else {
            break p2_stream_result(result);
        }
    };
    vec![
        format!("completion={completion}"),
        format!("prefix-persisted={}", written_blocks > 0),
    ]
}

pub(crate) fn inspect_p2_exhaustion() -> Vec<String> {
    let (root, _) = p2_preopens::get_directories()
        .into_iter()
        .next()
        .expect("no P2 preopened directory");
    let file = root
        .open_at(
            p2_types::PathFlags::empty(),
            "quota-p2-exhaustion.bin",
            p2_types::OpenFlags::empty(),
            p2_types::DescriptorFlags::READ,
        )
        .expect("open P2 quota exhaustion file");
    let size = file.stat().expect("stat P2 quota exhaustion file").size;
    let (bytes, _) = file.read(size, 0).expect("read P2 quota exhaustion file");
    vec![
        format!("size={size}"),
        format!(
            "prefix-complete={}",
            bytes.len() >= 4096 && bytes[..4096].iter().all(|byte| *byte == 0x4a)
        ),
        format!(
            "suffix-bytes={}",
            bytes[bytes.len().min(4096)..]
                .iter()
                .filter(|byte| **byte == 0x6b)
                .count()
        ),
    ]
}

pub(crate) async fn inspect_p3_exhaustion() -> Vec<String> {
    let (root, _) = p3_preopens::get_directories()
        .into_iter()
        .next()
        .expect("no P3 preopened directory");
    let file = root
        .open_at(
            p3_types::PathFlags::empty(),
            "quota-p3-exhaustion.bin".to_string(),
            p3_types::OpenFlags::empty(),
            p3_types::DescriptorFlags::READ,
        )
        .await
        .expect("open P3 quota exhaustion file");
    let size = file
        .stat()
        .await
        .expect("stat P3 quota exhaustion file")
        .size;
    let (reader, result) = file.read_via_stream(0);
    let bytes = reader.collect().await;
    result.await.expect("read P3 quota exhaustion file");
    vec![
        format!("size={size}"),
        format!(
            "prefix-complete={}",
            bytes.len() >= 4096 && bytes[..4096].iter().all(|byte| *byte == 0x5a)
        ),
        format!(
            "suffix-bytes={}",
            bytes[bytes.len().min(4096)..]
                .iter()
                .filter(|byte| **byte == 0x6c)
                .count()
        ),
    ]
}

pub(crate) async fn run_p2_quota_matrix() -> Vec<String> {
    let (root, _) = p2_preopens::get_directories()
        .into_iter()
        .next()
        .expect("no P2 preopened directory");
    let block = vec![0x2a; 4096];
    let file = root
        .open_at(
            p2_types::PathFlags::empty(),
            "p2-matrix.bin",
            p2_types::OpenFlags::CREATE | p2_types::OpenFlags::TRUNCATE,
            p2_types::DescriptorFlags::READ | p2_types::DescriptorFlags::WRITE,
        )
        .expect("create P2 matrix file");
    let direct = file.write(&block, 0).is_ok();
    let positioned = file
        .write_via_stream(4096)
        .and_then(|stream| {
            stream
                .blocking_write_and_flush(&block)
                .map_err(|_| p2_types::ErrorCode::Io)
        })
        .is_ok();
    let appended = file
        .append_via_stream()
        .and_then(|stream| {
            stream
                .blocking_write_and_flush(&block)
                .map_err(|_| p2_types::ErrorCode::Io)
        })
        .is_ok();
    let sparse_resize = file.set_size(512 * 1024).is_ok();
    let overwrite = file.write(&block, 0).is_ok();
    let truncate = file.set_size(8192).is_ok();

    let source = root
        .open_at(
            p2_types::PathFlags::empty(),
            "p2-splice-source.bin",
            p2_types::OpenFlags::CREATE | p2_types::OpenFlags::TRUNCATE,
            p2_types::DescriptorFlags::READ | p2_types::DescriptorFlags::WRITE,
        )
        .expect("create P2 splice source");
    source.write(&block, 0).expect("write P2 splice source");
    let input = source.read_via_stream(0).expect("open P2 splice input");
    let output = file.append_via_stream().expect("open P2 splice output");
    let splice = output.blocking_splice(&input, block.len() as u64).is_ok()
        && output.blocking_flush().is_ok();

    root.link_at(
        p2_types::PathFlags::empty(),
        "p2-matrix.bin",
        &root,
        "p2-matrix-link.bin",
    )
    .expect("create P2 hard link");
    let hard_link = file.stat().is_ok_and(|stat| stat.link_count == 2);
    root.unlink_file_at("p2-matrix.bin")
        .expect("unlink first P2 name");
    let first_unlink = root
        .open_at(
            p2_types::PathFlags::empty(),
            "p2-matrix-link.bin",
            p2_types::OpenFlags::empty(),
            p2_types::DescriptorFlags::READ,
        )
        .and_then(|alias| alias.read(1, 0))
        .is_ok();
    root.unlink_file_at("p2-matrix-link.bin")
        .expect("unlink final P2 name");
    let open_unlinked = file.read(1, 0).is_ok();

    let replacement = root
        .open_at(
            p2_types::PathFlags::empty(),
            "p2-replacement.bin",
            p2_types::OpenFlags::CREATE,
            p2_types::DescriptorFlags::READ | p2_types::DescriptorFlags::WRITE,
        )
        .expect("create P2 replacement target");
    replacement.write(b"old", 0).expect("write P2 replacement");
    root.rename_at("p2-splice-source.bin", &root, "p2-replacement.bin")
        .expect("replace P2 target by rename");
    let rename_replace = root
        .open_at(
            p2_types::PathFlags::empty(),
            "p2-replacement.bin",
            p2_types::OpenFlags::empty(),
            p2_types::DescriptorFlags::READ,
        )
        .and_then(|renamed| renamed.read(1, 0))
        .is_ok_and(|(bytes, _)| bytes == vec![0x2a]);
    root.create_directory_at("p2-metadata-dir")
        .expect("create P2 directory");
    root.symlink_at("p2-replacement.bin", "p2-metadata-link")
        .expect("create P2 symlink");

    let growth = root
        .open_at(
            p2_types::PathFlags::empty(),
            "p2-growth.bin",
            p2_types::OpenFlags::CREATE,
            p2_types::DescriptorFlags::WRITE,
        )
        .expect("create P2 growth file");
    let mut grew = false;
    let mut growth_denied = false;
    for index in 0..512 {
        match growth.write(&block, index * block.len() as u64) {
            Ok(written) if written == block.len() as u64 => {
                grew = true;
                growth.sync_data().expect("settle P2 matrix quota usage");
            }
            Ok(written) => {
                grew |= written > 0;
                growth_denied = true;
                break;
            }
            Err(p2_types::ErrorCode::Quota) => {
                growth_denied = true;
                break;
            }
            Err(error) => panic!("unexpected P2 matrix growth error: {error:?}"),
        }
    }
    vec![
        format!("direct={direct}"),
        format!("positioned-stream={positioned}"),
        format!("append={appended}"),
        format!("sparse-resize={sparse_resize}"),
        format!("overwrite={overwrite}"),
        format!("truncate={truncate}"),
        format!("splice={splice}"),
        format!("hard-link={hard_link}"),
        format!("first-unlink={first_unlink}"),
        format!("open-unlinked={open_unlinked}"),
        format!("rename-replace={rename_replace}"),
        format!("grew={grew}"),
        format!("growth-denied={growth_denied}"),
    ]
}

pub(crate) async fn run_p3_quota_matrix() -> Vec<String> {
    let (root, _) = p3_preopens::get_directories()
        .into_iter()
        .next()
        .expect("no P3 preopened directory");
    let block = vec![0x3b; 4096];
    let file = root
        .open_at(
            p3_types::PathFlags::empty(),
            "p3-matrix.bin".to_string(),
            p3_types::OpenFlags::CREATE | p3_types::OpenFlags::TRUNCATE,
            p3_types::DescriptorFlags::READ | p3_types::DescriptorFlags::WRITE,
        )
        .await
        .expect("create P3 matrix file");
    let (mut positioned_tx, positioned_data) = wit_stream::new();
    let positioned_result = file.write_via_stream(positioned_data, 0);
    let positioned = positioned_tx.write_all(block.clone()).await.is_empty();
    drop(positioned_tx);
    let positioned = positioned && positioned_result.await.is_ok();
    let (mut append_tx, append_data) = wit_stream::new();
    let append_result = file.append_via_stream(append_data);
    let appended = append_tx.write_all(block.clone()).await.is_empty();
    drop(append_tx);
    let appended = appended && append_result.await.is_ok();
    let sparse_resize = file.set_size(512 * 1024).await.is_ok();
    let (mut overwrite_tx, overwrite_data) = wit_stream::new();
    let overwrite_result = file.write_via_stream(overwrite_data, 0);
    let overwrite = overwrite_tx.write_all(block.clone()).await.is_empty();
    drop(overwrite_tx);
    let overwrite = overwrite && overwrite_result.await.is_ok();
    let truncate = file.set_size(8192).await.is_ok();

    root.link_at(
        p3_types::PathFlags::empty(),
        "p3-matrix.bin".to_string(),
        &root,
        "p3-matrix-link.bin".to_string(),
    )
    .await
    .expect("create P3 hard link");
    let hard_link = file.stat().await.is_ok_and(|stat| stat.link_count == 2);
    root.unlink_file_at("p3-matrix.bin".to_string())
        .await
        .expect("unlink first P3 name");
    let first_unlink = root
        .open_at(
            p3_types::PathFlags::empty(),
            "p3-matrix-link.bin".to_string(),
            p3_types::OpenFlags::empty(),
            p3_types::DescriptorFlags::READ,
        )
        .await
        .is_ok();
    root.unlink_file_at("p3-matrix-link.bin".to_string())
        .await
        .expect("unlink final P3 name");
    let open_unlinked = file.stat().await.is_ok();

    let source = root
        .open_at(
            p3_types::PathFlags::empty(),
            "p3-source.bin".to_string(),
            p3_types::OpenFlags::CREATE,
            p3_types::DescriptorFlags::READ | p3_types::DescriptorFlags::WRITE,
        )
        .await
        .expect("create P3 rename source");
    let (mut source_tx, source_data) = wit_stream::new();
    let source_result = source.write_via_stream(source_data, 0);
    assert!(source_tx.write_all(block.clone()).await.is_empty());
    drop(source_tx);
    source_result.await.expect("write P3 rename source");
    let replacement = root
        .open_at(
            p3_types::PathFlags::empty(),
            "p3-replacement.bin".to_string(),
            p3_types::OpenFlags::CREATE,
            p3_types::DescriptorFlags::WRITE,
        )
        .await
        .expect("create P3 replacement target");
    drop(replacement);
    root.rename_at(
        "p3-source.bin".to_string(),
        &root,
        "p3-replacement.bin".to_string(),
    )
    .await
    .expect("replace P3 target by rename");
    let rename_replace = root
        .open_at(
            p3_types::PathFlags::empty(),
            "p3-replacement.bin".to_string(),
            p3_types::OpenFlags::empty(),
            p3_types::DescriptorFlags::READ,
        )
        .await
        .is_ok();
    root.create_directory_at("p3-metadata-dir".to_string())
        .await
        .expect("create P3 directory");
    root.symlink_at(
        "p3-replacement.bin".to_string(),
        "p3-metadata-link".to_string(),
    )
    .await
    .expect("create P3 symlink");

    let growth = root
        .open_at(
            p3_types::PathFlags::empty(),
            "p3-growth.bin".to_string(),
            p3_types::OpenFlags::CREATE,
            p3_types::DescriptorFlags::WRITE,
        )
        .await
        .expect("create P3 growth file");
    let (mut growth_tx, growth_data) = wit_stream::new();
    let growth_result = growth.write_via_stream(growth_data, 0);
    let mut grew = false;
    let mut unwritten = Vec::new();
    for _ in 0..512 {
        unwritten = growth_tx.write_all(block.clone()).await;
        if unwritten.is_empty() {
            grew = true;
        } else {
            break;
        }
    }
    drop(growth_tx);
    let growth_denied = matches!(
        growth_result.await,
        Err(p3_types::ErrorCode::Quota)
    ) && grew
        && !unwritten.is_empty();
    vec![
        format!("positioned-stream={positioned}"),
        format!("append={appended}"),
        format!("sparse-resize={sparse_resize}"),
        format!("overwrite={overwrite}"),
        format!("truncate={truncate}"),
        format!("hard-link={hard_link}"),
        format!("first-unlink={first_unlink}"),
        format!("open-unlinked={open_unlinked}"),
        format!("rename-replace={rename_replace}"),
        format!("growth-denied={growth_denied}"),
    ]
}

pub(crate) async fn run_p2_object_quota() -> Vec<String> {
    let (root, _) = p2_preopens::get_directories()
        .into_iter()
        .next()
        .expect("no P2 preopened directory");
    let held = root
        .open_at(
            p2_types::PathFlags::empty(),
            "p2-held",
            p2_types::OpenFlags::CREATE,
            p2_types::DescriptorFlags::READ,
        )
        .expect("create P2 held object");
    root.link_at(
        p2_types::PathFlags::empty(),
        "p2-held",
        &root,
        "p2-held-link",
    )
    .expect("hard link must not consume an object");
    let hard_link_same_inode = held.stat().is_ok_and(|stat| stat.link_count == 2);
    let mut created = 0;
    for index in 0..64 {
        match root.open_at(
            p2_types::PathFlags::empty(),
            &format!("p2-object-{index}"),
            p2_types::OpenFlags::CREATE,
            p2_types::DescriptorFlags::READ,
        ) {
            Ok(file) => {
                created += 1;
                drop(file);
            }
            Err(p2_types::ErrorCode::Quota) => break,
            Err(error) => panic!("unexpected P2 object creation error: {error:?}"),
        }
    }
    let object_denied = created < 64;
    let directory_denied = matches!(
        root.create_directory_at("p2-object-directory"),
        Err(p2_types::ErrorCode::Quota)
    );
    let symlink_denied = matches!(
        root.symlink_at("p2-object-0", "p2-object-symlink"),
        Err(p2_types::ErrorCode::Quota)
    );
    root.unlink_file_at("p2-held").expect("unlink P2 held name");
    root.unlink_file_at("p2-held-link")
        .expect("unlink P2 held link");
    let denied_while_open = matches!(
        root.open_at(
            p2_types::PathFlags::empty(),
            "p2-before-close",
            p2_types::OpenFlags::CREATE,
            p2_types::DescriptorFlags::READ,
        ),
        Err(p2_types::ErrorCode::Quota)
    );
    drop(held);
    vec![
        format!("hard-link-same-inode={hard_link_same_inode}"),
        format!("object-denied={object_denied}"),
        format!("directory-denied={directory_denied}"),
        format!("symlink-denied={symlink_denied}"),
        format!("denied-while-open={denied_while_open}"),
    ]
}

pub(crate) async fn complete_p2_object_quota_release() -> bool {
    let (root, _) = p2_preopens::get_directories()
        .into_iter()
        .next()
        .expect("no P2 preopened directory");
    root.open_at(
        p2_types::PathFlags::empty(),
        "p2-after-close",
        p2_types::OpenFlags::CREATE,
        p2_types::DescriptorFlags::READ,
    )
    .is_ok()
}

pub(crate) async fn run_p3_object_quota() -> Vec<String> {
    let (root, _) = p3_preopens::get_directories()
        .into_iter()
        .next()
        .expect("no P3 preopened directory");
    let held = root
        .open_at(
            p3_types::PathFlags::empty(),
            "p3-held".to_string(),
            p3_types::OpenFlags::CREATE,
            p3_types::DescriptorFlags::READ,
        )
        .await
        .expect("create P3 held object");
    root.link_at(
        p3_types::PathFlags::empty(),
        "p3-held".to_string(),
        &root,
        "p3-held-link".to_string(),
    )
    .await
    .expect("hard link must not consume an object");
    let hard_link_same_inode = held.stat().await.is_ok_and(|stat| stat.link_count == 2);
    let mut created = 0;
    for index in 0..64 {
        match root
            .open_at(
                p3_types::PathFlags::empty(),
                format!("p3-object-{index}"),
                p3_types::OpenFlags::CREATE,
                p3_types::DescriptorFlags::READ,
            )
            .await
        {
            Ok(file) => {
                created += 1;
                drop(file);
            }
            Err(p3_types::ErrorCode::Quota) => break,
            Err(error) => panic!("unexpected P3 object creation error: {error:?}"),
        }
    }
    let object_denied = created < 64;
    let directory_denied = matches!(
        root.create_directory_at("p3-object-directory".to_string())
            .await,
        Err(p3_types::ErrorCode::Quota)
    );
    let symlink_denied = matches!(
        root.symlink_at("p3-object-0".to_string(), "p3-object-symlink".to_string())
            .await,
        Err(p3_types::ErrorCode::Quota)
    );
    root.unlink_file_at("p3-held".to_string())
        .await
        .expect("unlink P3 held name");
    root.unlink_file_at("p3-held-link".to_string())
        .await
        .expect("unlink P3 held link");
    let denied_while_open = matches!(
        root.open_at(
            p3_types::PathFlags::empty(),
            "p3-before-close".to_string(),
            p3_types::OpenFlags::CREATE,
            p3_types::DescriptorFlags::READ,
        )
        .await,
        Err(p3_types::ErrorCode::Quota)
    );
    drop(held);
    vec![
        format!("hard-link-same-inode={hard_link_same_inode}"),
        format!("object-denied={object_denied}"),
        format!("directory-denied={directory_denied}"),
        format!("symlink-denied={symlink_denied}"),
        format!("denied-while-open={denied_while_open}"),
    ]
}

pub(crate) async fn complete_p3_object_quota_release() -> bool {
    let (root, _) = p3_preopens::get_directories()
        .into_iter()
        .next()
        .expect("no P3 preopened directory");
    root.open_at(
        p3_types::PathFlags::empty(),
        "p3-after-close".to_string(),
        p3_types::OpenFlags::CREATE,
        p3_types::DescriptorFlags::READ,
    )
    .await
    .is_ok()
}
