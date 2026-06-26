use crate::FunctionCallError;
use crate::ToolName;
use crate::ToolOutput;
use crate::ToolSearchInfo;
use crate::ToolSpec;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

/// The boxed future returned by [`ToolExecutor::handle`].
pub type ToolExecutorFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Box<dyn ToolOutput>, FunctionCallError>> + Send + 'a>>;

/// Result type returned by `CoreToolRuntime::call`.
pub type ToolCallResult = Result<Box<dyn ToolOutput>, FunctionCallError>;

/// Shared runtime contract for tool handlers that implement `call()`.
pub trait CoreToolRuntime: Send + Sync {
    fn call(
        &self,
        args: serde_json::Value,
        context: Arc<dyn ToolCallContext>,
    ) -> ToolCallResult;

    /// The name of the tool (e.g. "read_file", "grep").
    fn tool_name(&self) -> String;

    /// Human-readable description of what this tool does.
    fn tool_description(&self) -> String;

    /// JSON Schema for the tool's input arguments.
    fn tool_input_schema(&self) -> serde_json::Value;
}

/// Context passed to tool handlers during `call`.
pub trait ToolCallContext: Send + Sync {}

// Re-export ToolOutput so it's accessible via codex_tools::tool_executor::ToolOutput
pub use crate::tool_output::ToolOutput;

/// Controls where a tool is exposed to the model.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ToolExposure {
    /// Include this tool in the initial model-visible tool list.
    ///
    /// When code mode is enabled, this tool is also available as a nested
    /// code-mode tool.
    Direct,

    /// Register this tool for later discovery, but omit it from the initial
    /// model-visible tool list. Deferred tools must provide search metadata via
    /// [`ToolExecutor::search_info`]. The default implementation derives
    /// metadata from function and namespace specs.
    Deferred,

    /// Include this tool in the initial model-visible tool list only.
    ///
    /// In code-mode-only sessions, this keeps the tool callable as a normal
    /// model tool while excluding it from the nested code-mode tool surface.
    DirectModelOnly,

    /// Keep this tool registered for dispatch without exposing it to the model.
    Hidden,
}

impl ToolExposure {
    pub fn is_direct(self) -> bool {
        matches!(self, Self::Direct | Self::DirectModelOnly)
    }
}

/// Shared runtime contract for model-visible tools.
///
/// Implementations keep the model-visible spec tied to the executable runtime.
/// Host crates can layer routing, hooks, telemetry, or other orchestration on
/// top without reopening the spec/runtime split.
pub trait ToolExecutor<Invocation>: Send + Sync {
    /// The concrete tool name handled by this runtime instance.
    fn tool_name(&self) -> ToolName;

    fn spec(&self) -> ToolSpec;

    fn exposure(&self) -> ToolExposure {
        ToolExposure::Direct
    }

    fn search_info(&self) -> Option<ToolSearchInfo> {
        let spec = self.spec();
        ToolSearchInfo::from_tool_spec(spec, /*source_info*/ None)
    }

    fn supports_parallel_tool_calls(&self) -> bool {
        false
    }

    fn handle(&self, invocation: Invocation) -> ToolExecutorFuture<'_>;
}
