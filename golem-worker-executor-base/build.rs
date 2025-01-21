use cargo_metadata::MetadataCommand;
use std::env::var_os;
use std::path::Path;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let golem_wit_root = find_package_root("golem-wit");
    let out_dir = var_os("OUT_DIR").unwrap();
    let target_file = Path::new(&out_dir).join("preview2_mod.rs");

    std::fs::write(target_file.clone(), preview2_mod_gen(&golem_wit_root)).unwrap();

    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=Cargo.toml");

    Ok(())
}

fn find_package_root(name: &str) -> String {
    let metadata = MetadataCommand::new()
        .manifest_path("./Cargo.toml")
        .exec()
        .unwrap();

    let package = metadata
        .packages
        .into_iter()
        .fold(None, |acc, p| {
            if p.name == name {
                match acc {
                    None => Some(p),
                    Some(cp)
                        if (cp.version < p.version
                            && (cp.version.major != 0
                                || cp.version.minor != 0
                                || cp.version.patch != 0))
                            || (p.version.major == 0
                                && p.version.minor == 0
                                && p.version.patch == 0) =>
                    {
                        Some(p)
                    }
                    _ => acc,
                }
            } else {
                acc
            }
        })
        .unwrap();

    package.manifest_path.parent().unwrap().to_string()
}

fn preview2_mod_gen(golem_wit_path: &str) -> String {
    let golem_wit_path = Path::new(golem_wit_path)
        .join("wit")
        .to_str()
        .map(|s| s.to_string())
        .unwrap();
    format!(
        r#"wasmtime::component::bindgen!({{
        path: r"{golem_wit_path}",
        inline: "
          package golem:api;

          world golem {{
              import golem:api/host@0.2.0;
              import golem:api/host@1.1.0;
              import golem:api/oplog@1.1.0;
              import golem:api/durability@1.2.0;

              import wasi:blobstore/blobstore;
              import wasi:blobstore/container;
              import wasi:blobstore/types;
              import wasi:keyvalue/atomic@0.1.0;
              import wasi:keyvalue/eventual-batch@0.1.0;
              import wasi:keyvalue/cache@0.1.0;
              import wasi:keyvalue/eventual@0.1.0;
              import wasi:keyvalue/types@0.1.0;
              import wasi:keyvalue/wasi-keyvalue-error@0.1.0;
              import wasi:logging/logging;
          }}
        ",
        world: "golem:api/golem",
        tracing: false,
        async: true,
        trappable_imports: true,
        with: {{
            "wasi:io/streams/input-stream": InputStream,
            "wasi:io/streams/output-stream": OutputStream,
            "wasi:io/poll/pollable": Pollable,
            "wasi:blobstore/container/container": super::durable_host::blobstore::types::ContainerEntry,
            "wasi:blobstore/container/stream-object-names": super::durable_host::blobstore::types::StreamObjectNamesEntry,
            "wasi:blobstore/types/incoming-value": super::durable_host::blobstore::types::IncomingValueEntry,
            "wasi:blobstore/types/outgoing-value": super::durable_host::blobstore::types::OutgoingValueEntry,
            "wasi:keyvalue/wasi-keyvalue-error/error": super::durable_host::keyvalue::error::ErrorEntry,
            "wasi:keyvalue/types/bucket": super::durable_host::keyvalue::types::BucketEntry,
            "wasi:keyvalue/types/incoming-value": super::durable_host::keyvalue::types::IncomingValueEntry,
            "wasi:keyvalue/types/outgoing-value": super::durable_host::keyvalue::types::OutgoingValueEntry,
            "golem:api/host/get-workers": super::durable_host::golem::GetWorkersEntry,
            "golem:api/oplog/get-oplog": super::durable_host::golem::v11::GetOplogEntry,
            "golem:api/oplog/search-oplog": super::durable_host::golem::v11::SearchOplogEntry,
        }},
    }});
        "#
    )
}
