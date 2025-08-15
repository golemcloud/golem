use assert_cmd::Command;
use httpmock::prelude::*;
use predicates::str::contains;
use tempfile::NamedTempFile;

#[test]
fn cli_list_files_json() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET).path("/comp/workers/worker/files/");
        then.status(200).json_body_obj(&serde_json::json!({
            "nodes": [{"name":"file.txt","path":"/file.txt","is_dir":false}]
        }));
    });
    let mut cmd = Command::cargo_bin("golem").unwrap();
    cmd.env("GOLEM_URL", server.url(""))
        .args(["worker", "files", "list", "comp", "worker", "--json"]);
    cmd.assert().success().stdout(contains("file.txt"));
}

#[test]
fn cli_get_file_output() {
    let server = MockServer::start();
    let content = b"hello".to_vec();
    server.mock(|when, then| {
        when.method(GET).path("/comp/workers/worker/file-contents/file%2Etxt");
        then.status(200).body(content.clone());
    });
    let tmp = NamedTempFile::new().unwrap();
    let path = tmp.path().to_str().unwrap().to_string();
    let mut cmd = Command::cargo_bin("golem").unwrap();
    cmd.env("GOLEM_URL", server.url(""))
        .args(["worker", "files", "get", "comp", "worker", "file.txt", "--output", &path]);
    cmd.assert().success();
    let saved = std::fs::read(path).unwrap();
    assert_eq!(saved, content);
}