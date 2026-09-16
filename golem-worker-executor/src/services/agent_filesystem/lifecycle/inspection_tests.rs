use super::*;
use golem_common::model::filesystem::{
    FileByteSelection, FileReadError, FileReadExtent, FileReadHead,
};
use test_r::{test, timeout};

pub(super) async fn native_resident(parent: &Path) -> (ResidentFilesystem, PathBuf) {
    let id = agent_id();
    let root = parent
        .join(id.environment_id.to_string())
        .join(id.agent_id.component_id.to_string())
        .join(id.agent_id.agent_name_encoded());
    let created = create_fresh(
        sandbox_provisioning(&FilesystemStorageConfig {
            deterministic_root_dir: Some(parent.to_path_buf()),
            ..FilesystemStorageConfig::default()
        })
        .unwrap(),
        id,
        ResolvedStorageLimits::Unlimited,
    )
    .await
    .unwrap();
    let (account, _) = account();
    let reconstructing = bind_configured_resource_usage_metering(
        created,
        account,
        ResourceUsageMeteringConfig {
            compute: false,
            memory: false,
            filesystem: false,
        },
    )
    .unwrap();
    let reconstructing = materialize_initial_files(reconstructing, PreparedInitialFiles::empty())
        .await
        .unwrap();
    let reconstructing = finish_replay(reconstructing).await.unwrap();
    (finish_reconstruction(reconstructing).await.unwrap(), root)
}

#[test]
#[timeout("30s")]
async fn inspection_descriptor_metadata_and_selected_bytes() {
    let parent = tempfile::tempdir().unwrap();
    let (resident, root) = native_resident(parent.path()).await;
    std::fs::create_dir_all(root.join("public/sub")).unwrap();
    std::fs::write(root.join("public/sub/%2e%2e"), b"abcdefghijk").unwrap();
    let handle = resident_generation_handle(&resident);
    let FileInspection::Opened { file, metadata } = open_file_for_inspection(
        &handle,
        "/public/sub/%2e%2e",
        FileByteSelection::Bounded {
            start: 3,
            end_inclusive: 6,
        },
    )
    .await
    .unwrap() else {
        panic!("regular target must open");
    };
    assert_eq!(metadata.total_size, 11);
    assert_eq!(
        metadata.selection,
        FileReadExtent::Selected {
            offset: 3,
            length: 4
        }
    );
    // Replacing the name cannot redirect the descriptor to the replacement's bytes/metadata.
    std::fs::rename(root.join("public/sub/%2e%2e"), root.join("old")).unwrap();
    std::fs::write(root.join("public/sub/%2e%2e"), b"WRONG").unwrap();
    let bytes = read_file(
        &handle,
        &file,
        ReadRange {
            offset: 3,
            length: 4,
        },
    )
    .unwrap()
    .await
    .unwrap();
    assert_eq!(bytes.as_ref(), b"defg");
    close(OpenNode::File(file)).await.unwrap();
    delete(seal(resident)).await.unwrap();
}

#[test]
#[timeout("30s")]
#[cfg(unix)]
async fn inspection_owned_corpus_rejects_symlinks_and_implicit_index() {
    let corpus: serde_json::Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../golem-service-base/tests/fixtures/http-handlers/corpus.json"
    )))
    .unwrap();
    for (id, expected) in [
        ("file-no-final-symlink", FileReadHead::Symlink),
        ("file-no-parent-symlink", FileReadHead::Symlink),
        ("file-no-implicit-index", FileReadHead::NotRegular),
    ] {
        let case = corpus["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|case| case["id"] == id)
            .unwrap();
        let parent = tempfile::tempdir().unwrap();
        let (resident, root) = native_resident(parent.path()).await;
        for (path, entry) in case["input"]["filesystem"].as_object().unwrap() {
            let path = root.join(path.strip_prefix('/').unwrap());
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            if let Some(target) = entry["symlink"].as_str() {
                std::os::unix::fs::symlink(root.join(target.strip_prefix('/').unwrap()), path)
                    .unwrap();
            } else if entry["directory"] == true {
                std::fs::create_dir_all(path).unwrap();
            } else {
                std::fs::write(
                    path,
                    hex::decode(entry["body_hex"].as_str().unwrap()).unwrap(),
                )
                .unwrap();
            }
        }
        let result = open_file_for_inspection(
            &resident_generation_handle(&resident),
            case["input"]["path"].as_str().unwrap(),
            FileByteSelection::Full,
        )
        .await
        .unwrap();
        let FileInspection::Rejected(head) = result else {
            panic!("{id}: must reject");
        };
        assert_eq!(head, expected, "{id}");
        delete(seal(resident)).await.unwrap();
    }
}

#[test]
#[timeout("30s")]
#[cfg(target_os = "linux")]
async fn inspection_root_symlink_fifo_and_directories() {
    let parent = tempfile::tempdir().unwrap();
    let (resident, root) = native_resident(parent.path()).await;
    std::fs::create_dir(root.join("public")).unwrap();
    std::fs::write(root.join("public/file"), b"abc").unwrap();
    std::os::unix::fs::symlink(root.join("public"), root.join("alias")).unwrap();
    std::os::unix::fs::symlink("file", root.join("public/link-to-file")).unwrap();
    std::os::unix::fs::symlink(".", root.join("public/link-dir")).unwrap();
    let fifo =
        std::ffi::CString::new(root.join("public/fifo").as_os_str().as_encoded_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
    let handle = resident_generation_handle(&resident);
    for (path, expected) in [
        ("/alias/file", FileReadHead::Symlink),
        ("/public/link-to-file", FileReadHead::Symlink),
        ("/public/link-dir/file", FileReadHead::Symlink),
        ("/public/fifo", FileReadHead::NotRegular),
        ("/public/fifo/child", FileReadHead::NotRegular),
        ("/public", FileReadHead::NotRegular),
        ("/public/missing", FileReadHead::Absent),
        ("/", FileReadHead::NotRegular),
    ] {
        let FileInspection::Rejected(head) =
            open_file_for_inspection(&handle, path, FileByteSelection::Full)
                .await
                .unwrap()
        else {
            panic!("must reject {path}");
        };
        assert_eq!(head, expected, "{path}");
    }
    delete(seal(resident)).await.unwrap();
}

#[test]
#[timeout("30s")]
async fn inspection_revalidates_before_io_and_sanitizes_permission_failures() {
    let (filesystem, control, _) = resident(Err(unsupported_allocation())).await;
    let handle = resident_generation_handle(&filesystem);
    let before = control.calls();
    assert!(matches!(
        open_file_for_inspection(&handle, "/public/../private", FileByteSelection::Full).await,
        Err(FileReadError::InvalidTarget)
    ));
    assert_eq!(control.calls(), before);
    control.push_open(Err(sandbox_error(
        "secret host path",
        std::io::ErrorKind::PermissionDenied,
    )));
    assert!(matches!(
        open_file_for_inspection(&handle, "/file", FileByteSelection::Full)
            .await
            .unwrap(),
        FileInspection::Rejected(FileReadHead::PermissionDenied)
    ));
    assert!(!admit(&handle).unwrap().registry.is_invalidated());
    // Once open succeeds, a metadata failure must never turn into a target miss.
    for kind in [std::io::ErrorKind::NotFound, std::io::ErrorKind::Other] {
        control.push_open(Ok(SandboxOpened::scripted_file(42)));
        control.push_get_attributes(Err(sandbox_error("secret host path", kind)));
        control.push_close(Ok(()));
        assert!(matches!(
            open_file_for_inspection(&handle, "/file", FileByteSelection::Full).await,
            Err(FileReadError::Storage)
        ));
    }
    control.push_delete_and_verify(Ok(()));
    let sealed = seal(filesystem);
    assert!(matches!(
        open_file_for_inspection(&handle, "/", FileByteSelection::Full).await,
        Err(FileReadError::Lifecycle)
    ));
    delete(sealed).await.unwrap();
}

#[test]
#[timeout("30s")]
async fn inspection_walk_uses_single_component_descriptors_and_open_metadata() {
    let (filesystem, control, _) = resident(Err(unsupported_allocation())).await;
    let handle = resident_generation_handle(&filesystem);
    for id in [10, 11] {
        control.push_open(Ok(SandboxOpened::scripted_directory(id)));
        control.push_close(Ok(()));
    }
    control.push_open(Ok(SandboxOpened::scripted_file(12)));
    control.push_close(Ok(()));
    let mut attrs = sandbox_attributes(SandboxObjectKind::File);
    attrs.size = u64::MAX;
    control.push_get_attributes(Ok(attrs));
    let before = control.calls().len();
    let FileInspection::Opened { file, metadata } = open_file_for_inspection(
        &handle,
        "/public/nested/leaf",
        FileByteSelection::Suffix { length: 3 },
    )
    .await
    .unwrap() else {
        panic!("expected scripted file");
    };
    assert_eq!(metadata.total_size, u64::MAX);
    assert_eq!(
        metadata.selection,
        FileReadExtent::Selected {
            offset: u64::MAX - 3,
            length: 3
        }
    );
    let calls = control.calls();
    let calls = &calls[before..];
    assert_eq!(
        calls
            .iter()
            .filter(|call| call.starts_with("open("))
            .count(),
        3
    );
    assert!(
        calls
            .iter()
            .filter(|call| call.starts_with("open("))
            .all(|call| call.contains("Inspection"))
    );
    assert!(
        calls
            .iter()
            .any(|call| call.starts_with("get_node_attributes("))
    );
    assert!(
        !calls
            .iter()
            .any(|call| call.starts_with("get_path_attributes(") || call.starts_with("read("))
    );
    close(OpenNode::File(file)).await.unwrap();
    control.push_delete_and_verify(Ok(()));
    delete(seal(filesystem)).await.unwrap();
}
