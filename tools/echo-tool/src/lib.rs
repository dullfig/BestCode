wit_bindgen::generate!({
    path: "../../wit",
    world: "tool",
});

struct EchoTool;

impl Guest for EchoTool {
    fn get_metadata() -> ToolMetadata {
        ToolMetadata {
            name: "echo".into(),
            description: "Echo tool — returns the input message".into(),
            semantic_description: "A simple echo tool that repeats back whatever message \
                it receives. Useful for testing pipeline connectivity, verifying tool \
                dispatch, and debugging message flow through the system.".into(),
            request_tag: "EchoRequest".into(),
            request_schema: r#"<xs:schema>
  <xs:element name="EchoRequest">
    <xs:complexType>
      <xs:sequence>
        <xs:element name="message" type="xs:string"/>
      </xs:sequence>
    </xs:complexType>
  </xs:element>
</xs:schema>"#.into(),
            response_schema: r#"<xs:schema>
  <xs:element name="ToolResponse">
    <xs:complexType>
      <xs:sequence>
        <xs:element name="success" type="xs:boolean"/>
        <xs:element name="result" type="xs:string" minOccurs="0"/>
        <xs:element name="error" type="xs:string" minOccurs="0"/>
      </xs:sequence>
    </xs:complexType>
  </xs:element>
</xs:schema>"#.into(),
            input_json_schema: r#"{"type":"object","properties":{"message":{"type":"string","description":"The message to echo back"}},"required":["message"]}"#.into(),
        }
    }

    fn handle(request_xml: String) -> ToolResult {
        // Extract <message>...</message> from XML
        let message = extract_tag(&request_xml, "message")
            .unwrap_or_else(|| "(no message)".into());
        ToolResult {
            success: true,
            payload: format!("echo: {message}"),
        }
    }
}

export!(EchoTool);

fn extract_tag(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = xml.find(&open)? + open.len();
    let end = xml.find(&close)?;
    if start <= end {
        Some(xml[start..end].to_string())
    } else {
        None
    }
}
