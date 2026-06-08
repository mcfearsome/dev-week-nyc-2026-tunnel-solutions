use rmcp::{
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResult, Content, ServerCapabilities, ServerInfo},
    tool, tool_handler, tool_router, ErrorData, ServerHandler,
};
use schemars::JsonSchema;
use serde::Deserialize;

#[derive(Clone)]
pub struct Demo {
    tool_router: ToolRouter<Self>,
}

#[derive(Deserialize, JsonSchema)]
struct ReadParams {
    path: String,
}
#[derive(Deserialize, JsonSchema)]
struct ShellParams {
    cmd: String,
}

#[tool_router]
impl Demo {
    pub fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
        }
    }

    #[tool(description = "Read a UTF-8 text file and return its contents.")]
    async fn read(
        &self,
        Parameters(ReadParams { path }): Parameters<ReadParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let body = tokio::fs::read_to_string(&path)
            .await
            .map_err(|e| ErrorData::internal_error(format!("read failed: {e}"), None))?;
        Ok(CallToolResult::success(vec![Content::text(body)]))
    }

    #[tool(description = "Run a shell command and return its output.")]
    async fn shell(
        &self,
        Parameters(ShellParams { cmd }): Parameters<ShellParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let out = tokio::process::Command::new("sh")
            .arg("-c")
            .arg(&cmd)
            .output()
            .await
            .map_err(|e| ErrorData::internal_error(format!("spawn failed: {e}"), None))?;
        let mut s = String::from_utf8_lossy(&out.stdout).into_owned();
        if !out.stderr.is_empty() {
            s.push_str(&String::from_utf8_lossy(&out.stderr));
        }
        Ok(CallToolResult::success(vec![Content::text(s)]))
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for Demo {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions("Sample MCP server exposing read + shell.".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn read_returns_file_contents() {
        let p = std::env::temp_dir().join("tnls-demo-read-test.txt");
        tokio::fs::write(&p, "hello-file").await.unwrap();
        let out = Demo::new()
            .read(Parameters(ReadParams {
                path: p.to_str().unwrap().into(),
            }))
            .await
            .unwrap();
        assert_eq!(out.content[0].as_text().unwrap().text, "hello-file");
    }

    #[tokio::test]
    async fn shell_executes() {
        let out = Demo::new()
            .shell(Parameters(ShellParams {
                cmd: "echo hi".into(),
            }))
            .await
            .unwrap();
        assert_eq!(out.content[0].as_text().unwrap().text.trim(), "hi");
    }
}
