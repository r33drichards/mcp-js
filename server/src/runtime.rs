//! Shared library facade used by embedded callers and all server transports.

use std::ops::Deref;

use serde_json::{json, Value};

use crate::engine::Engine;
use crate::mcp::{built_in_tool_catalog, ToolCatalog};

#[derive(Clone)]
pub struct McpJsRuntime {
    engine: Engine,
}

impl McpJsRuntime {
    pub fn new(engine: Engine) -> Self {
        Self { engine }
    }

    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    pub fn tool_catalog(&self) -> ToolCatalog {
        built_in_tool_catalog(self.heap_enabled(), self.fs_enabled())
    }

    pub async fn call_tool(
        &self,
        session_id: Option<&str>,
        mcp_headers: Option<&Value>,
        name: &str,
        arguments: &Value,
    ) -> Value {
        if self.session_capable() {
            crate::mcp_dispatch::call_tool(
                &self.engine,
                session_id,
                mcp_headers,
                name,
                arguments,
            )
            .await
        } else if name == "run_js" {
            crate::mcp_dispatch::run_js_blocking(&self.engine, mcp_headers, arguments).await
        } else {
            json!({ "error": format!("unknown stateless tool: {name}") })
        }
    }
}

impl From<Engine> for McpJsRuntime {
    fn from(engine: Engine) -> Self {
        Self::new(engine)
    }
}

impl Deref for McpJsRuntime {
    type Target = Engine;

    fn deref(&self) -> &Self::Target {
        &self.engine
    }
}
