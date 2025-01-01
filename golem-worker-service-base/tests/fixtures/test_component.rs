use super::comprehensive_wit_types::*;

pub struct TestComponent;

impl TestComponent {
    // Test primitive types
    pub fn test_primitives(&self) -> PrimitiveTypes {
        PrimitiveTypes {
            bool_val: true,
            u8_val: 255,
            u16_val: 65535,
            u32_val: 4294967295,
            u64_val: 18446744073709551615,
            s8_val: -128,
            s16_val: -32768,
            s32_val: -2147483648,
            s64_val: -9223372036854775808,
            f32_val: 3.14159,
            f64_val: 2.718281828459045,
            char_val: '🦀',
            string_val: "Hello, WIT!".to_string(),
        }
    }

    // Test complex record with optional fields
    pub fn test_user_profile(&self) -> UserProfile {
        UserProfile {
            id: 42,
            username: "test_user".to_string(),
            settings: Some(UserSettings {
                theme: "dark".to_string(),
                notifications_enabled: true,
                email_frequency: "daily".to_string(),
            }),
            permissions: UserPermissions {
                can_read: true,
                can_write: true,
                can_delete: false,
                is_admin: false,
            },
        }
    }

    // Test variant type with different cases
    pub fn test_content_types(&self) -> Vec<ContentType> {
        vec![
            ContentType::Text("Plain text".to_string()),
            ContentType::Number(42.0),
            ContentType::Boolean(true),
            ContentType::Complex {
                id: 1,
                data: vec!["data1".to_string(), "data2".to_string()],
            },
        ]
    }

    // Test Result type
    pub fn test_operation_result(&self, succeed: bool) -> OperationResult {
        if succeed {
            Ok(SuccessResponse {
                code: 200,
                message: "Operation successful".to_string(),
                data: Some("Additional data".to_string()),
            })
        } else {
            Err(ErrorDetails {
                code: 400,
                message: "Operation failed".to_string(),
                details: Some(vec![
                    "Invalid input".to_string(),
                    "Please try again".to_string(),
                ]),
            })
        }
    }

    pub fn perform_search(&self, _query: SearchQuery) -> SearchResult {
        SearchResult {
            matches: vec![
                SearchMatch {
                    id: 1,
                    score: 0.95,
                    context: "Found in document 1".to_string(),
                },
                SearchMatch {
                    id: 2,
                    score: 0.85,
                    context: "Found in document 2".to_string(),
                },
            ],
            total_count: 2,
            execution_time_ms: 100,
        }
    }

    pub fn validate_search_query(&self, query: SearchQuery) -> Result<bool, String> {
        if query.query.is_empty() {
            return Err("Query cannot be empty".to_string());
        }
        Ok(true)
    }

    pub fn batch_process(&self, items: Vec<String>, _options: BatchOptions) -> BatchResult {
        BatchResult {
            successful: items.len() as u32 - 1,
            failed: 1,
            errors: vec!["Failed to process item 3".to_string()],
        }
    }

    pub fn batch_validate(&self, items: Vec<String>) -> Vec<Result<bool, String>> {
        items.iter().map(|item| {
            if item.len() > 3 {
                Ok(true)
            } else {
                Err("Item too short".to_string())
            }
        }).collect()
    }

    pub fn apply_transformation(&self, data: Vec<String>, _transform: DataTransformation) -> TransformationResult {
        let input_size = data.len() as u32;
        TransformationResult {
            success: true,
            output: data,
            metrics: TransformationMetrics {
                input_size,
                output_size: input_size,
                duration_ms: 50,
            },
        }
    }

    pub fn chain_transformations(&self, data: Vec<String>, _transforms: Vec<DataTransformation>) 
        -> Result<TransformationResult, String> 
    {
        let input_size = data.len() as u32;
        Ok(TransformationResult {
            success: true,
            output: data,
            metrics: TransformationMetrics {
                input_size,
                output_size: input_size,
                duration_ms: 150,
            },
        })
    }

    pub fn create_tree(&self, root: TreeNode) -> Result<TreeNode, String> {
        Ok(root)
    }

    pub fn modify_tree(&self, _operation: TreeOperation) -> Result<OperationStats, String> {
        Ok(OperationStats {
            operation_type: "insert".to_string(),
            nodes_affected: 1,
            depth_changed: 1,
        })
    }

    pub fn query_tree(&self, node_id: u32, _depth: Option<u32>) -> Option<TreeNode> {
        Some(TreeNode {
            id: node_id,
            value: "test".to_string(),
            children: vec![],
            metadata: NodeMetadata {
                created_at: 1234567890,
                modified_at: 1234567890,
                tags: vec!["test".to_string()],
            },
        })
    }

    pub fn process_batch_async(&self, _items: Vec<String>, _options: BatchOptions) -> Result<u32, String> {
        Ok(42) // Return a batch ID
    }

    pub fn get_batch_status(&self, _batch_id: u32) -> Option<BatchResult> {
        Some(BatchResult {
            successful: 10,
            failed: 0,
            errors: vec![],
        })
    }

    pub fn validate_complex_input(&self, 
        _profile: UserProfile,
        _query: SearchQuery,
        _options: BatchOptions
    ) -> Result<bool, Vec<String>> {
        Ok(true)
    }
} 