import re

with open('cli/golem-cli/tests/mcp_server.rs', 'r') as f:
    content = f.read()

sse_test_code = """
    // Test SSE stream endpoint
    let sse_url = format!("http://127.0.0.1:{}/sse", port);
    let sse_resp = client.get(&sse_url)
        .send()
        .await
        .expect("Failed to connect to /sse");
    
    assert!(sse_resp.status().is_success(), "SSE endpoint returned non-success status");
    let content_type = sse_resp.headers()
        .get("content-type")
        .expect("Missing content-type header")
        .to_str()
        .unwrap();
    assert!(content_type.starts_with("text/event-stream"), "SSE endpoint did not return text/event-stream");

    child.kill().await.unwrap();
}
"""

content = content.replace("    child.kill().await.unwrap();\n}", sse_test_code)

with open('cli/golem-cli/tests/mcp_server.rs', 'w') as f:
    f.write(content)
