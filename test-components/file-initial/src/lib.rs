mod bindings;

use crate::bindings::Guest;

struct Component;

fn transpose(res: Result<String, String>) -> Result<String, String> {
    match res {
        Ok(ok) => Err(ok),
        Err(err) => Ok(err),
    }
}

fn read_and_write(path: &str) -> Result<String, String> {
    println!("Reading '{path}'...");
    let mut contents = std::fs::read_to_string(path)
        .map_err(|e| e.to_string())?;
    println!("Reading '{path}' succeeded");
    
    contents.make_ascii_uppercase();
    
    println!("Writing '{path}'...");
    std::fs::write(path, &contents)
        .map_err(|e| e.to_string())?;
    println!("Writing '{path}' succeeded");
    
    println!("Reading '{path}' back...");
    let mut new_contents = std::fs::read_to_string(path)
        .map_err(|e| e.to_string())?;
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

fn view() -> String {
    let mut output = String::new();

    let mut paths = vec!["/".to_string()];

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
        let quick_fox = read_and_write("/quick_fox.txt");
        println!("{quick_fox:?}");

        // read-only files should fail
        let lorem_ipsum = transpose(read_and_write("/static/lorem_ipsum.txt"));
        println!("{lorem_ipsum:?}");

        let view = view();

        (
            quick_fox,
            lorem_ipsum,
            view,
        )
    }
}

bindings::export!(Component with_types_in bindings);
