//! Typed parameters for built-in tools.
//!
//! Provides compile-time type safety and automatic JSON Schema generation
//! for tool parameters using `schemars`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Parameters for the `bash` tool.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct BashParams {
    /// The bash command to execute.
    pub command: String,
    /// Optional timeout in seconds (default: 120).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
}

/// Parameters for the `read` tool.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ReadParams {
    /// Absolute path to the file to read.
    pub file_path: String,
    /// Optional line offset to start reading from.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offset: Option<usize>,
    /// Optional maximum number of lines to read.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
}

/// Parameters for the `write` tool.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct WriteParams {
    /// Absolute path to the file to write.
    pub file_path: String,
    /// The content to write to the file.
    pub content: String,
}

/// Parameters for the `edit` tool.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct EditParams {
    /// Absolute path to the file to edit.
    pub file_path: String,
    /// The exact string to find and replace.
    pub old_string: String,
    /// The replacement string.
    pub new_string: String,
    /// Whether to replace all occurrences (default: false, replaces first only).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub replace_all: bool,
}

/// Generate a JSON Schema for a type that implements `JsonSchema`.
pub fn schema_for<T: JsonSchema>() -> serde_json::Value {
    let schema = schemars::schema_for!(T);
    serde_json::to_value(schema).unwrap_or_default()
}

/// Generate a JSON Schema as a `serde_json::Value` suitable for tool definitions.
///
/// This extracts just the schema object without the `$schema` and `title` fields,
/// making it compatible with OpenAI/Anthropic tool definitions.
pub fn tool_schema_for<T: JsonSchema>() -> serde_json::Value {
    let schema = schemars::schema_for!(T);
    let mut value = serde_json::to_value(schema).unwrap_or_default();

    // Remove $schema and title to match expected tool definition format
    if let Some(obj) = value.as_object_mut() {
        obj.remove("$schema");
        obj.remove("title");
    }

    value
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bash_params_schema_has_required_command() {
        let schema = tool_schema_for::<BashParams>();
        let required = schema.get("required").and_then(|r| r.as_array());
        assert!(required.is_some());
        let required = required.unwrap();
        assert!(required.iter().any(|v| v.as_str() == Some("command")));
    }

    #[test]
    fn read_params_schema_has_file_path() {
        let schema = tool_schema_for::<ReadParams>();
        let props = schema.get("properties");
        assert!(props.is_some());
        let props = props.unwrap();
        assert!(props.get("file_path").is_some());
    }

    #[test]
    fn write_params_schema_has_required_fields() {
        let schema = tool_schema_for::<WriteParams>();
        let required = schema.get("required").and_then(|r| r.as_array());
        assert!(required.is_some());
        let required = required.unwrap();
        assert!(required.iter().any(|v| v.as_str() == Some("file_path")));
        assert!(required.iter().any(|v| v.as_str() == Some("content")));
    }

    #[test]
    fn edit_params_schema_has_all_fields() {
        let schema = tool_schema_for::<EditParams>();
        let props = schema.get("properties");
        assert!(props.is_some());
        let props = props.unwrap();
        assert!(props.get("file_path").is_some());
        assert!(props.get("old_string").is_some());
        assert!(props.get("new_string").is_some());
        assert!(props.get("replace_all").is_some());
    }

    #[test]
    fn bash_params_deserialize() {
        let json = r#"{"command": "echo hello"}"#;
        let params: BashParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.command, "echo hello");
        assert!(params.timeout_secs.is_none());
    }

    #[test]
    fn bash_params_with_timeout() {
        let json = r#"{"command": "sleep 10", "timeout_secs": 30}"#;
        let params: BashParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.timeout_secs, Some(30));
    }

    #[test]
    fn read_params_with_offset_and_limit() {
        let json = r#"{"file_path": "/tmp/test.txt", "offset": 10, "limit": 50}"#;
        let params: ReadParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.file_path, "/tmp/test.txt");
        assert_eq!(params.offset, Some(10));
        assert_eq!(params.limit, Some(50));
    }

    #[test]
    fn edit_params_replace_all_default_false() {
        let json = r#"{"file_path": "/tmp/test.txt", "old_string": "foo", "new_string": "bar"}"#;
        let params: EditParams = serde_json::from_str(json).unwrap();
        assert!(!params.replace_all);
    }

    #[test]
    fn edit_params_replace_all_true() {
        let json = r#"{"file_path": "/tmp/test.txt", "old_string": "foo", "new_string": "bar", "replace_all": true}"#;
        let params: EditParams = serde_json::from_str(json).unwrap();
        assert!(params.replace_all);
    }

    #[test]
    fn schema_is_valid_json() {
        let bash = tool_schema_for::<BashParams>();
        let read = tool_schema_for::<ReadParams>();
        let write = tool_schema_for::<WriteParams>();
        let edit = tool_schema_for::<EditParams>();

        // All should be objects with properties
        assert!(bash.get("properties").is_some());
        assert!(read.get("properties").is_some());
        assert!(write.get("properties").is_some());
        assert!(edit.get("properties").is_some());

        // None should have $schema (stripped for tool defs)
        assert!(bash.get("$schema").is_none());
        assert!(read.get("$schema").is_none());
    }
}
