mod bindings;

use crate::bindings::Guest;

struct Component;

fn read_and_write(path: &str) -> Result<String, String> {
    println!("Reading '{path}'...");
    let mut contents = std::fs::read_to_string(path)
        .map_err(|e| format!("Read {path}: {e}"))?;
    println!("Reading '{path}' succeeded");
    
    contents.make_ascii_uppercase();
    
    println!("Writing '{path}'...");
    std::fs::write(path, &contents)
    .map_err(|e| format!("Write {path}: {e}"))?;
    println!("Writing '{path}' succeeded");
    
    println!("Reading '{path}' back...");
    let mut new_contents = std::fs::read_to_string(path)
    .map_err(|e| format!("Read2 {path}: {e}"))?;
    println!("Reading '{path}' back succeeded...");

    if contents == new_contents {
        new_contents.truncate(32);
        Ok(new_contents)
    } else {
        contents.truncate(32);
        new_contents.truncate(32);
        Err(format!("'{contents}' != '{new_contents}'"))
    }
}

fn view(dir: &str) -> String {
    let mut output = String::new();

    let mut paths = vec![dir.to_string()];

    while let Some(path) = paths.pop() {
        output = format!("{output}{path}\n");
        if let Ok(dir) = std::fs::read_dir(path) {
            for n in dir {
                let p = n.unwrap().path().display().to_string();
                paths.push(p);
            }
        }
    }

    output
}

impl Guest for Component {
    fn run() -> (Result<String, String>, Result<String, String>, String) {
        let view = view("/");

        let quick_fox = read_and_write("/quick_fox.txt");
        println!("{quick_fox:?}");

        // read-only files should fail
        let lorem = read_and_write("/static/ro/lorem.txt");
        println!("{lorem:?}");

        (
            quick_fox,
            lorem,
            view,
        )
    }
}

bindings::export!(Component with_types_in bindings);
