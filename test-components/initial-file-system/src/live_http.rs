use golem_rust::{agent_definition, agent_implementation};

#[agent_definition(
    mount = "/files/{name}",
    auth = false,
    cors = ["https://allowed.test"],
    filesystem_bindings = [
        ("/alias", "/public/created.txt"),
        ("/*", "/public/$1"),
        ("/*", "/fallback/$1"),
    ],
)]
pub trait LiveFiles {
    fn new(name: String) -> Self;
    fn replace(&self, path: String, contents: Vec<u8>) -> bool;
}

struct LiveFilesImpl;

#[agent_implementation]
impl LiveFiles for LiveFilesImpl {
    fn new(name: String) -> Self {
        assert!(name != "fail", "private initializer failure");
        std::fs::create_dir_all("/public/directory").unwrap();
        std::fs::create_dir_all("/fallback").unwrap();
        std::fs::create_dir_all("/private").unwrap();
        let (root, _) = wasi::filesystem::preopens::get_directories()
            .into_iter()
            .find(|(_, path)| path == "/")
            .unwrap();
        std::fs::write("/public/created.txt", name.as_bytes()).unwrap();
        std::fs::write("/fallback/only.txt", b"fallback").unwrap();
        std::fs::write("/fallback/directory", b"must not fall through").unwrap();
        std::fs::write("/private/secret.txt", b"must not be exposed").unwrap();
        root.symlink_at("../private/secret.txt", "public/link.txt")
            .unwrap();
        root.symlink_at("../private", "public/parent-link").unwrap();
        if name.starts_with("large-") {
            // gRPC gzip must not shrink this below the transport receive window.
            let mut contents = vec![0; 32 * 1024 * 1024];
            blake3::Hasher::new()
                .update(b"live-file-backpressure")
                .finalize_xof()
                .fill(&mut contents);
            std::fs::write("/public/large.bin", contents).unwrap();
        }
        Self
    }

    fn replace(&self, path: String, contents: Vec<u8>) -> bool {
        std::fs::write(path, contents).is_ok()
    }
}
