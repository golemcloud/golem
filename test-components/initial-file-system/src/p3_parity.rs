use golem_rust::wasip3::filesystem::preopens as p3_preopens;
use golem_rust::wasip3::filesystem::types as p3_types;
use golem_rust::{agent_definition, agent_implementation};
use wasi::filesystem::preopens as p2_preopens;
use wasi::filesystem::types as p2_types;

#[agent_definition]
pub trait P3FileSystem {
    fn new(name: String) -> Self;
    /// Runs the same filesystem operations through both the WASI 0.2 and WASI 0.3 imports
    /// against a read-only and a read-write initial file, and reports `name=value` entries
    /// so the host-side test can assert P2/P3 parity.
    async fn run(&self) -> Vec<String>;
}

struct P3FileSystemImpl {
    _name: String,
}

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

#[agent_implementation]
impl P3FileSystem for P3FileSystemImpl {
    fn new(name: String) -> Self {
        Self { _name: name }
    }

    async fn run(&self) -> Vec<String> {
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

        results
    }
}
