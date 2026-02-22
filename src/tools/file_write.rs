//! FileWriteTool — write/create files with automatic parent directory creation.

use async_trait::async_trait;
use rust_pipeline::prelude::*;
use std::path::Path;

use super::{extract_tag, ToolPeer, ToolResponse};

/// Write or create files. Auto-creates parent directories.
pub struct FileWriteTool;

#[async_trait]
impl Handler for FileWriteTool {
    async fn handle(&self, payload: ValidatedPayload, _ctx: HandlerContext) -> HandlerResult {
        let xml_str = String::from_utf8_lossy(&payload.xml);

        let path = extract_tag(&xml_str, "path").unwrap_or_default();
        if path.is_empty() {
            return Ok(HandlerResponse::Reply {
                payload_xml: ToolResponse::err("missing required <path>"),
            });
        }

        let content = extract_tag(&xml_str, "content").unwrap_or_default();

        let file_path = Path::new(&path);

        // Auto-create parent directories
        if let Some(parent) = file_path.parent() {
            if !parent.exists() {
                if let Err(e) = std::fs::create_dir_all(parent) {
                    return Ok(HandlerResponse::Reply {
                        payload_xml: ToolResponse::err(&format!(
                            "failed to create directories: {e}"
                        )),
                    });
                }
            }
        }

        let bytes = content.as_bytes();
        match std::fs::write(file_path, bytes) {
            Ok(()) => Ok(HandlerResponse::Reply {
                payload_xml: ToolResponse::ok(&format!(
                    "wrote {} bytes to {path}",
                    bytes.len()
                )),
            }),
            Err(e) => Ok(HandlerResponse::Reply {
                payload_xml: ToolResponse::err(&format!("write error: {e}")),
            }),
        }
    }
}

#[async_trait]
impl ToolPeer for FileWriteTool {
    fn name(&self) -> &str {
        "file-write"
    }

    fn description(&self) -> &str {
        "Write or create files"
    }

    fn request_schema(&self) -> &str {
        r#"<xs:schema>
  <xs:element name="FileWriteRequest">
    <xs:complexType>
      <xs:sequence>
        <xs:element name="path" type="xs:string"/>
        <xs:element name="content" type="xs:string"/>
      </xs:sequence>
    </xs:complexType>
  </xs:element>
</xs:schema>"#
    }

    fn response_schema(&self) -> &str {
        r#"<xs:schema>
  <xs:element name="ToolResponse">
    <xs:complexType>
      <xs:sequence>
        <xs:element name="success" type="xs:boolean"/>
        <xs:element name="result" type="xs:string" minOccurs="0"/>
        <xs:element name="error" type="xs:string" minOccurs="0"/>
      </xs:sequence>
    </xs:complexType>
  </xs:element>
</xs:schema>"#
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_ctx() -> HandlerContext {
        HandlerContext {
            thread_id: "t1".into(),
            from: "agent".into(),
            own_name: "file-write".into(),
        }
    }

    fn make_payload(xml: &str) -> ValidatedPayload {
        ValidatedPayload {
            xml: xml.as_bytes().to_vec(),
            tag: "FileWriteRequest".into(),
        }
    }

    fn get_result(resp: HandlerResponse) -> (bool, String) {
        match resp {
            HandlerResponse::Reply { payload_xml } => {
                let xml = String::from_utf8(payload_xml).unwrap();
                let success = xml.contains("<success>true</success>");
                let content = if success {
                    extract_tag(&xml, "result").unwrap_or_default()
                } else {
                    extract_tag(&xml, "error").unwrap_or_default()
                };
                (success, content)
            }
            _ => panic!("expected Reply"),
        }
    }

    #[tokio::test]
    async fn write_new_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.txt");
        let path_str = path.to_str().unwrap();

        let xml = format!(
            "<FileWriteRequest><path>{path_str}</path><content>hello world</content></FileWriteRequest>"
        );
        let (ok, content) = get_result(FileWriteTool.handle(make_payload(&xml), make_ctx()).await.unwrap());
        assert!(ok);
        assert!(content.contains("11 bytes"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello world");
    }

    #[tokio::test]
    async fn write_creates_parent_dirs() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("a").join("b").join("c").join("deep.txt");
        let path_str = path.to_str().unwrap();

        let xml = format!(
            "<FileWriteRequest><path>{path_str}</path><content>deep content</content></FileWriteRequest>"
        );
        let (ok, _) = get_result(FileWriteTool.handle(make_payload(&xml), make_ctx()).await.unwrap());
        assert!(ok);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "deep content");
    }

    #[tokio::test]
    async fn write_overwrites_existing() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("existing.txt");
        std::fs::write(&path, "old content").unwrap();
        let path_str = path.to_str().unwrap();

        let xml = format!(
            "<FileWriteRequest><path>{path_str}</path><content>new content</content></FileWriteRequest>"
        );
        let (ok, _) = get_result(FileWriteTool.handle(make_payload(&xml), make_ctx()).await.unwrap());
        assert!(ok);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new content");
    }

    #[tokio::test]
    async fn write_empty_content() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("empty.txt");
        let path_str = path.to_str().unwrap();

        let xml = format!(
            "<FileWriteRequest><path>{path_str}</path><content></content></FileWriteRequest>"
        );
        let (ok, content) = get_result(FileWriteTool.handle(make_payload(&xml), make_ctx()).await.unwrap());
        assert!(ok);
        assert!(content.contains("0 bytes"));
    }

    #[tokio::test]
    async fn write_missing_path() {
        let xml = "<FileWriteRequest><content>hello</content></FileWriteRequest>";
        let (ok, content) = get_result(FileWriteTool.handle(make_payload(xml), make_ctx()).await.unwrap());
        assert!(!ok);
        assert!(content.contains("missing required"));
    }

    #[tokio::test]
    async fn write_xml_entities_in_content() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("entities.txt");
        let path_str = path.to_str().unwrap();

        let xml = format!(
            "<FileWriteRequest><path>{path_str}</path><content>a &lt; b &amp; c</content></FileWriteRequest>"
        );
        let (ok, _) = get_result(FileWriteTool.handle(make_payload(&xml), make_ctx()).await.unwrap());
        assert!(ok);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "a < b & c");
    }

    #[test]
    fn file_write_metadata() {
        let tool = FileWriteTool;
        assert_eq!(tool.name(), "file-write");
        assert!(!tool.description().is_empty());
        assert!(tool.request_schema().contains("FileWriteRequest"));
    }
}
