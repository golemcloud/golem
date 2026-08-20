use golem_rust::{agent_definition, agent_implementation};

mod parity;
mod quota;

#[agent_definition]
pub trait P3FileSystem {
    fn new(name: String) -> Self;
    /// Runs the same filesystem operations through both the WASI 0.2 and WASI 0.3 imports
    /// against a read-only and a read-write initial file, and reports `name=value` entries
    /// so the host-side test can assert P2/P3 parity.
    async fn run(&self) -> Vec<String>;
    /// Reads the mutable initial file left by `run` through both WASI versions
    /// without modifying it.
    async fn inspect_run(&self) -> Vec<String>;
    /// Runs read-write filesystem operations through both WASI versions against
    /// a file created by the agent.
    async fn run_writable(&self) -> Vec<String>;
    /// Reads the file produced by `run_writable` through both WASI versions
    /// without modifying it.
    async fn inspect_writable(&self) -> Vec<String>;
    /// Applies filesystem mutation histories whose final bytes and topology are
    /// inspected after reconstruction.
    async fn run_reconstruction_matrix(&self) -> Vec<String>;
    /// Inspects the final bytes and topology produced by `run_reconstruction_matrix`.
    async fn inspect_reconstruction_matrix(&self) -> Vec<String>;
    /// Replaces a file used to verify reconstruction to an exact oplog position.
    async fn write_replay_target(&self, value: String);
    /// Reads a file without changing it.
    async fn inspect_path(&self, path: String) -> Vec<String>;
    /// Writes blocks through a P2 direct descriptor until the project quota
    /// denies further growth.
    async fn run_p2_quota_surface(&self) -> Vec<String>;
    /// Streams one block through P3 while project quota enforcement is active.
    async fn run_p3_with_quota(&self) -> bool;
    /// Attempts to stream more data through P3 than the project limit permits.
    async fn exhaust_p3_quota(&self) -> Vec<String>;
    /// Attempts a P2 write larger than the available filesystem capacity.
    async fn exhaust_p2_quota(&self) -> Vec<String>;
    /// Inspects the successfully persisted prefix of the failed P2 write.
    async fn inspect_p2_exhaustion(&self) -> Vec<String>;
    /// Inspects the successfully persisted prefix of the failed P3 quota write.
    async fn inspect_p3_exhaustion(&self) -> Vec<String>;
    /// Exercises the storage-affecting P2 filesystem operations under a project quota.
    async fn run_p2_quota_matrix(&self) -> Vec<String>;
    /// Exercises the storage-affecting P3 filesystem operations under a project quota.
    async fn run_p3_quota_matrix(&self) -> Vec<String>;
    /// Exhausts the P2 object quota and verifies open-unlinked inode accounting.
    async fn run_p2_object_quota(&self) -> Vec<String>;
    /// Verifies P2 object capacity returns after the prior invocation closes its handles.
    async fn complete_p2_object_quota_release(&self) -> bool;
    /// Exhausts the P3 object quota and verifies open-unlinked inode accounting.
    async fn run_p3_object_quota(&self) -> Vec<String>;
    /// Verifies P3 object capacity returns after the prior invocation closes its handles.
    async fn complete_p3_object_quota_release(&self) -> bool;
    /// Confirms that the guest invocation started without producing side effects.
    async fn confirm_invocation_started(&self) -> String;
    /// Verifies that abandoning only a P3 write completion future does not cancel
    /// the write still driven by its input stream.
    async fn abandon_p3_write_completion(&self) -> bool;
}

struct P3FileSystemImpl {
    _name: String,
}

#[agent_implementation]
impl P3FileSystem for P3FileSystemImpl {
    fn new(name: String) -> Self {
        Self { _name: name }
    }

    async fn run(&self) -> Vec<String> {
        parity::run().await
    }

    async fn inspect_run(&self) -> Vec<String> {
        parity::inspect_run().await
    }

    async fn run_writable(&self) -> Vec<String> {
        parity::run_writable().await
    }

    async fn inspect_writable(&self) -> Vec<String> {
        parity::inspect_writable().await
    }

    async fn run_reconstruction_matrix(&self) -> Vec<String> {
        parity::run_reconstruction_matrix().await
    }

    async fn inspect_reconstruction_matrix(&self) -> Vec<String> {
        parity::inspect_reconstruction_matrix().await
    }

    async fn write_replay_target(&self, value: String) {
        parity::write_replay_target(&value)
    }

    async fn inspect_path(&self, path: String) -> Vec<String> {
        parity::inspect_file(&path).await
    }

    async fn run_p2_quota_surface(&self) -> Vec<String> {
        quota::run_p2_quota_surface().await
    }

    async fn run_p3_with_quota(&self) -> bool {
        quota::run_p3_with_quota().await
    }

    async fn exhaust_p3_quota(&self) -> Vec<String> {
        quota::exhaust_p3_quota().await
    }

    async fn exhaust_p2_quota(&self) -> Vec<String> {
        quota::exhaust_p2_quota()
    }

    async fn inspect_p2_exhaustion(&self) -> Vec<String> {
        quota::inspect_p2_exhaustion()
    }

    async fn inspect_p3_exhaustion(&self) -> Vec<String> {
        quota::inspect_p3_exhaustion().await
    }

    async fn run_p2_quota_matrix(&self) -> Vec<String> {
        quota::run_p2_quota_matrix().await
    }

    async fn run_p3_quota_matrix(&self) -> Vec<String> {
        quota::run_p3_quota_matrix().await
    }

    async fn run_p2_object_quota(&self) -> Vec<String> {
        quota::run_p2_object_quota().await
    }

    async fn complete_p2_object_quota_release(&self) -> bool {
        quota::complete_p2_object_quota_release().await
    }

    async fn run_p3_object_quota(&self) -> Vec<String> {
        quota::run_p3_object_quota().await
    }

    async fn complete_p3_object_quota_release(&self) -> bool {
        quota::complete_p3_object_quota_release().await
    }

    async fn confirm_invocation_started(&self) -> String {
        "executed".to_string()
    }

    async fn abandon_p3_write_completion(&self) -> bool {
        parity::abandon_p3_write_completion().await
    }
}
