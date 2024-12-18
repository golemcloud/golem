use axum::response::Html;
use axum::extract::Path;
use axum::http::StatusCode;

pub async fn serve_swagger_ui(Path(path): Path<String>) -> Result<Html<String>, StatusCode> {
    let html = match std::fs::read_to_string("assets/swagger-ui/index.html") {
        Ok(content) => content,
        Err(_) => return Err(StatusCode::INTERNAL_SERVER_ERROR),
    };
    
    let modified_html = html.replace("{{SPEC_URL}}", &format!("/v1/api/definitions/{}/version/{}/export", path, path));
    Ok(Html(modified_html))
}