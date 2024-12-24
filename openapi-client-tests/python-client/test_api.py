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
        # Test create user
        new_user = {
            "id": 1,
            "name": "Test User",
            "email": "test@example.com"
        }
        response = self.api.create_user(new_user)
        self.assertEqual(response.status, "success")

        # Test get user
        response = self.api.get_user(1)
        self.assertEqual(response.data.name, "Test User")

        # Test update user
        updated_user = {
            "id": 1,
            "name": "Updated User",
            "email": "test@example.com"
        }
        response = self.api.update_user(1, updated_user)
        self.assertEqual(response.data.name, "Updated User")

        # Test delete user
        response = self.api.delete_user(1)
        self.assertEqual(response.status, "success")

if __name__ == '__main__':
    unittest.main()