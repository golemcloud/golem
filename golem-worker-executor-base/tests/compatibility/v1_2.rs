use crate::compatibility::v1::backward_compatible;
use goldenfile::Mint;
use golem_common::base_model::OplogIndex;
use golem_common::model::oplog::OplogEntry;
use golem_common::model::regions::OplogRegion;
use golem_common::model::Timestamp;

#[test]
pub fn oplog_entry() {
    let oe31 = OplogEntry::Revert {
        timestamp: Timestamp::from(1724701938466),
        dropped_region: OplogRegion {
            start: OplogIndex::from_u64(3),
            end: OplogIndex::from_u64(10),
        },
    };

    let mut mint = Mint::new("tests/goldenfiles");
    backward_compatible("oplog_entry_revert", &mut mint, oe31);
}
