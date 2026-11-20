use golem_rust::{agent_definition, agent_implementation};

/// Observes the native filesystem only through a directory listing of the preopened root: no
/// `open-at`, file read, environment, clock or random call happens before the listing.
#[agent_definition]
pub trait DirectoryListing {
    fn new() -> Self;
    /// Lists the preopened root directory, then draws random bytes. agent-updates-v2 reads the
    /// system clock instead, so a full replay of this call on v2 diverges right after the listing.
    async fn list_then_draw(&self) -> u32;
}

struct DirectoryListingImpl;

#[agent_implementation]
impl DirectoryListing for DirectoryListingImpl {
    fn new() -> Self {
        Self
    }

    async fn list_then_draw(&self) -> u32 {
        let (root, _) = golem_rust::wasip3::filesystem::preopens::get_directories()
            .into_iter()
            .next()
            .expect("the agent has a preopened root directory");
        let (entries, result) = root.read_directory();
        let count = entries.collect().await.len() as u32;
        let _ = result.await;
        let _ = golem_rust::wasip3::random::random::get_random_bytes(1);
        count
    }
}
