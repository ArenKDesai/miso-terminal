//! The assistant behind ASK: Claude, answering questions about MISO and the
//! markets with the terminal's own data.
//!
//! - [`api`]: the Messages API over [`mt_data`] (streaming, retries, prompt
//!   caching, server-side fallback on refusals), behind the [`Provider`] seam.
//! - [`conversation`]: the tool loop for one question, with caps on rounds,
//!   tokens and money, and cancelling.
//! - [`tools`]: the read-only [`ToolBox`] the loop (and the MCP server) calls,
//!   with input checking.
//! - [`mcp`]: the same tools over the Model Context Protocol (stdio), for
//!   Claude Desktop and Claude Code.
//! - [`usage`]: tokens, costs and the monthly ledger; [`config`]: `[ai]`.
//!
//! No UI and no writes: tools get data, never an order desk or app commands.

pub mod api;
pub mod config;
pub mod conversation;
pub mod mcp;
pub mod tools;
pub mod usage;

pub use api::{
    AiError, Anthropic, KeyCheckQuery, KeyStatus, Message, Provider, Role, StopReason, Turn,
    TurnRequest,
};
pub use config::{AiConfig, Effort, ModelInfo};
pub use conversation::{Allowance, Cancel, Conversation, Ending, Outcome, Progress, ask};
pub use tools::{NoTools, ToolBox, ToolResult, ToolSpec};
pub use usage::{Ledger, Usage};
