use golem_rust::{agent_definition, agent_implementation};
use std::fs::{metadata, read_to_string, set_permissions, write};

#[agent_definition]
pub trait FileReadWrite {
    fn new(name: String) -> Self;
    fn run(
        &self,
    ) -> (
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
    );
}

struct FileReadWriteImpl {
    _name: String,
}

#[agent_implementation]
impl FileReadWrite for FileReadWriteImpl {
    fn new(name: String) -> Self {
        Self { _name: name }
    }

    fn run(
        &self,
    ) -> (
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
    ) {
        println!("initial ro file read starting");
        let ro_read_result: Option<String> = read_to_string("foo.txt").ok();

        println!("initial ro file write starting");
        let ro_write_result = write("foo.txt", "hello world")
            .ok()
            .map(|_| "success".to_string());

        println!("initial ro file permission change starting");
        let ro_permission_change_result = {
            let metadata = metadata("foo.txt").unwrap();
            let mut permissions = metadata.permissions();
            permissions.set_readonly(true);
            set_permissions("foo.txt", permissions)
                .ok()
                .map(|_| "success".to_string())
        };

        println!("initial rw file read starting");
        let rw_read_result: Option<String> = read_to_string("bar/baz.txt").ok();

        println!("initial rw file write starting");
        write("bar/baz.txt", "hello world").unwrap();
        let rw_read_after_update_result: Option<String> = read_to_string("bar/baz.txt").ok();

        (
            ro_read_result,
            ro_write_result,
            ro_permission_change_result,
            rw_read_result,
            rw_read_after_update_result,
        )
    }
}
