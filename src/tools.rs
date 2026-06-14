// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 breitburg

//! Tool (function-calling) definitions for the Responses API.
//!
//! A [`Tool`] bundles the JSON schema the model sees with a blocking executor
//! that runs on the API worker thread. Executors must be `Send` because the
//! agent loop lives off the GLib main context; they take the model's parsed
//! arguments and return either an output string or an error string (which is
//! still fed back to the model as the call's output, so it can recover rather
//! than the loop aborting).

use std::process::Command;
use std::sync::Arc;

use serde_json::{json, Value};

use crate::calendar;
use crate::mail;
use crate::mcp::{McpManager, McpStatus};

/// A blocking tool executor. Receives the model's `arguments` already parsed
/// into a `Value` object; returns the output string sent back to the model, or
/// an error string (also surfaced to the model as the output, so it can retry).
type Executor = Box<dyn Fn(Value) -> Result<String, String> + Send + Sync>;

pub struct Tool {
    pub name: String,
    pub description: String,
    /// JSON schema for the `parameters` object the model fills in.
    pub parameters: Value,
    executor: Executor,
}

impl Tool {
    pub(crate) fn new(
        name: &str,
        description: &str,
        parameters: Value,
        executor: impl Fn(Value) -> Result<String, String> + Send + Sync + 'static,
    ) -> Self {
        Tool {
            name: name.to_string(),
            description: description.to_string(),
            parameters,
            executor: Box::new(executor),
        }
    }

    /// The tool's definition as it appears in the Responses API `tools` array.
    /// Responses uses a flat function-tool shape (unlike Chat Completions,
    /// which nests the fields under a `function` key).
    fn definition(&self) -> Value {
        json!({
            "type": "function",
            "name": self.name,
            "description": self.description,
            "parameters": self.parameters,
        })
    }
}

/// The set of tools available to a conversation. Cheap to build; passed by
/// value into the worker thread (it is `Send`).
#[derive(Default)]
pub struct ToolRegistry {
    tools: Vec<Tool>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        ToolRegistry { tools: Vec::new() }
    }

    fn register(&mut self, tool: Tool) {
        self.tools.push(tool);
    }

    /// The `tools` array for the request body, or `None` when empty (so the
    /// request omits the field entirely rather than sending `[]`).
    pub fn definitions(&self) -> Option<Value> {
        if self.tools.is_empty() {
            return None;
        }
        Some(Value::Array(self.tools.iter().map(Tool::definition).collect()))
    }

    /// Run the named tool with `arguments` (the model's argument string, already
    /// parsed). Unknown tools produce an error string fed back to the model so
    /// it can correct itself rather than hanging the loop.
    pub fn dispatch(&self, name: &str, arguments: Value) -> Result<String, String> {
        match self.tools.iter().find(|t| t.name == name) {
            Some(tool) => (tool.executor)(arguments),
            None => Err(format!("unknown tool: {name}")),
        }
    }
}

/// One toolset the user can enable, as shown in the settings checkboxes. A
/// toolset is a named group of one or more [`Tool`]s toggled together; the UI
/// lists toolsets without constructing their executors.
pub struct ToolsetInfo {
    /// Stable identifier, stored in `config.enabled_toolsets` and used to build
    /// the toolset's tools.
    pub name: &'static str,
    /// Human-readable label for the checkbox.
    pub label: &'static str,
    /// Tooltip describing what the toolset does (and any risk).
    pub description: &'static str,
    /// Name of a built-in (themed) icon shown beside the label in settings.
    pub icon: &'static str,
}

/// Every toolset the user can enable. Adding a toolset means adding a
/// `ToolsetInfo` here and a matching arm in [`build_toolset`].
pub fn catalog() -> &'static [ToolsetInfo] {
    &[
        ToolsetInfo {
            name: "bash",
            label: "Shell",
            description: "Lets the assistant run shell commands on your machine. Powerful — only enable if you trust the model and endpoint.",
            icon: "utilities-terminal-symbolic",
        },
        ToolsetInfo {
            name: "calendar",
            label: "Calendar",
            description: "Lets the assistant read your calendar and create, modify, and delete events via Evolution Data Server. It can change your real calendar.",
            icon: "io.elementary.calendar",
        },
        ToolsetInfo {
            name: "mail",
            label: "Mail",
            description: "Lets the assistant search your locally-synced mail (subject, sender, recipients, date) via the Evolution/elementary Mail cache. Read-only.",
            icon: "io.elementary.mail",
        },
    ]
}

/// Build a registry holding the tools of every enabled built-in toolset plus
/// the tools of every connected MCP server. Unknown toolset names (e.g. one
/// removed in a later version) are ignored.
pub fn registry_for(enabled: &[String], mcp: &Arc<McpManager>) -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    for name in enabled {
        for tool in build_toolset(name) {
            registry.register(tool);
        }
    }
    register_mcp_tools(&mut registry, mcp);
    registry
}

/// Fold the tools of every `Ready` MCP server into `registry`. Each MCP tool is
/// exposed under its own name; should two servers (or a built-in) collide on a
/// name, the later one is exposed as `{tool}__{server}` so dispatch stays
/// unambiguous. The executor always routes to the original `(server, tool)`
/// regardless of the exposed name.
fn register_mcp_tools(registry: &mut ToolRegistry, mcp: &Arc<McpManager>) {
    for server in mcp.snapshot() {
        if !matches!(server.status, McpStatus::Ready) {
            continue;
        }
        for tool in server.tools {
            let mut exposed = tool.name.clone();
            if registry.tools.iter().any(|t| t.name == exposed) {
                exposed = format!("{}__{}", tool.name, sanitize(&tool.server));
            }
            let mcp = mcp.clone();
            let server_name = tool.server.clone();
            let tool_name = tool.name.clone();
            registry.register(Tool::new(
                &exposed,
                &tool.description,
                tool.input_schema.clone(),
                move |args| mcp.call(&server_name, &tool_name, args),
            ));
        }
    }
}

/// Reduce a server name to characters safe inside a tool identifier, so a
/// collision-disambiguating suffix stays a valid function name for the model.
fn sanitize(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

/// Construct the tools belonging to the named toolset. Adding a toolset means
/// adding an arm here (and a `ToolsetInfo` in [`catalog`]). An unknown name
/// yields no tools.
fn build_toolset(name: &str) -> Vec<Tool> {
    match name {
        "bash" => vec![bash_tool()],
        "calendar" => calendar::tools(),
        "mail" => mail::tools(),
        _ => Vec::new(),
    }
}

/// Output past this many bytes is truncated, so a runaway command can't bloat
/// every subsequent request in the conversation.
pub(crate) const MAX_OUTPUT_BYTES: usize = 10 * 1024;

/// Run a shell command via `sh -c` and return its combined output. Captures
/// both stdout and stderr and always appends the exit status, so the model
/// sees failures as data it can act on rather than as an error.
fn bash_tool() -> Tool {
    Tool::new(
        "bash",
        "Run a shell command via `sh -c` and return its combined stdout and stderr.",
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The shell command to execute."
                }
            },
            "required": ["command"],
            "additionalProperties": false
        }),
        |args| {
            let command = args["command"]
                .as_str()
                .ok_or("`command` must be a string")?;
            let output = Command::new("sh")
                .arg("-c")
                .arg(command)
                .output()
                .map_err(|err| format!("failed to run command: {err}"))?;

            let mut combined = String::new();
            combined.push_str(&String::from_utf8_lossy(&output.stdout));
            let stderr = String::from_utf8_lossy(&output.stderr);
            if !stderr.is_empty() {
                if !combined.is_empty() && !combined.ends_with('\n') {
                    combined.push('\n');
                }
                combined.push_str(&stderr);
            }
            truncate(&mut combined, MAX_OUTPUT_BYTES);
            match output.status.code() {
                Some(code) => combined.push_str(&format!("\n[exit status: {code}]")),
                None => combined.push_str("\n[terminated by signal]"),
            }
            Ok(combined)
        },
    )
}

/// Truncate `text` to at most `max_bytes`, on a char boundary, appending a note
/// when anything was dropped.
pub(crate) fn truncate(text: &mut String, max_bytes: usize) {
    if text.len() <= max_bytes {
        return;
    }
    let mut end = max_bytes;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
    text.push_str("\n[output truncated]");
}
