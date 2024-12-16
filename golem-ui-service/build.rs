use std::fs;
use std::path::Path;

fn main() {
    let index_path = Path::new("frontend/dist/index.html");

    if !index_path.exists() {
        println!("cargo:warning=Frontend assets not found. Creating a default index.html.");

        let html_content = r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Frontend Not Compiled</title>
</head>
<body>
    <h1>Frontend assets have not been compiled</h1>
    <p>Please run 'npm run build' in the frontend directory to compile the frontend assets.</p>
</body>
</html>"#;

        fs::create_dir_all(index_path.parent().unwrap()).unwrap();
        fs::write(index_path, html_content).unwrap();

        println!("cargo:rerun-if-changed=frontend/dist/index.html");
    }
}
