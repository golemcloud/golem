
#[cfg(test)]
mod tests {
    use golem_worker_service_base::gateway_api_definition::http::HttpApiDefinition;
    use std::fs;
    use std::path::PathBuf;
    use std::process::Command;
    use std::time::Duration;
    use tokio::time::sleep;

    use openapi_client_tests::test_server::TestServer;

    async fn start_test_server() {
        let server = TestServer::new();
        tokio::spawn(async move {
            server.start(3000).await;
        });
        // Give the server time to start
        sleep(Duration::from_secs(1)).await;
    }

    fn verify_typescript_client() -> Result<(), Box<dyn std::error::Error>> {
        Command::new("npm")
            .current_dir("tests/openapi-client-tests/typescript/client")
            .arg("install")
            .status()?;

        let test_code = r#"
import { Configuration, DefaultApi } from './';

async function test() {
    const config = new Configuration({
        basePath: 'http://localhost:3000'
    });
    const api = new DefaultApi(config);

    const newUser = {
        id: 1,
        name: 'Test User',
        email: 'test@example.com'
    };
    const createResponse = await api.createUser(newUser);
    console.assert(createResponse.data.status === 'success', 'Create user failed');

    const getResponse = await api.getUser(1);
    console.assert(getResponse.data.data.name === 'Test User', 'Get user failed');

    const updatedUser = { ...newUser, name: 'Updated User' };
    const updateResponse = await api.updateUser(1, updatedUser);
    console.assert(updateResponse.data.data.name === 'Updated User', 'Update user failed');

    const deleteResponse = await api.deleteUser(1);
    console.assert(deleteResponse.data.status === 'success', 'Delete user failed');
}

test().catch(console.error);
"#;
        fs::write(
            "tests/openapi-client-tests/typescript/client/test.ts",
            test_code,
        )?;

        Command::new("npx")
            .current_dir("tests/openapi-client-tests/typescript/client")
            .args(&["ts-node", "test.ts"])
            .status()?;

        Ok(())
    }

    fn verify_python_client() -> Result<(), Box<dyn std::error::Error>> {
        Command::new("pip")
            .args(&["install", "-r", "requirements.txt"])
            .current_dir("tests/openapi-client-tests/python/client")
            .status()?;

        let test_code = r#"
import unittest
from __future__ import absolute_import
import os
import sys
sys.path.append(".")

import openapi_client
from openapi_client.rest import ApiException

class TestDefaultApi(unittest.TestCase):
    def setUp(self):
        configuration = openapi_client.Configuration(
            host="http://localhost:3000"
        )
        self.api = openapi_client.DefaultApi(openapi_client.ApiClient(configuration))

    def test_crud_operations(self):
        new_user = {
            "id": 1,
            "name": "Test User",
            "email": "test@example.com"
        }
        response = self.api.create_user(new_user)
        self.assertEqual(response.status, "success")

        response = self.api.get_user(1)
        self.assertEqual(response.data.name, "Test User")

        updated_user = {
            "id": 1,
            "name": "Updated User",
            "email": "test@example.com"
        }
        response = self.api.update_user(1, updated_user)
        self.assertEqual(response.data.name, "Updated User")

        response = self.api.delete_user(1)
        self.assertEqual(response.status, "success")

if __name__ == '__main__':
    unittest.main()
"#;
        fs::write(
            "tests/openapi-client-tests/python/client/test_api.py",
            test_code,
        )?;

        Command::new("python")
            .args(&["-m", "unittest", "test_api.py"])
            .current_dir("tests/openapi-client-tests/python/client")
            .status()?;

        Ok(())
    }

    fn generate_openapi_spec(api_def: &HttpApiDefinition) -> String {
        serde_json::to_string_pretty(api_def).unwrap()
    }

    fn generate_client_library(
        openapi_spec: &str,
        lang: &str,
        output_dir: &PathBuf,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let status = Command::new("openapi-generator-cli")
            .arg("version")
            .status()?;

        if !status.success() {
            return Err("openapi-generator-cli not found. Please install it first.".into());
        }

        let status = Command::new("openapi-generator-cli")
            .args(&[
                "generate",
                "-i",
                openapi_spec,
                "-g",
                lang,
                "-o",
                output_dir.to_str().unwrap(),
            ])
            .status()?;

        if !status.success() {
            return Err("Failed to generate client library".into());
        }

        Ok(())
    }
}
