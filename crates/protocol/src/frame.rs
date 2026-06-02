use serde::{Deserialize, Serialize};
use serde_json::Value;

/// MCP tool shape, passed through to the viewer. `inputSchema` stays camelCase on the wire.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Tool {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(rename = "inputSchema", default)]
    pub input_schema: Value,
}

/// viewer -> agent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ViewerFrame {
    Hello { token: String },
    List,
    Call { id: u64, tool: String, #[serde(default)] args: Value },
}

/// agent -> viewer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum AgentFrame {
    Ready { scope: Vec<String>, expires_in_ms: u64 },
    Tools { tools: Vec<Tool> },
    Result { id: u64, content: Value },
    Error {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<u64>,
        code: ErrorCode,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tool: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    Unauthorized,
    Expired,
    OutOfScope,
    ToolError,
    BadRequest,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn hello_parses_from_viewer_json() {
        let v: ViewerFrame = serde_json::from_str(r#"{"type":"hello","token":"t"}"#).unwrap();
        assert_eq!(v, ViewerFrame::Hello { token: "t".into() });
    }

    #[test]
    fn call_defaults_missing_args() {
        let v: ViewerFrame = serde_json::from_str(r#"{"type":"call","id":7,"tool":"read"}"#).unwrap();
        assert_eq!(v, ViewerFrame::Call { id: 7, tool: "read".into(), args: Value::Null });
    }

    #[test]
    fn error_code_serializes_snake_case() {
        let f = AgentFrame::Error { id: Some(7), code: ErrorCode::OutOfScope, tool: Some("shell".into()), message: None };
        let s = serde_json::to_string(&f).unwrap();
        assert!(s.contains(r#""type":"error""#), "got {s}");
        assert!(s.contains(r#""code":"out_of_scope""#), "got {s}");
        assert!(!s.contains(r#""message""#), "None fields must be omitted: {s}");
    }

    #[test]
    fn tool_roundtrips_with_camelcase_schema() {
        let t = Tool { name: "read".into(), description: Some("Read a file".into()), input_schema: json!({"type":"object"}) };
        let s = serde_json::to_string(&t).unwrap();
        assert!(s.contains(r#""inputSchema""#), "got {s}");
        assert_eq!(serde_json::from_str::<Tool>(&s).unwrap(), t);
    }
}
