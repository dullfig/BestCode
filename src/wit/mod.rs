//! WIT-as-Schema — single source of truth for tool definitions.
//!
//! Each tool declares its interface in WIT text. Parse once at registration,
//! generate PayloadSchema (validation), ToolDefinition (LLM), and XML tag
//! mapping (routing) from the single source.

pub mod parser;

use std::collections::HashMap;

use rust_pipeline::prelude::*;

use crate::llm::types::ToolDefinition;

/// Parsed WIT interface for a tool.
#[derive(Debug, Clone)]
pub struct ToolInterface {
    /// Interface name (e.g. "file-read").
    pub name: String,
    /// Tool description from doc comments.
    pub description: String,
    /// Request record (fields the tool accepts).
    pub request: ToolRecord,
}

/// Parsed record (collection of typed fields).
#[derive(Debug, Clone)]
pub struct ToolRecord {
    pub fields: Vec<ToolField>,
}

/// A single field in a record.
#[derive(Debug, Clone)]
pub struct ToolField {
    /// Field name (e.g. "path", "offset").
    pub name: String,
    /// Field type.
    pub field_type: ToolFieldType,
    /// Description from doc comment.
    pub description: Option<String>,
}

/// WIT type system subset.
#[derive(Debug, Clone, PartialEq)]
pub enum ToolFieldType {
    String,
    Bool,
    U32,
    U64,
    S32,
    S64,
    F32,
    F64,
    Option(Box<ToolFieldType>),
    List(Box<ToolFieldType>),
}

impl ToolInterface {
    /// Generate the XML request tag name (PascalCase of interface name + "Request").
    ///
    /// Examples:
    /// - "file-read" → "FileReadRequest"
    /// - "command-exec" → "CommandExecRequest"
    /// - "glob" → "GlobRequest"
    pub fn request_tag(&self) -> String {
        let pascal = self
            .name
            .split('-')
            .map(|part| {
                let mut chars = part.chars();
                match chars.next() {
                    None => String::new(),
                    Some(c) => c.to_uppercase().to_string() + chars.as_str(),
                }
            })
            .collect::<String>();
        format!("{pascal}Request")
    }

    /// Generate a `PayloadSchema` for pipeline validation.
    ///
    /// Maps WIT types to `FieldType` variants. `option<T>` fields are
    /// marked as not required; everything else is required.
    /// Always uses `strict: false` to allow additional child elements.
    pub fn to_payload_schema(&self) -> PayloadSchema {
        let mut fields = HashMap::new();
        for field in &self.request.fields {
            let (required, field_type) = wit_to_field_schema(&field.field_type);
            fields.insert(
                wit_name_to_underscore(&field.name),
                FieldSchema {
                    required,
                    field_type,
                },
            );
        }
        PayloadSchema {
            root_tag: self.request_tag(),
            fields,
            strict: false,
        }
    }

    /// Generate a `ToolDefinition` for the Anthropic API (JSON Schema).
    ///
    /// Produces `{ type: "object", properties: {...}, required: [...] }`.
    pub fn to_tool_definition(&self) -> ToolDefinition {
        let mut properties = serde_json::Map::new();
        let mut required = Vec::new();

        for field in &self.request.fields {
            let (is_required, json_type) = wit_to_json_schema(&field.field_type);
            let field_name = wit_name_to_underscore(&field.name);

            let mut prop = serde_json::Map::new();
            prop.insert("type".into(), serde_json::Value::String(json_type));
            if let Some(ref desc) = field.description {
                prop.insert("description".into(), serde_json::Value::String(desc.clone()));
            }
            properties.insert(field_name.clone(), serde_json::Value::Object(prop));

            if is_required {
                required.push(serde_json::Value::String(field_name));
            }
        }

        let mut schema = serde_json::Map::new();
        schema.insert("type".into(), serde_json::Value::String("object".into()));
        schema.insert(
            "properties".into(),
            serde_json::Value::Object(properties),
        );
        if !required.is_empty() {
            schema.insert("required".into(), serde_json::Value::Array(required));
        }

        ToolDefinition {
            name: self.name.clone(),
            description: self.description.clone(),
            input_schema: serde_json::Value::Object(schema),
        }
    }
}

/// Convert a WIT kebab-case name to underscore (XML/JSON convention).
///
/// "old-string" → "old_string", "case-insensitive" → "case_insensitive"
fn wit_name_to_underscore(name: &str) -> String {
    name.replace('-', "_")
}

/// Map a WIT type to (required, FieldType) for PayloadSchema.
fn wit_to_field_schema(ty: &ToolFieldType) -> (bool, FieldType) {
    match ty {
        ToolFieldType::String => (true, FieldType::String),
        ToolFieldType::Bool => (true, FieldType::Boolean),
        ToolFieldType::U32 | ToolFieldType::U64 | ToolFieldType::S32 | ToolFieldType::S64 => {
            (true, FieldType::Integer)
        }
        ToolFieldType::F32 | ToolFieldType::F64 => (true, FieldType::Integer),
        ToolFieldType::Option(inner) => {
            let (_, field_type) = wit_to_field_schema(inner);
            (false, field_type) // option = not required
        }
        ToolFieldType::List(_) => (true, FieldType::String), // lists serialize as string content
    }
}

/// Map a WIT type to (required, json_schema_type_string) for ToolDefinition.
fn wit_to_json_schema(ty: &ToolFieldType) -> (bool, String) {
    match ty {
        ToolFieldType::String => (true, "string".into()),
        ToolFieldType::Bool => (true, "boolean".into()),
        ToolFieldType::U32 | ToolFieldType::U64 | ToolFieldType::S32 | ToolFieldType::S64 => {
            (true, "integer".into())
        }
        ToolFieldType::F32 | ToolFieldType::F64 => (true, "number".into()),
        ToolFieldType::Option(inner) => {
            let (_, json_type) = wit_to_json_schema(inner);
            (false, json_type) // option = not required
        }
        ToolFieldType::List(_) => (true, "array".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_interface() -> ToolInterface {
        ToolInterface {
            name: "file-read".into(),
            description: "Read file contents with optional offset/limit.".into(),
            request: ToolRecord {
                fields: vec![
                    ToolField {
                        name: "path".into(),
                        field_type: ToolFieldType::String,
                        description: Some("The file path to read".into()),
                    },
                    ToolField {
                        name: "offset".into(),
                        field_type: ToolFieldType::Option(Box::new(ToolFieldType::U32)),
                        description: Some("Starting line number (1-based, default: 1)".into()),
                    },
                    ToolField {
                        name: "limit".into(),
                        field_type: ToolFieldType::Option(Box::new(ToolFieldType::U32)),
                        description: Some("Maximum lines to read (default: 2000)".into()),
                    },
                ],
            },
        }
    }

    #[test]
    fn request_tag_simple() {
        let iface = ToolInterface {
            name: "glob".into(),
            description: String::new(),
            request: ToolRecord { fields: vec![] },
        };
        assert_eq!(iface.request_tag(), "GlobRequest");
    }

    #[test]
    fn request_tag_hyphenated() {
        let iface = sample_interface();
        assert_eq!(iface.request_tag(), "FileReadRequest");
    }

    #[test]
    fn request_tag_multi_hyphen() {
        let iface = ToolInterface {
            name: "command-exec".into(),
            description: String::new(),
            request: ToolRecord { fields: vec![] },
        };
        assert_eq!(iface.request_tag(), "CommandExecRequest");
    }

    #[test]
    fn to_payload_schema_fields() {
        let iface = sample_interface();
        let schema = iface.to_payload_schema();

        assert_eq!(schema.root_tag, "FileReadRequest");
        assert!(!schema.strict);
        assert_eq!(schema.fields.len(), 3);

        let path = &schema.fields["path"];
        assert!(path.required);
        assert_eq!(path.field_type, FieldType::String);

        let offset = &schema.fields["offset"];
        assert!(!offset.required); // option<u32> = not required
        assert_eq!(offset.field_type, FieldType::Integer);

        let limit = &schema.fields["limit"];
        assert!(!limit.required);
        assert_eq!(limit.field_type, FieldType::Integer);
    }

    #[test]
    fn to_payload_schema_validates() {
        let iface = sample_interface();
        let schema = iface.to_payload_schema();

        // Valid payload
        let xml = b"<FileReadRequest><path>/tmp/test.txt</path></FileReadRequest>";
        rust_pipeline::validation::validate_payload(xml, &schema).unwrap();

        // With optional fields
        let xml2 = b"<FileReadRequest><path>/tmp/test.txt</path><offset>5</offset><limit>10</limit></FileReadRequest>";
        rust_pipeline::validation::validate_payload(xml2, &schema).unwrap();
    }

    #[test]
    fn to_payload_schema_rejects_missing_required() {
        let iface = sample_interface();
        let schema = iface.to_payload_schema();

        // Missing required `path`
        let xml = b"<FileReadRequest><offset>5</offset></FileReadRequest>";
        assert!(rust_pipeline::validation::validate_payload(xml, &schema).is_err());
    }

    #[test]
    fn to_payload_schema_rejects_wrong_root() {
        let iface = sample_interface();
        let schema = iface.to_payload_schema();

        let xml = b"<WrongTag><path>x</path></WrongTag>";
        assert!(rust_pipeline::validation::validate_payload(xml, &schema).is_err());
    }

    #[test]
    fn to_tool_definition_structure() {
        let iface = sample_interface();
        let def = iface.to_tool_definition();

        assert_eq!(def.name, "file-read");
        assert_eq!(def.description, "Read file contents with optional offset/limit.");
        assert_eq!(def.input_schema["type"], "object");

        let props = &def.input_schema["properties"];
        assert!(props.get("path").is_some());
        assert_eq!(props["path"]["type"], "string");
        assert_eq!(props["path"]["description"], "The file path to read");

        assert!(props.get("offset").is_some());
        assert_eq!(props["offset"]["type"], "integer");

        assert!(props.get("limit").is_some());
        assert_eq!(props["limit"]["type"], "integer");

        let required = def.input_schema["required"].as_array().unwrap();
        assert_eq!(required.len(), 1);
        assert!(required.contains(&serde_json::json!("path")));
    }

    #[test]
    fn to_tool_definition_all_required() {
        let iface = ToolInterface {
            name: "file-write".into(),
            description: "Write files".into(),
            request: ToolRecord {
                fields: vec![
                    ToolField {
                        name: "path".into(),
                        field_type: ToolFieldType::String,
                        description: None,
                    },
                    ToolField {
                        name: "content".into(),
                        field_type: ToolFieldType::String,
                        description: None,
                    },
                ],
            },
        };
        let def = iface.to_tool_definition();
        let required = def.input_schema["required"].as_array().unwrap();
        assert_eq!(required.len(), 2);
    }

    #[test]
    fn to_tool_definition_no_required() {
        let iface = ToolInterface {
            name: "opt-only".into(),
            description: "All optional".into(),
            request: ToolRecord {
                fields: vec![ToolField {
                    name: "x".into(),
                    field_type: ToolFieldType::Option(Box::new(ToolFieldType::String)),
                    description: None,
                }],
            },
        };
        let def = iface.to_tool_definition();
        // No required array when everything is optional
        assert!(def.input_schema.get("required").is_none());
    }

    #[test]
    fn to_tool_definition_bool_type() {
        let iface = ToolInterface {
            name: "flag-tool".into(),
            description: "Bool test".into(),
            request: ToolRecord {
                fields: vec![ToolField {
                    name: "flag".into(),
                    field_type: ToolFieldType::Bool,
                    description: None,
                }],
            },
        };
        let def = iface.to_tool_definition();
        assert_eq!(def.input_schema["properties"]["flag"]["type"], "boolean");
    }

    #[test]
    fn to_tool_definition_float_type() {
        let iface = ToolInterface {
            name: "float-tool".into(),
            description: "Float test".into(),
            request: ToolRecord {
                fields: vec![ToolField {
                    name: "score".into(),
                    field_type: ToolFieldType::F64,
                    description: None,
                }],
            },
        };
        let def = iface.to_tool_definition();
        assert_eq!(def.input_schema["properties"]["score"]["type"], "number");
    }

    #[test]
    fn roundtrip_parse_to_definition() {
        let wit = r#"
/// Read file contents with optional offset/limit.
interface file-read {
    record request {
        /// The file path to read
        path: string,
        /// Starting line number (1-based, default: 1)
        offset: option<u32>,
        /// Maximum lines to read (default: 2000)
        limit: option<u32>,
    }
    read: func(req: request) -> result<string, string>;
}
"#;
        let iface = parser::parse_wit(wit).unwrap();
        let def = iface.to_tool_definition();

        assert_eq!(def.name, "file-read");
        assert!(def.description.contains("Read file"));
        assert_eq!(def.input_schema["properties"]["path"]["type"], "string");
        assert_eq!(def.input_schema["properties"]["offset"]["type"], "integer");
        let required = def.input_schema["required"].as_array().unwrap();
        assert!(required.contains(&serde_json::json!("path")));
        assert!(!required.contains(&serde_json::json!("offset")));
    }

    #[test]
    fn roundtrip_parse_to_schema() {
        let wit = r#"
/// Write or create files.
interface file-write {
    record request {
        /// The file path
        path: string,
        /// Content to write
        content: string,
    }
}
"#;
        let iface = parser::parse_wit(wit).unwrap();
        let schema = iface.to_payload_schema();

        assert_eq!(schema.root_tag, "FileWriteRequest");
        assert!(schema.fields["path"].required);
        assert!(schema.fields["content"].required);

        // Validate a payload against it
        let xml = b"<FileWriteRequest><path>/tmp/x</path><content>hello</content></FileWriteRequest>";
        rust_pipeline::validation::validate_payload(xml, &schema).unwrap();
    }

    #[test]
    fn to_tool_definition_serializes() {
        let iface = sample_interface();
        let def = iface.to_tool_definition();
        let json = serde_json::to_string(&def).unwrap();
        assert!(json.contains("file-read"));
        let _: serde_json::Value = serde_json::from_str(&json).unwrap();
    }
}
