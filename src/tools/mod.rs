//! Tool-Peer framework — protocol for tools as pipeline listeners.
//!
//! Tools don't think — they execute. Every tool-peer is a Handler,
//! but adds self-documenting metadata (name, description, schemas).

pub mod command_exec;
pub mod file_edit;
pub mod file_read;
pub mod file_write;
pub mod glob_tool;
pub mod grep;

use async_trait::async_trait;
use rust_pipeline::prelude::*;

/// Marker trait for tool-peers. All tool-peers are Handlers,
/// but this trait adds tool-specific metadata for self-documentation.
#[async_trait]
pub trait ToolPeer: Handler {
    /// Tool name (used in routing).
    fn name(&self) -> &str;

    /// Human-readable description.
    fn description(&self) -> &str;

    /// XML schema for this tool's request payload (self-documenting).
    fn request_schema(&self) -> &str;

    /// XML schema for this tool's response payload.
    fn response_schema(&self) -> &str;
}

/// Standard tool response envelope.
pub struct ToolResponse;

impl ToolResponse {
    /// Build a success response as XML bytes.
    pub fn ok(result: &str) -> Vec<u8> {
        format!(
            "<ToolResponse><success>true</success><result>{}</result></ToolResponse>",
            xml_escape(result)
        )
        .into_bytes()
    }

    /// Build an error response as XML bytes.
    pub fn err(error: &str) -> Vec<u8> {
        format!(
            "<ToolResponse><success>false</success><error>{}</error></ToolResponse>",
            xml_escape(error)
        )
        .into_bytes()
    }
}

/// Basic XML escaping.
pub fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Extract text content between `<tag>` and `</tag>`.
pub fn extract_tag(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = xml.find(&open)? + open.len();
    let end = xml.find(&close)?;
    if start <= end {
        Some(xml_unescape(&xml[start..end]))
    } else {
        None
    }
}

/// Unescape XML entities.
pub fn xml_unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&amp;", "&")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_response_ok() {
        let resp = ToolResponse::ok("file contents here");
        let xml = String::from_utf8(resp).unwrap();
        assert!(xml.contains("<success>true</success>"));
        assert!(xml.contains("<result>file contents here</result>"));
    }

    #[test]
    fn tool_response_err() {
        let resp = ToolResponse::err("file not found");
        let xml = String::from_utf8(resp).unwrap();
        assert!(xml.contains("<success>false</success>"));
        assert!(xml.contains("<error>file not found</error>"));
    }

    #[test]
    fn tool_response_escapes_xml() {
        let resp = ToolResponse::ok("a < b & c > d");
        let xml = String::from_utf8(resp).unwrap();
        assert!(xml.contains("a &lt; b &amp; c &gt; d"));
    }

    #[test]
    fn extract_tag_basic() {
        let xml = "<root><name>hello</name></root>";
        assert_eq!(extract_tag(xml, "name"), Some("hello".into()));
    }

    #[test]
    fn extract_tag_with_entities() {
        let xml = "<root><val>a &lt; b</val></root>";
        assert_eq!(extract_tag(xml, "val"), Some("a < b".into()));
    }

    #[test]
    fn extract_tag_missing() {
        let xml = "<root><name>hello</name></root>";
        assert_eq!(extract_tag(xml, "missing"), None);
    }
}
