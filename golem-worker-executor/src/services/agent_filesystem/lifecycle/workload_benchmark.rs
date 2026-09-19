use super::*;
use crate::services::active_agents::{ConcurrentAgentsScheduler, MemoryGrant};
use crate::services::golem_config::ResourceUsageMeteringConfig;
use crate::services::linear_memory::LinearMemoryTracker;
use crate::services::resource_limits::AtomicResourceEntry;
use crate::services::resource_usage_metering::close_window;
use golem_common::model::account::AccountId;
use golem_common::model::agent::AgentMode;
use golem_common::model::component::ComponentId;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::{AgentId, RetryConfig};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicUsize;
use std::time::{Duration, Instant};
use test_r::{test, timeout};
use tokio::sync::Barrier;
use tokio::task::JoinSet;
use uuid::Uuid;

const STORAGE_LIMIT_BYTES: u64 = 128 * 1024 * 1024;
const STORAGE_LIMIT_OBJECTS: u64 = 32_768;

struct BenchmarkFilesystem {
    resident: ResidentFilesystem,
    window: ResourceUsageMeteringWindow,
    generation_handle: FilesystemGenerationHandle,
}

#[derive(Clone)]
struct WorkloadPayloads {
    package_json: Bytes,
    javascript: Bytes,
    small_file: Bytes,
    object: Bytes,
    pack: Bytes,
}

#[derive(Clone, Copy)]
struct WorkloadSpec {
    name: &'static str,
    package_count: usize,
    object_count: usize,
    checkout_file_count: usize,
    object_fanout: usize,
    regular_files: usize,
    directories: usize,
    symlinks: usize,
    bytes_written: usize,
    filesystem_operations: usize,
}

impl WorkloadSpec {
    fn npm_like(quick: bool) -> Self {
        let package_count = if quick { 12 } else { 128 };
        Self {
            name: "npm-like",
            package_count,
            object_count: 0,
            checkout_file_count: 0,
            object_fanout: 0,
            regular_files: 3 * package_count + 3,
            directories: 2 * package_count + 4,
            symlinks: 1,
            bytes_written: 2_816 * package_count + 6_400,
            filesystem_operations: 17 * package_count + 19,
        }
    }

    fn git_clone_like(quick: bool) -> Self {
        let (object_count, checkout_file_count, object_fanout) =
            if quick { (24, 24, 4) } else { (256, 256, 16) };
        Self {
            name: "git-clone-like",
            package_count: 0,
            object_count,
            checkout_file_count,
            object_fanout,
            regular_files: object_count + checkout_file_count + 6,
            directories: object_fanout + 12,
            symlinks: 0,
            bytes_written: 4_096 * object_count + 2_048 * checkout_file_count + 75_008,
            filesystem_operations: 5 * object_count
                + 4 * checkout_file_count
                + 3 * checkout_file_count.div_ceil(6)
                + object_fanout
                + 43,
        }
    }
}

impl WorkloadPayloads {
    fn new() -> Self {
        Self {
            package_json: Bytes::from(vec![b'p'; 256]),
            javascript: Bytes::from(vec![b'j'; 2 * 1024]),
            small_file: Bytes::from(vec![b's'; 512]),
            object: Bytes::from(vec![b'o'; 4 * 1024]),
            pack: Bytes::from(vec![b'k'; 64 * 1024]),
        }
    }
}

fn benchmark_agent_id(index: usize) -> OwnedAgentId {
    OwnedAgentId::new(
        EnvironmentId::new(),
        &AgentId::from_agent_name_string(
            ComponentId::new(),
            format!("filesystem-workload-benchmark-{index}"),
        )
        .unwrap(),
    )
}

fn benchmark_account() -> (ResourceUsageAccount, Arc<AtomicResourceEntry>) {
    let entry = Arc::new(AtomicResourceEntry::new(0, 0, 0, 0, 1));
    let memory = LinearMemoryTracker::new_with_metering(
        0,
        0,
        AgentMode::Durable,
        false,
        entry.clone(),
        Arc::new(Mutex::new(MemoryGrant::inert(0))),
        false,
    );
    (
        ResourceUsageAccount::new(AgentMode::Durable, memory, entry.clone()),
        entry,
    )
}

async fn benchmark_permit(
    entry: &Arc<AtomicResourceEntry>,
    agent: &OwnedAgentId,
) -> ConcurrentAgentPermit {
    let scheduler = Arc::new(ConcurrentAgentsScheduler::new());
    let account_id = AccountId(Uuid::new_v4());
    scheduler.register_account(account_id, entry.clone()).await;
    scheduler.acquire(account_id, agent.agent_id.clone()).await
}

async fn create_benchmark_filesystem(
    provisioning: SandboxFilesystemProvisioning,
    agent: OwnedAgentId,
    limits: ResolvedStorageLimits,
    filesystem_metering: bool,
) -> BenchmarkFilesystem {
    let created = create_fresh(provisioning, agent.clone(), limits)
        .await
        .unwrap();
    let (account, entry) = benchmark_account();
    let reconstructing = bind_configured_resource_usage_metering(
        created,
        account,
        ResourceUsageMeteringConfig {
            compute: false,
            memory: false,
            filesystem: filesystem_metering,
        },
    )
    .unwrap();
    let reconstructing = materialize_initial_files(reconstructing, PreparedInitialFiles::empty())
        .await
        .unwrap();
    let reconstructing = finish_replay(reconstructing).await.unwrap();
    let resident = finish_reconstruction(reconstructing).await.unwrap();
    let window = open_resource_usage_window(&resident, benchmark_permit(&entry, &agent).await)
        .await
        .unwrap();
    let generation_handle = resident_generation_handle(&resident);
    BenchmarkFilesystem {
        resident,
        window,
        generation_handle,
    }
}

async fn create_directory(
    generation_handle: &FilesystemGenerationHandle,
    path: impl Into<PathBuf>,
) {
    edit_namespace(
        generation_handle,
        NamespaceEdit::Insert {
            destination: PathTarget::at_root(generation_handle, path).unwrap(),
            object: NewObject::Directory,
        },
    )
    .unwrap()
    .await
    .unwrap();
}

async fn create_file(
    generation_handle: &FilesystemGenerationHandle,
    path: impl Into<PathBuf>,
    bytes: Bytes,
) {
    let opened = open(
        generation_handle,
        PathTarget::at_root(generation_handle, path).unwrap(),
        OpenOptions::File {
            access: AccessMode::ReadWrite,
            disposition: FileDisposition::CreateExclusive,
            follow: Follow::Yes,
        },
    )
    .unwrap()
    .await
    .unwrap();
    let OpenNode::File(file) = opened.node else {
        panic!("new benchmark file opened as a directory")
    };
    let expected = bytes.len() as u64;
    assert_eq!(
        write(generation_handle, &file, WritePlacement::At(0), bytes)
            .unwrap()
            .await
            .unwrap()
            .written,
        expected
    );
    close(OpenNode::File(file)).await.unwrap();
}

async fn read_file_exact(
    generation_handle: &FilesystemGenerationHandle,
    path: impl Into<PathBuf>,
    expected: &Bytes,
) {
    let opened = open(
        generation_handle,
        PathTarget::at_root(generation_handle, path).unwrap(),
        OpenOptions::Existing {
            expected: ObjectKind::File,
            access: AccessMode::Read,
            follow: Follow::Yes,
        },
    )
    .unwrap()
    .await
    .unwrap();
    let OpenNode::File(file) = opened.node else {
        panic!("benchmark file opened as a directory")
    };
    let actual = read_file(
        generation_handle,
        &file,
        ReadRange {
            offset: 0,
            length: expected.len(),
        },
    )
    .unwrap()
    .await
    .unwrap();
    assert_eq!(&actual, expected);
    close(OpenNode::File(file)).await.unwrap();
}

async fn assert_file_metadata(
    generation_handle: &FilesystemGenerationHandle,
    path: impl Into<PathBuf>,
    size: usize,
) {
    let target = PathTarget::at_root(generation_handle, path).unwrap();
    let result = attributes(generation_handle, Target::Path(&target, Follow::Yes))
        .unwrap()
        .await
        .unwrap();
    assert_eq!(result.kind, ObjectKind::File);
    assert_eq!(result.size, size as u64);
}

async fn assert_missing(generation_handle: &FilesystemGenerationHandle, path: impl Into<PathBuf>) {
    let target = PathTarget::at_root(generation_handle, path).unwrap();
    let error = attributes(generation_handle, Target::Path(&target, Follow::Yes))
        .unwrap()
        .await
        .expect_err("benchmark path unexpectedly exists");
    let Error::Sandbox(source) = error else {
        panic!("benchmark path lookup failed for a reason other than absence: {error}")
    };
    assert_eq!(source.io_kind(), Some(std::io::ErrorKind::NotFound));
}

async fn rename(
    generation_handle: &FilesystemGenerationHandle,
    source: impl Into<PathBuf>,
    destination: impl Into<PathBuf>,
) {
    edit_namespace(
        generation_handle,
        NamespaceEdit::Move {
            source: PathTarget::at_root(generation_handle, source).unwrap(),
            destination: PathTarget::at_root(generation_handle, destination).unwrap(),
        },
    )
    .unwrap()
    .await
    .unwrap();
}

async fn list_directory_exact(
    generation_handle: &FilesystemGenerationHandle,
    path: impl Into<PathBuf>,
    count: usize,
) {
    let opened = open(
        generation_handle,
        PathTarget::at_root(generation_handle, path).unwrap(),
        OpenOptions::Existing {
            expected: ObjectKind::Directory,
            access: AccessMode::Read,
            follow: Follow::Yes,
        },
    )
    .unwrap()
    .await
    .unwrap();
    let OpenNode::Directory(directory) = opened.node else {
        panic!("benchmark directory opened as a file")
    };
    assert_eq!(
        list_directory(generation_handle, &directory)
            .unwrap()
            .await
            .unwrap()
            .len(),
        count
    );
    close(OpenNode::Directory(directory)).await.unwrap();
}

async fn remove(
    generation_handle: &FilesystemGenerationHandle,
    path: impl Into<PathBuf>,
    expected: ObjectKind,
) {
    edit_namespace(
        generation_handle,
        NamespaceEdit::Remove {
            target: PathTarget::at_root(generation_handle, path).unwrap(),
            expected,
        },
    )
    .unwrap()
    .await
    .unwrap();
}

async fn run_npm_like(
    generation_handle: &FilesystemGenerationHandle,
    root: &Path,
    payloads: &WorkloadPayloads,
    package_count: usize,
) {
    create_directory(generation_handle, root).await;
    create_directory(generation_handle, root.join("node_modules")).await;
    create_directory(generation_handle, root.join("node_modules/.bin")).await;
    create_directory(generation_handle, root.join(".npm-cache")).await;
    create_file(
        generation_handle,
        root.join("package.json"),
        payloads.package_json.clone(),
    )
    .await;
    create_file(
        generation_handle,
        root.join("package-lock.json.tmp"),
        payloads.javascript.clone(),
    )
    .await;

    for package in 0..package_count {
        let package_root = root.join(format!("node_modules/package-{package:02}"));
        create_directory(generation_handle, &package_root).await;
        create_directory(generation_handle, package_root.join("lib")).await;
        create_file(
            generation_handle,
            package_root.join("package.json"),
            payloads.package_json.clone(),
        )
        .await;
        create_file(
            generation_handle,
            package_root.join("index.js"),
            payloads.javascript.clone(),
        )
        .await;
        create_file(
            generation_handle,
            package_root.join("lib/helper.js"),
            payloads.small_file.clone(),
        )
        .await;
        assert_file_metadata(
            generation_handle,
            package_root.join("package.json"),
            payloads.package_json.len(),
        )
        .await;
        assert_missing(generation_handle, package_root.join("index.json")).await;
        assert_missing(generation_handle, package_root.join("index.node")).await;
        read_file_exact(
            generation_handle,
            package_root.join("package.json"),
            &payloads.package_json,
        )
        .await;
    }

    create_file(
        generation_handle,
        root.join(".npm-cache/package.tgz"),
        payloads.object.clone(),
    )
    .await;
    edit_namespace(
        generation_handle,
        NamespaceEdit::Insert {
            destination: PathTarget::at_root(
                generation_handle,
                root.join("node_modules/.bin/package-00"),
            )
            .unwrap(),
            object: NewObject::Symlink(SymlinkTarget(PathBuf::from("../package-00/index.js"))),
        },
    )
    .unwrap()
    .await
    .unwrap();
    rename(
        generation_handle,
        root.join("package-lock.json.tmp"),
        root.join("package-lock.json"),
    )
    .await;
    list_directory_exact(
        generation_handle,
        root.join("node_modules"),
        package_count + 1,
    )
    .await;
    assert_file_metadata(
        generation_handle,
        root.join("package-lock.json"),
        payloads.javascript.len(),
    )
    .await;
}

async fn clean_npm_like(
    generation_handle: &FilesystemGenerationHandle,
    root: &Path,
    package_count: usize,
) {
    remove(
        generation_handle,
        root.join("node_modules/.bin/package-00"),
        ObjectKind::Symlink,
    )
    .await;
    remove(
        generation_handle,
        root.join(".npm-cache/package.tgz"),
        ObjectKind::File,
    )
    .await;
    remove(
        generation_handle,
        root.join("package-lock.json"),
        ObjectKind::File,
    )
    .await;
    remove(
        generation_handle,
        root.join("package.json"),
        ObjectKind::File,
    )
    .await;
    for package in (0..package_count).rev() {
        let package_root = root.join(format!("node_modules/package-{package:02}"));
        remove(
            generation_handle,
            package_root.join("lib/helper.js"),
            ObjectKind::File,
        )
        .await;
        remove(
            generation_handle,
            package_root.join("index.js"),
            ObjectKind::File,
        )
        .await;
        remove(
            generation_handle,
            package_root.join("package.json"),
            ObjectKind::File,
        )
        .await;
        remove(
            generation_handle,
            package_root.join("lib"),
            ObjectKind::Directory,
        )
        .await;
        remove(generation_handle, package_root, ObjectKind::Directory).await;
    }
    remove(
        generation_handle,
        root.join("node_modules/.bin"),
        ObjectKind::Directory,
    )
    .await;
    remove(
        generation_handle,
        root.join("node_modules"),
        ObjectKind::Directory,
    )
    .await;
    remove(
        generation_handle,
        root.join(".npm-cache"),
        ObjectKind::Directory,
    )
    .await;
    remove(generation_handle, root, ObjectKind::Directory).await;
}

async fn run_git_clone_like(
    generation_handle: &FilesystemGenerationHandle,
    root: &Path,
    payloads: &WorkloadPayloads,
    object_count: usize,
    checkout_file_count: usize,
    object_fanout: usize,
) {
    for directory in [
        root.to_path_buf(),
        root.join(".git"),
        root.join(".git/objects"),
        root.join(".git/objects/pack"),
        root.join(".git/refs"),
        root.join(".git/refs/heads"),
        root.join(".git/logs"),
        root.join(".git/logs/refs"),
        root.join(".git/logs/refs/heads"),
        root.join("src"),
        root.join("tests"),
        root.join("docs"),
    ] {
        create_directory(generation_handle, directory).await;
    }

    create_file(
        generation_handle,
        root.join(".git/HEAD"),
        payloads.small_file.clone(),
    )
    .await;
    create_file(
        generation_handle,
        root.join(".git/config"),
        payloads.package_json.clone(),
    )
    .await;
    create_file(
        generation_handle,
        root.join(".git/refs/heads/main.lock"),
        payloads.small_file.clone(),
    )
    .await;
    rename(
        generation_handle,
        root.join(".git/refs/heads/main.lock"),
        root.join(".git/refs/heads/main"),
    )
    .await;

    for fanout in 0..object_fanout {
        create_directory(
            generation_handle,
            root.join(format!(".git/objects/{fanout:02x}")),
        )
        .await;
    }
    for object in 0..object_count {
        let fanout = object % object_fanout;
        let final_path = root.join(format!(".git/objects/{fanout:02x}/object-{object:02}"));
        let temporary_path = root.join(format!(".git/objects/{fanout:02x}/tmp-{object:02}"));
        create_file(generation_handle, &temporary_path, payloads.object.clone()).await;
        rename(generation_handle, temporary_path, &final_path).await;
        assert_file_metadata(generation_handle, final_path, payloads.object.len()).await;
    }

    create_file(
        generation_handle,
        root.join(".git/objects/pack/pack-main.pack.tmp"),
        payloads.pack.clone(),
    )
    .await;
    rename(
        generation_handle,
        root.join(".git/objects/pack/pack-main.pack.tmp"),
        root.join(".git/objects/pack/pack-main.pack"),
    )
    .await;
    create_file(
        generation_handle,
        root.join(".git/objects/pack/pack-main.idx.tmp"),
        payloads.object.clone(),
    )
    .await;
    rename(
        generation_handle,
        root.join(".git/objects/pack/pack-main.idx.tmp"),
        root.join(".git/objects/pack/pack-main.idx"),
    )
    .await;

    for file in 0..checkout_file_count {
        let directory = match file % 4 {
            0 => "src",
            1 => "tests",
            2 => "docs",
            _ => "",
        };
        let path = if directory.is_empty() {
            root.join(format!("file-{file:02}.txt"))
        } else {
            root.join(directory).join(format!("file-{file:02}.txt"))
        };
        create_file(generation_handle, &path, payloads.javascript.clone()).await;
        assert_file_metadata(generation_handle, &path, payloads.javascript.len()).await;
        if file % 6 == 0 {
            read_file_exact(generation_handle, path, &payloads.javascript).await;
        }
    }

    create_file(
        generation_handle,
        root.join(".git/index.lock"),
        payloads.object.clone(),
    )
    .await;
    rename(
        generation_handle,
        root.join(".git/index.lock"),
        root.join(".git/index"),
    )
    .await;
    read_file_exact(
        generation_handle,
        root.join(".git/config"),
        &payloads.package_json,
    )
    .await;
    list_directory_exact(
        generation_handle,
        root.join(".git/objects"),
        object_fanout + 1,
    )
    .await;
    list_directory_exact(generation_handle, root, checkout_file_count.div_ceil(4) + 4).await;
}

async fn clean_git_clone_like(
    generation_handle: &FilesystemGenerationHandle,
    root: &Path,
    object_count: usize,
    checkout_file_count: usize,
    object_fanout: usize,
) {
    for file in (0..checkout_file_count).rev() {
        let directory = match file % 4 {
            0 => "src",
            1 => "tests",
            2 => "docs",
            _ => "",
        };
        let path = if directory.is_empty() {
            root.join(format!("file-{file:02}.txt"))
        } else {
            root.join(directory).join(format!("file-{file:02}.txt"))
        };
        remove(generation_handle, path, ObjectKind::File).await;
    }
    for path in [
        root.join(".git/index"),
        root.join(".git/objects/pack/pack-main.idx"),
        root.join(".git/objects/pack/pack-main.pack"),
        root.join(".git/refs/heads/main"),
        root.join(".git/config"),
        root.join(".git/HEAD"),
    ] {
        remove(generation_handle, path, ObjectKind::File).await;
    }
    for object in (0..object_count).rev() {
        let fanout = object % object_fanout;
        remove(
            generation_handle,
            root.join(format!(".git/objects/{fanout:02x}/object-{object:02}")),
            ObjectKind::File,
        )
        .await;
    }
    for fanout in (0..object_fanout).rev() {
        remove(
            generation_handle,
            root.join(format!(".git/objects/{fanout:02x}")),
            ObjectKind::Directory,
        )
        .await;
    }
    for path in [
        root.join(".git/objects/pack"),
        root.join(".git/objects"),
        root.join(".git/refs/heads"),
        root.join(".git/refs"),
        root.join(".git/logs/refs/heads"),
        root.join(".git/logs/refs"),
        root.join(".git/logs"),
        root.join(".git"),
        root.join("src"),
        root.join("tests"),
        root.join("docs"),
        root.to_path_buf(),
    ] {
        remove(generation_handle, path, ObjectKind::Directory).await;
    }
}

fn run_sandbox_npm_like(root: &Path, payloads: &WorkloadPayloads, package_count: usize) {
    std::fs::create_dir(root).unwrap();
    std::fs::create_dir(root.join("node_modules")).unwrap();
    std::fs::create_dir(root.join("node_modules/.bin")).unwrap();
    std::fs::create_dir(root.join(".npm-cache")).unwrap();
    std::fs::write(root.join("package.json"), payloads.package_json.as_ref()).unwrap();
    std::fs::write(
        root.join("package-lock.json.tmp"),
        payloads.javascript.as_ref(),
    )
    .unwrap();

    for package in 0..package_count {
        let package_root = root.join(format!("node_modules/package-{package:02}"));
        std::fs::create_dir(&package_root).unwrap();
        std::fs::create_dir(package_root.join("lib")).unwrap();
        std::fs::write(
            package_root.join("package.json"),
            payloads.package_json.as_ref(),
        )
        .unwrap();
        std::fs::write(package_root.join("index.js"), payloads.javascript.as_ref()).unwrap();
        std::fs::write(
            package_root.join("lib/helper.js"),
            payloads.small_file.as_ref(),
        )
        .unwrap();
        let metadata = std::fs::metadata(package_root.join("package.json")).unwrap();
        assert_eq!(metadata.len(), payloads.package_json.len() as u64);
        for missing in ["index.json", "index.node"] {
            assert_eq!(
                std::fs::metadata(package_root.join(missing))
                    .unwrap_err()
                    .kind(),
                std::io::ErrorKind::NotFound
            );
        }
        assert_eq!(
            std::fs::read(package_root.join("package.json")).unwrap(),
            payloads.package_json.as_ref()
        );
    }

    std::fs::write(
        root.join(".npm-cache/package.tgz"),
        payloads.object.as_ref(),
    )
    .unwrap();
    std::os::unix::fs::symlink(
        "../package-00/index.js",
        root.join("node_modules/.bin/package-00"),
    )
    .unwrap();
    std::fs::rename(
        root.join("package-lock.json.tmp"),
        root.join("package-lock.json"),
    )
    .unwrap();
    assert_eq!(
        std::fs::read_dir(root.join("node_modules"))
            .unwrap()
            .count(),
        package_count + 1
    );
    assert_eq!(
        std::fs::metadata(root.join("package-lock.json"))
            .unwrap()
            .len(),
        payloads.javascript.len() as u64
    );
}

fn run_sandbox_git_clone_like(
    root: &Path,
    payloads: &WorkloadPayloads,
    object_count: usize,
    checkout_file_count: usize,
    object_fanout: usize,
) {
    for directory in [
        root.to_path_buf(),
        root.join(".git"),
        root.join(".git/objects"),
        root.join(".git/objects/pack"),
        root.join(".git/refs"),
        root.join(".git/refs/heads"),
        root.join(".git/logs"),
        root.join(".git/logs/refs"),
        root.join(".git/logs/refs/heads"),
        root.join("src"),
        root.join("tests"),
        root.join("docs"),
    ] {
        std::fs::create_dir(directory).unwrap();
    }

    std::fs::write(root.join(".git/HEAD"), payloads.small_file.as_ref()).unwrap();
    std::fs::write(root.join(".git/config"), payloads.package_json.as_ref()).unwrap();
    std::fs::write(
        root.join(".git/refs/heads/main.lock"),
        payloads.small_file.as_ref(),
    )
    .unwrap();
    std::fs::rename(
        root.join(".git/refs/heads/main.lock"),
        root.join(".git/refs/heads/main"),
    )
    .unwrap();

    for fanout in 0..object_fanout {
        std::fs::create_dir(root.join(format!(".git/objects/{fanout:02x}"))).unwrap();
    }
    for object in 0..object_count {
        let fanout = object % object_fanout;
        let final_path = root.join(format!(".git/objects/{fanout:02x}/object-{object:02}"));
        let temporary_path = root.join(format!(".git/objects/{fanout:02x}/tmp-{object:02}"));
        std::fs::write(&temporary_path, payloads.object.as_ref()).unwrap();
        std::fs::rename(temporary_path, &final_path).unwrap();
        assert_eq!(
            std::fs::metadata(final_path).unwrap().len(),
            payloads.object.len() as u64
        );
    }

    std::fs::write(
        root.join(".git/objects/pack/pack-main.pack.tmp"),
        payloads.pack.as_ref(),
    )
    .unwrap();
    std::fs::rename(
        root.join(".git/objects/pack/pack-main.pack.tmp"),
        root.join(".git/objects/pack/pack-main.pack"),
    )
    .unwrap();
    std::fs::write(
        root.join(".git/objects/pack/pack-main.idx.tmp"),
        payloads.object.as_ref(),
    )
    .unwrap();
    std::fs::rename(
        root.join(".git/objects/pack/pack-main.idx.tmp"),
        root.join(".git/objects/pack/pack-main.idx"),
    )
    .unwrap();

    for file in 0..checkout_file_count {
        let directory = match file % 4 {
            0 => "src",
            1 => "tests",
            2 => "docs",
            _ => "",
        };
        let path = if directory.is_empty() {
            root.join(format!("file-{file:02}.txt"))
        } else {
            root.join(directory).join(format!("file-{file:02}.txt"))
        };
        std::fs::write(&path, payloads.javascript.as_ref()).unwrap();
        assert_eq!(
            std::fs::metadata(&path).unwrap().len(),
            payloads.javascript.len() as u64
        );
        if file % 6 == 0 {
            assert_eq!(std::fs::read(path).unwrap(), payloads.javascript.as_ref());
        }
    }

    std::fs::write(root.join(".git/index.lock"), payloads.object.as_ref()).unwrap();
    std::fs::rename(root.join(".git/index.lock"), root.join(".git/index")).unwrap();
    assert_eq!(
        std::fs::read(root.join(".git/config")).unwrap(),
        payloads.package_json.as_ref()
    );
    assert_eq!(
        std::fs::read_dir(root.join(".git/objects"))
            .unwrap()
            .count(),
        object_fanout + 1
    );
    assert_eq!(
        std::fs::read_dir(root).unwrap().count(),
        checkout_file_count.div_ceil(4) + 4
    );
}

fn percentile(sorted: &[u128], percent: usize) -> u128 {
    let rank = sorted.len().saturating_mul(percent).div_ceil(100);
    sorted[rank.saturating_sub(1).min(sorted.len() - 1)]
}

fn verify_managed_workload_gate(
    workload: &str,
    concurrency: usize,
    p50_ns: u128,
    p95_ns: u128,
) -> Result<(), String> {
    let limits = match (workload, concurrency) {
        ("npm-like", 1) => Some((Some(30_000_000), Some(40_000_000))),
        ("npm-like", 4) => Some((None, Some(217_255_442))),
        ("npm-like", 16) => Some((None, Some(479_872_717))),
        ("npm-like", 64) => Some((Some(700_000_000), Some(1_575_210_845))),
        ("git-clone-like", 1) => Some((Some(40_000_000), Some(50_000_000))),
        ("git-clone-like", 4) => Some((None, Some(287_786_406))),
        ("git-clone-like", 16) => Some((None, Some(577_242_017))),
        ("git-clone-like", 64) => Some((Some(1_000_000_000), Some(2_576_282_922))),
        _ => None,
    };
    let Some((p50_limit, p95_limit)) = limits else {
        return Ok(());
    };
    if let Some(p50_limit) = p50_limit
        && p50_ns > p50_limit
    {
        return Err(format!(
            "managed {workload} concurrency {concurrency} p50 {p50_ns} ns exceeds {p50_limit} ns"
        ));
    }
    if let Some(p95_limit) = p95_limit
        && p95_ns > p95_limit
    {
        return Err(format!(
            "managed {workload} concurrency {concurrency} p95 {p95_ns} ns exceeds {p95_limit} ns"
        ));
    }
    Ok(())
}

#[derive(Clone, Copy)]
struct BenchmarkIsolationControls {
    disable_root_capability_reuse: bool,
    disable_managed_xfs_name_mode_shortcut: bool,
    eager_append_coordination: bool,
}

impl BenchmarkIsolationControls {
    fn from_environment() -> Self {
        Self {
            disable_root_capability_reuse: std::env::var(
                "GOLEM_FILESYSTEM_BENCHMARK_DISABLE_ROOT_CAPABILITY_REUSE",
            )
            .as_deref()
                == Ok("1"),
            disable_managed_xfs_name_mode_shortcut: std::env::var(
                "GOLEM_FILESYSTEM_DISABLE_MANAGED_XFS_NAME_MODE_SHORTCUT",
            )
            .as_deref()
                == Ok("1"),
            eager_append_coordination: std::env::var(
                "GOLEM_FILESYSTEM_BENCH_EAGER_APPEND_COORDINATION",
            )
            .as_deref()
                == Ok("1"),
        }
    }

    fn uses_production_behavior(self) -> bool {
        !self.disable_root_capability_reuse
            && !self.disable_managed_xfs_name_mode_shortcut
            && !self.eager_append_coordination
    }

    fn record_fields(self) -> String {
        format!(
            "\"disable_root_capability_reuse\":{},\"disable_managed_xfs_name_mode_shortcut\":{},\"eager_append_coordination\":{}",
            self.disable_root_capability_reuse,
            self.disable_managed_xfs_name_mode_shortcut,
            self.eager_append_coordination,
        )
    }
}

fn verify_benchmark_gate(
    mode: &str,
    enforce_gates: bool,
    controls: BenchmarkIsolationControls,
    workload: &str,
    concurrency: usize,
    p50_ns: u128,
    p95_ns: u128,
) -> Result<(), String> {
    if mode == "managed" && enforce_gates && controls.uses_production_behavior() {
        verify_managed_workload_gate(workload, concurrency, p50_ns, p95_ns)
    } else {
        Ok(())
    }
}

fn report(
    mode: &str,
    workload: WorkloadSpec,
    concurrency: usize,
    mut samples: Vec<u128>,
    enforce_gates: bool,
) {
    samples.sort_unstable();
    let mean = samples.iter().sum::<u128>() / samples.len() as u128;
    let p50 = percentile(&samples, 50);
    let p95 = percentile(&samples, 95);
    let isolation_controls = BenchmarkIsolationControls::from_environment();
    verify_benchmark_gate(
        mode,
        enforce_gates,
        isolation_controls,
        workload.name,
        concurrency,
        p50,
        p95,
    )
    .unwrap_or_else(|error| panic!("filesystem workload gate failed: {error}"));
    let execution = std::env::var("GOLEM_FILESYSTEM_PROTOTYPE_EXECUTION")
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "current".to_string());
    let micro_optimizations =
        std::env::var("GOLEM_FILESYSTEM_PROTOTYPE_MICRO_OPTIMIZATIONS").as_deref() == Ok("1");
    let isolation_control_fields = isolation_controls.record_fields();
    println!(
        "FILESYSTEM_WORKLOAD_BENCHMARK {{\"mode\":\"{mode}\",\"execution\":\"{execution}\",\"micro_optimizations\":{micro_optimizations},{isolation_control_fields},\"workload\":\"{}\",\"concurrency\":{concurrency},\"samples\":{},\"regular_files\":{},\"directories\":{},\"symlinks\":{},\"bytes_written\":{},\"filesystem_operations\":{},\"mean_ns\":{mean},\"p50_ns\":{},\"p95_ns\":{},\"p99_ns\":{}}}",
        workload.name,
        samples.len(),
        workload.regular_files,
        workload.directories,
        workload.symlinks,
        workload.bytes_written,
        workload.filesystem_operations,
        p50,
        p95,
        percentile(&samples, 99),
    );
}

#[test]
fn production_benchmark_records_enforce_thresholds() {
    let controls = BenchmarkIsolationControls {
        disable_root_capability_reuse: false,
        disable_managed_xfs_name_mode_shortcut: false,
        eager_append_coordination: false,
    };
    assert!(
        verify_benchmark_gate(
            "managed", true, controls, "npm-like", 1, 30_000_001, 40_000_000,
        )
        .is_err()
    );
    assert_eq!(
        controls.record_fields(),
        "\"disable_root_capability_reuse\":false,\"disable_managed_xfs_name_mode_shortcut\":false,\"eager_append_coordination\":false"
    );
}

#[test]
fn isolated_baseline_records_bypass_production_thresholds() {
    for (controls, expected) in [
        (
            BenchmarkIsolationControls {
                disable_root_capability_reuse: true,
                disable_managed_xfs_name_mode_shortcut: false,
                eager_append_coordination: false,
            },
            "\"disable_root_capability_reuse\":true,\"disable_managed_xfs_name_mode_shortcut\":false,\"eager_append_coordination\":false",
        ),
        (
            BenchmarkIsolationControls {
                disable_root_capability_reuse: false,
                disable_managed_xfs_name_mode_shortcut: true,
                eager_append_coordination: false,
            },
            "\"disable_root_capability_reuse\":false,\"disable_managed_xfs_name_mode_shortcut\":true,\"eager_append_coordination\":false",
        ),
        (
            BenchmarkIsolationControls {
                disable_root_capability_reuse: false,
                disable_managed_xfs_name_mode_shortcut: false,
                eager_append_coordination: true,
            },
            "\"disable_root_capability_reuse\":false,\"disable_managed_xfs_name_mode_shortcut\":false,\"eager_append_coordination\":true",
        ),
    ] {
        assert!(
            verify_benchmark_gate(
                "managed", true, controls, "npm-like", 1, 30_000_001, 40_000_000,
            )
            .is_ok()
        );
        assert_eq!(controls.record_fields(), expected);
    }
}

#[test]
fn quick_benchmark_records_keep_thresholds_disabled() {
    let controls = BenchmarkIsolationControls {
        disable_root_capability_reuse: false,
        disable_managed_xfs_name_mode_shortcut: false,
        eager_append_coordination: false,
    };
    assert!(
        verify_benchmark_gate(
            "managed", false, controls, "npm-like", 1, 30_000_001, 40_000_000,
        )
        .is_ok()
    );
}

#[test]
fn managed_workload_gates_accept_boundaries_and_reject_regressions() {
    for (workload, concurrency, p50_limit, p95_limit) in [
        ("npm-like", 1, Some(30_000_000), Some(40_000_000)),
        ("npm-like", 4, None, Some(217_255_442)),
        ("npm-like", 16, None, Some(479_872_717)),
        ("npm-like", 64, Some(700_000_000), Some(1_575_210_845)),
        ("git-clone-like", 1, Some(40_000_000), Some(50_000_000)),
        ("git-clone-like", 4, None, Some(287_786_406)),
        ("git-clone-like", 16, None, Some(577_242_017)),
        (
            "git-clone-like",
            64,
            Some(1_000_000_000),
            Some(2_576_282_922),
        ),
    ] {
        let p50 = p50_limit.unwrap_or(0);
        let p95 = p95_limit.unwrap_or(0);
        assert!(
            verify_managed_workload_gate(workload, concurrency, p50, p95).is_ok(),
            "{workload} concurrency {concurrency} rejected its exact boundary"
        );
        if p50_limit.is_some() {
            assert!(
                verify_managed_workload_gate(workload, concurrency, p50 + 1, p95).is_err(),
                "{workload} concurrency {concurrency} accepted p50 one nanosecond above its limit"
            );
        }
        if p95_limit.is_some() {
            assert!(
                verify_managed_workload_gate(workload, concurrency, p50, p95 + 1).is_err(),
                "{workload} concurrency {concurrency} accepted p95 one nanosecond above its limit"
            );
        }
    }
}

async fn run_workload(
    filesystems: &[BenchmarkFilesystem],
    workload: WorkloadSpec,
    samples_per_agent: usize,
) -> Vec<u128> {
    static RUN_ID: AtomicUsize = AtomicUsize::new(0);
    let run_id = RUN_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let payloads = WorkloadPayloads::new();
    let barrier = Arc::new(Barrier::new(filesystems.len()));
    let mut tasks = JoinSet::new();
    for (agent, filesystem) in filesystems.iter().enumerate() {
        let generation_handle = filesystem.generation_handle.clone();
        let payloads = payloads.clone();
        let barrier = barrier.clone();
        tasks.spawn(async move {
            let mut durations = Vec::with_capacity(samples_per_agent);
            for sample in 0..samples_per_agent {
                let root = PathBuf::from(format!("{}-{run_id}-{agent}-{sample}", workload.name));
                barrier.wait().await;
                let started = Instant::now();
                match workload.name {
                    "npm-like" => {
                        run_npm_like(&generation_handle, &root, &payloads, workload.package_count)
                            .await
                    }
                    "git-clone-like" => {
                        run_git_clone_like(
                            &generation_handle,
                            &root,
                            &payloads,
                            workload.object_count,
                            workload.checkout_file_count,
                            workload.object_fanout,
                        )
                        .await
                    }
                    _ => unreachable!(),
                }
                durations.push(started.elapsed().as_nanos());
                barrier.wait().await;
                match workload.name {
                    "npm-like" => {
                        clean_npm_like(&generation_handle, &root, workload.package_count).await
                    }
                    "git-clone-like" => {
                        clean_git_clone_like(
                            &generation_handle,
                            &root,
                            workload.object_count,
                            workload.checkout_file_count,
                            workload.object_fanout,
                        )
                        .await
                    }
                    _ => unreachable!(),
                }
            }
            durations
        });
    }

    let mut durations = Vec::with_capacity(filesystems.len() * samples_per_agent);
    while let Some(result) = tasks.join_next().await {
        durations.extend(result.unwrap());
    }
    durations
}

#[test]
#[ignore = "requires the privileged managed XFS benchmark runner"]
#[timeout("30m")]
async fn filesystem_workload_benchmark() {
    let mode = std::env::var("GOLEM_FILESYSTEM_BENCH_MODE")
        .expect("GOLEM_FILESYSTEM_BENCH_MODE must be set");
    let managed_root = std::env::var_os("GOLEM_MANAGED_XFS_TEST_ROOT")
        .map(PathBuf::from)
        .expect("GOLEM_MANAGED_XFS_TEST_ROOT must name the mounted XFS test root");
    let quick = std::env::var_os("GOLEM_FILESYSTEM_BENCH_QUICK").is_some_and(|value| value == "1");
    let (provisioning, limits, filesystem_metering) = match mode.as_str() {
        "managed" => (
            SandboxFilesystemProvisioning::new(None, Some(managed_root), RetryConfig::default())
                .unwrap(),
            ResolvedStorageLimits::Finite(FilesystemLimits {
                allocated_bytes: STORAGE_LIMIT_BYTES,
                filesystem_objects: STORAGE_LIMIT_OBJECTS,
            }),
            true,
        ),
        "managed-unmetered" => (
            SandboxFilesystemProvisioning::new(None, Some(managed_root), RetryConfig::default())
                .unwrap(),
            ResolvedStorageLimits::Finite(FilesystemLimits {
                allocated_bytes: STORAGE_LIMIT_BYTES,
                filesystem_objects: STORAGE_LIMIT_OBJECTS,
            }),
            false,
        ),
        "unmanaged" => {
            let root = managed_root.join("unmanaged-workload-benchmark");
            std::fs::create_dir_all(&root).unwrap();
            (
                SandboxFilesystemProvisioning::new(Some(root), None, RetryConfig::default())
                    .unwrap(),
                ResolvedStorageLimits::Unlimited,
                false,
            )
        }
        _ => panic!("unsupported GOLEM_FILESYSTEM_BENCH_MODE: {mode}"),
    };

    let single_agent =
        std::env::var_os("GOLEM_FILESYSTEM_BENCH_SINGLE_AGENT").is_some_and(|value| value == "1");
    let concurrency_levels: &[usize] = if quick || single_agent {
        &[1]
    } else {
        &[1, 4, 16, 64]
    };
    for &concurrency in concurrency_levels {
        let samples_per_agent = if quick {
            2
        } else {
            match concurrency {
                1 => 16,
                4 => 8,
                16 => 4,
                64 => 1,
                _ => unreachable!(),
            }
        };
        for workload in [
            WorkloadSpec::npm_like(quick),
            WorkloadSpec::git_clone_like(quick),
        ] {
            let mut filesystems = Vec::with_capacity(concurrency);
            for index in 0..concurrency {
                filesystems.push(
                    create_benchmark_filesystem(
                        provisioning.clone(),
                        benchmark_agent_id(index),
                        limits,
                        filesystem_metering,
                    )
                    .await,
                );
            }
            println!(
                "FILESYSTEM_WORKLOAD_BENCHMARK_PHASE {{\"mode\":\"{mode}\",\"workload\":\"{}\",\"concurrency\":{concurrency},\"phase\":\"start\"}}",
                workload.name
            );
            let samples = run_workload(&filesystems, workload, samples_per_agent).await;
            report(&mode, workload, concurrency, samples, !quick);
            println!(
                "FILESYSTEM_WORKLOAD_BENCHMARK_PHASE {{\"mode\":\"{mode}\",\"workload\":\"{}\",\"concurrency\":{concurrency},\"phase\":\"end\"}}",
                workload.name
            );

            for filesystem in filesystems {
                close_window(filesystem.window, Instant::now() + Duration::from_secs(5))
                    .await
                    .unwrap();
                delete(seal(filesystem.resident)).await.unwrap();
            }
        }
    }
}

#[test]
#[ignore = "requires the privileged managed XFS benchmark runner"]
#[timeout("5m")]
async fn filesystem_native_workload_baseline() {
    let managed_root = std::env::var_os("GOLEM_MANAGED_XFS_TEST_ROOT")
        .map(PathBuf::from)
        .expect("GOLEM_MANAGED_XFS_TEST_ROOT must name the mounted XFS test root");
    let baseline_root = managed_root.join("native-workload-baseline");
    if baseline_root.exists() {
        std::fs::remove_dir_all(&baseline_root).unwrap();
    }
    std::fs::create_dir(&baseline_root).unwrap();
    let payloads = WorkloadPayloads::new();

    for workload in [
        WorkloadSpec::npm_like(false),
        WorkloadSpec::git_clone_like(false),
    ] {
        let mut durations = Vec::with_capacity(16);
        for sample in 0..16 {
            let root = baseline_root.join(format!("{}-{sample}", workload.name));
            let started = Instant::now();
            match workload.name {
                "npm-like" => {
                    run_sandbox_npm_like(&root, &payloads, workload.package_count);
                }
                "git-clone-like" => {
                    run_sandbox_git_clone_like(
                        &root,
                        &payloads,
                        workload.object_count,
                        workload.checkout_file_count,
                        workload.object_fanout,
                    );
                }
                _ => unreachable!(),
            }
            durations.push(started.elapsed().as_nanos());
            std::fs::remove_dir_all(root).unwrap();
        }
        report("native", workload, 1, durations, false);
    }

    std::fs::remove_dir(&baseline_root).unwrap();
}
