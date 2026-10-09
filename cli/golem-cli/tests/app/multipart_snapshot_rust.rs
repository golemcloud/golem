use golem_rust::agentic::{MultipartSnapshot, SnapshotPart, SnapshotRestoreContext};
use golem_rust::{agent_definition, agent_implementation};

const MARKER: &str = "revision-0";
#[derive(golem_rust::serde::Serialize, golem_rust::serde::Deserialize)]
struct Saved {
    revision: u32,
}

#[agent_definition(snapshotting = "every(10)")]
pub trait MultipartIndex {
    fn new(name: String) -> Self;
    fn append(&mut self, byte: u8);
    fn inspect(&self) -> String;
}
struct MultipartIndexImpl {
    revision: u32,
    index: Vec<u8>,
    restored: bool,
}
#[agent_implementation]
impl MultipartIndex for MultipartIndexImpl {
    fn new(_name: String) -> Self {
        Self {
            revision: 0,
            index: (0..=255).collect(),
            restored: false,
        }
    }
    fn append(&mut self, byte: u8) {
        self.revision += 1;
        self.index.push(byte);
    }
    fn inspect(&self) -> String {
        format!(
            "{}|{}|{}|{}",
            MARKER,
            self.revision,
            self.restored,
            self.index
                .iter()
                .map(u8::to_string)
                .collect::<Vec<_>>()
                .join(",")
        )
    }
    async fn save_snapshot_parts(&self) -> Result<MultipartSnapshot, String> {
        MultipartSnapshot::from_state(
            &Saved {
                revision: self.revision,
            },
            std::collections::BTreeMap::from([(
                "index".into(),
                SnapshotPart {
                    bytes: self.index.clone(),
                    content_type: "application/octet-stream".into(),
                },
            )]),
        )
    }
    async fn load_snapshot_parts(
        snapshot: MultipartSnapshot,
        _context: SnapshotRestoreContext,
    ) -> Result<Self, String> {
        let saved: Saved = snapshot.decode_state()?;
        let index = snapshot
            .require_part("index", "application/octet-stream")?
            .to_vec();
        Ok(Self {
            revision: saved.revision,
            index,
            restored: true,
        })
    }
}
