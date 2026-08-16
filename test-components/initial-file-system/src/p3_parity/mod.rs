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
    /// Runs read-write filesystem operations through both WASI versions against
    /// a file created by the agent.
    async fn run_writable(&self) -> Vec<String>;
    /// Writes blocks through a P2 direct descriptor until the project quota
    /// denies further growth.
    async fn run_p2_quota_surface(&self) -> Vec<String>;
    /// Streams one block through P3 while project quota enforcement is active.
    async fn run_p3_with_quota(&self) -> bool;
    /// Attempts to stream more data through P3 than the project limit permits.
    async fn exhaust_p3_quota(&self) -> Vec<String>;
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

    async fn run_writable(&self) -> Vec<String> {
        parity::run_writable().await
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
}
