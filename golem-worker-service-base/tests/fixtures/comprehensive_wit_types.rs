#[derive(Debug)]
#[allow(dead_code)]
pub struct PrimitiveTypes {
    pub bool_val: bool,
    pub u8_val: u8,
    pub u16_val: u16,
    pub u32_val: u32,
    pub u64_val: u64,
    pub s8_val: i8,
    pub s16_val: i16,
    pub s32_val: i32,
    pub s64_val: i64,
    pub f32_val: f32,
    pub f64_val: f64,
    pub char_val: char,
    pub string_val: String,
}

#[derive(Debug)]
#[allow(dead_code)]
pub struct UserSettings {
    pub theme: String,
    pub notifications_enabled: bool,
    pub email_frequency: String,
}

#[derive(Debug)]
#[allow(dead_code)]
pub struct UserPermissions {
    pub can_read: bool,
    pub can_write: bool,
    pub can_delete: bool,
    pub is_admin: bool,
}

#[derive(Debug)]
#[allow(dead_code)]
pub struct UserProfile {
    pub id: u32,
    pub username: String,
    pub settings: Option<UserSettings>,
    pub permissions: UserPermissions,
}

#[derive(Debug)]
#[allow(dead_code)]
pub struct ComplexData {
    pub id: u32,
    pub data: Vec<String>,
}

#[derive(Debug)]
#[allow(dead_code)]
pub enum ContentType {
    Text(String),
    Number(f64),
    Boolean(bool),
    Complex { id: u32, data: Vec<String> },
}

#[derive(Debug)]
#[allow(dead_code)]
pub struct SuccessResponse {
    pub code: u16,
    pub message: String,
    pub data: Option<String>,
}

#[derive(Debug)]
#[allow(dead_code)]
pub struct ErrorDetails {
    pub code: u16,
    pub message: String,
    pub details: Option<Vec<String>>,
}

pub type OperationResult = Result<SuccessResponse, ErrorDetails>;

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct SearchQuery {
    pub query: String,
    pub filters: SearchFilters,
    pub pagination: Option<Pagination>,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct SearchFilters {
    pub categories: Vec<String>,
    pub date_range: Option<DateRange>,
    pub flags: SearchFlags,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct SearchFlags {
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub regex_enabled: bool,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct DateRange {
    pub start: u64,
    pub end: u64,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct Pagination {
    pub page: u32,
    pub items_per_page: u32,
}

#[derive(Debug)]
#[allow(dead_code)]
pub struct SearchResult {
    pub matches: Vec<SearchMatch>,
    pub total_count: u32,
    pub execution_time_ms: u32,
}

#[derive(Debug)]
#[allow(dead_code)]
pub struct SearchMatch {
    pub id: u32,
    pub score: f64,
    pub context: String,
}

#[derive(Debug)]
#[allow(dead_code)]
pub enum ValidationError {
    InvalidInput(String),
    OutOfRange { field: String, min: i64, max: i64 },
    MissingRequired(Vec<String>),
}

#[derive(Debug)]
#[allow(dead_code)]
pub struct BatchOperation<T> {
    pub items: Vec<T>,
    pub options: BatchOptions,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct BatchOptions {
    pub parallel: bool,
    pub retry_count: u32,
    pub timeout_ms: u32,
}

#[derive(Debug)]
#[allow(dead_code)]
pub struct BatchResult {
    pub successful: u32,
    pub failed: u32,
    pub errors: Vec<String>,
}

#[derive(Debug)]
#[allow(dead_code)]
pub enum DataTransformation {
    Sort { field: String, ascending: bool },
    Filter { predicate: String },
    Map { expression: String },
    GroupBy { key: String },
}

#[derive(Debug)]
#[allow(dead_code)]
pub struct TransformationResult {
    pub success: bool,
    pub output: Vec<String>,
    pub metrics: TransformationMetrics,
}

#[derive(Debug)]
#[allow(dead_code)]
pub struct TransformationMetrics {
    pub input_size: u32,
    pub output_size: u32,
    pub duration_ms: u32,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct TreeNode {
    pub id: u32,
    pub value: String,
    pub children: Vec<TreeNode>,
    pub metadata: NodeMetadata,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct NodeMetadata {
    pub created_at: u64,
    pub modified_at: u64,
    pub tags: Vec<String>,
}

#[derive(Debug)]
#[allow(dead_code)]
pub enum TreeOperation {
    Insert { parent_id: u32, node: TreeNode },
    Delete { node_id: u32 },
    Move { node_id: u32, new_parent_id: u32 },
    Update { node_id: u32, new_value: String },
}

#[derive(Debug)]
#[allow(dead_code)]
pub struct OperationStats {
    pub operation_type: String,
    pub nodes_affected: u32,
    pub depth_changed: i32,
} 