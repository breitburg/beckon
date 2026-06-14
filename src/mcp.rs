// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 breitburg

//! Model Context Protocol (MCP) client support.
//!
//! MCP connections are stateful and long-lived: a server is `initialize`d once,
//! then its tools are listed and called many times against that same live peer.
//! The rest of beckon, though, has no async runtime — it builds a fresh
//! [`crate::tools::ToolRegistry`] per message and runs blocking tool executors on
//! a plain worker thread (`api.rs`). [`McpManager`] bridges the two: it owns a
//! dedicated multi-threaded tokio runtime that keeps the connections alive for
//! the life of the process, and exposes a small *synchronous* surface the
//! blocking executors call into.
//!
//! - [`McpManager::reload`] reconciles the live connections against the configured
//!   server list (fire-and-forget; connecting happens in the background).
//! - [`McpManager::snapshot`] returns the cached per-server status + tool list,
//!   read by the registry builder and the settings UI without ever blocking on
//!   the network.
//! - [`McpManager::call`] runs a tool on a connected server, blocking the caller
//!   (with a timeout) until the result comes back.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::{mpsc, oneshot};

use rmcp::model::CallToolRequestParams;
use rmcp::service::RunningService;
use rmcp::transport::{
    streamable_http_client::StreamableHttpClientTransportConfig, StreamableHttpClientTransport,
    TokioChildProcess,
};
use rmcp::{RoleClient, ServiceExt};

use crate::tools::{self, MAX_OUTPUT_BYTES};

/// How a server is reached. Stored in the config file as a lowercase tag.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum McpTransport {
    /// A local server launched as a child process, spoken to over stdio.
    Stdio,
    /// A remote server reached over the Streamable HTTP transport.
    Http,
}

fn default_true() -> bool {
    true
}

/// A configured MCP server. Persisted in `config.toml` under `[[mcp_servers]]`.
/// Only the fields relevant to the chosen `transport` are used; the others stay
/// at their defaults so a server can be switched between transports without
/// losing data.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct McpServerConfig {
    /// Unique, user-facing identifier (also the label shown in settings).
    pub name: String,
    /// Whether to connect and forward this server's tools to the model.
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub transport: McpTransport,
    /// stdio: the executable to launch.
    #[serde(default)]
    pub command: String,
    /// stdio: arguments passed to `command`.
    #[serde(default)]
    pub args: Vec<String>,
    /// stdio: extra environment variables for the child process.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// http: the server endpoint, e.g. `https://example.com/mcp`.
    #[serde(default)]
    pub url: String,
    /// http: bearer token, sent as `Authorization: Bearer …` (without the prefix
    /// here — rmcp adds it). Empty means no auth header.
    #[serde(default)]
    pub auth_token: String,
}

/// Connection state of a single enabled server, as cached in the snapshot.
#[derive(Clone)]
pub enum McpStatus {
    /// The connection/handshake is still in flight.
    Connecting,
    /// Connected; its tools are available.
    Ready,
    /// The connection or tool listing failed; the message is shown in settings.
    Error(String),
}

/// One MCP tool, flattened into the plain shape the rest of the app uses.
#[derive(Clone)]
pub struct ToolMeta {
    /// Name of the server this tool belongs to (used to route calls).
    pub server: String,
    /// The tool's own name, as the server reports it.
    pub name: String,
    pub description: String,
    /// The tool's input JSON schema, as a plain `serde_json::Value`.
    pub input_schema: Value,
}

/// A point-in-time view of one enabled server: its status and (when `Ready`) the
/// tools it exposes. Only enabled servers appear; the settings UI overlays this
/// onto the full configured list to render disabled rows.
#[derive(Clone)]
pub struct ServerState {
    pub name: String,
    pub status: McpStatus,
    pub tools: Vec<ToolMeta>,
}

/// Commands sent from the synchronous surface into the actor task that owns the
/// live connections.
enum Command {
    /// Reconcile live connections against this configured server list.
    Reload(Vec<McpServerConfig>),
    /// A background connect attempt finished (success carries the live peer).
    Connected {
        config: McpServerConfig,
        outcome: Result<(RunningService<RoleClient, ()>, Vec<ToolMeta>), String>,
    },
    /// Run `tool` on `server` with `args`; the result goes back over `reply`.
    Call {
        server: String,
        tool: String,
        args: Value,
        reply: oneshot::Sender<Result<String, String>>,
    },
}

/// Owns a tokio runtime and the live MCP connections. Cheap to clone-share via
/// `Arc`; the blocking executors built in [`crate::tools`] capture one.
pub struct McpManager {
    rt: tokio::runtime::Runtime,
    cmd_tx: mpsc::UnboundedSender<Command>,
    snapshot: Arc<Mutex<Vec<ServerState>>>,
}

/// How long a single tool call may run before the worker thread gives up. The
/// agent loop has no per-tool timeout of its own, so a hung server would
/// otherwise wedge the whole conversation.
const CALL_TIMEOUT: Duration = Duration::from_secs(30);

impl McpManager {
    /// Build the runtime and start the connection actor. No servers are
    /// connected until [`reload`](Self::reload) is called.
    pub fn new() -> Arc<Self> {
        // A couple of worker threads is ample: the work is occasional network /
        // child-process I/O, not CPU-bound, so async tasks multiplex freely. The
        // default (one thread per core) would spawn idle threads for nothing.
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("failed to build tokio runtime for MCP");
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let snapshot = Arc::new(Mutex::new(Vec::new()));

        let actor = Actor {
            clients: HashMap::new(),
            desired: HashMap::new(),
            snapshot: snapshot.clone(),
            cmd_tx: cmd_tx.clone(),
        };
        rt.spawn(actor.run(cmd_rx));

        Arc::new(McpManager {
            rt,
            cmd_tx,
            snapshot,
        })
    }

    /// Reconcile the live connections against `servers`: connect newly-enabled
    /// (or changed) servers, drop disabled/removed ones, and leave unchanged
    /// connections untouched. Returns immediately; connecting happens in the
    /// background and is observable via [`snapshot`](Self::snapshot).
    pub fn reload(&self, servers: &[McpServerConfig]) {
        let _ = self.cmd_tx.send(Command::Reload(servers.to_vec()));
    }

    /// A cheap clone of the current per-server state, for the registry builder
    /// and the settings UI. Never blocks on the network.
    pub fn snapshot(&self) -> Vec<ServerState> {
        self.snapshot.lock().unwrap().clone()
    }

    /// Run `tool` on the connected `server` with `args`, blocking until it
    /// completes, fails, or times out. The returned string (whether `Ok` or
    /// `Err`) is fed back to the model so it can recover.
    pub fn call(&self, server: &str, tool: &str, args: Value) -> Result<String, String> {
        let (reply, rx) = oneshot::channel();
        self.cmd_tx
            .send(Command::Call {
                server: server.to_string(),
                tool: tool.to_string(),
                args,
                reply,
            })
            .map_err(|_| "MCP runtime is not running".to_string())?;

        self.rt.block_on(async move {
            match tokio::time::timeout(CALL_TIMEOUT, rx).await {
                Ok(Ok(result)) => result,
                Ok(Err(_)) => Err("MCP call was dropped before completing".to_string()),
                Err(_) => Err(format!(
                    "MCP tool '{tool}' timed out after {}s",
                    CALL_TIMEOUT.as_secs()
                )),
            }
        })
    }
}

/// A live connection plus the exact config it was opened with, so a later
/// reload can tell whether the config changed and a reconnect is needed.
struct Connection {
    service: RunningService<RoleClient, ()>,
    config: McpServerConfig,
}

/// The single task that owns every live connection. All mutation of the
/// connection set and the snapshot happens here, so no locking is needed beyond
/// the snapshot mutex (which exists only to share reads with other threads).
struct Actor {
    /// Live, initialized connections by server name.
    clients: HashMap<String, Connection>,
    /// The currently-desired (enabled) servers, used to discard stale connect
    /// results from a superseded reload.
    desired: HashMap<String, McpServerConfig>,
    snapshot: Arc<Mutex<Vec<ServerState>>>,
    cmd_tx: mpsc::UnboundedSender<Command>,
}

impl Actor {
    async fn run(mut self, mut rx: mpsc::UnboundedReceiver<Command>) {
        while let Some(cmd) = rx.recv().await {
            match cmd {
                Command::Reload(servers) => self.reload(servers),
                Command::Connected { config, outcome } => self.connected(config, outcome),
                Command::Call {
                    server,
                    tool,
                    args,
                    reply,
                } => self.call(server, tool, args, reply),
            }
        }
    }

    fn reload(&mut self, servers: Vec<McpServerConfig>) {
        let desired: HashMap<String, McpServerConfig> = servers
            .into_iter()
            .filter(|s| s.enabled && !s.name.is_empty())
            .map(|s| (s.name.clone(), s))
            .collect();

        // Drop connections that are no longer wanted, or whose config changed
        // (so they get reopened below with the new settings).
        self.clients
            .retain(|name, conn| desired.get(name) == Some(&conn.config));

        // Open anything desired that isn't already connected.
        for (name, config) in &desired {
            if self.clients.contains_key(name) {
                continue;
            }
            self.set_status(name, McpStatus::Connecting, Vec::new());
            let config = config.clone();
            let cmd_tx = self.cmd_tx.clone();
            tokio::spawn(async move {
                let outcome = connect(&config).await;
                let _ = cmd_tx.send(Command::Connected { config, outcome });
            });
        }

        // Forget servers (and their cached status) that are no longer desired.
        self.snapshot
            .lock()
            .unwrap()
            .retain(|s| desired.contains_key(&s.name));
        self.desired = desired;
    }

    fn connected(
        &mut self,
        config: McpServerConfig,
        outcome: Result<(RunningService<RoleClient, ()>, Vec<ToolMeta>), String>,
    ) {
        // A later reload may have changed or removed this server while we were
        // connecting; if so, this result is stale — drop it (and its peer).
        if self.desired.get(&config.name) != Some(&config) {
            return;
        }
        match outcome {
            Ok((service, tools)) => {
                self.set_status(&config.name, McpStatus::Ready, tools);
                self.clients
                    .insert(config.name.clone(), Connection { service, config });
            }
            Err(message) => {
                self.clients.remove(&config.name);
                self.set_status(&config.name, McpStatus::Error(message), Vec::new());
            }
        }
    }

    fn call(
        &self,
        server: String,
        tool: String,
        args: Value,
        reply: oneshot::Sender<Result<String, String>>,
    ) {
        let Some(connection) = self.clients.get(&server) else {
            let _ = reply.send(Err(format!("MCP server '{server}' is not connected")));
            return;
        };
        // Clone the peer so the call runs concurrently without tying up the
        // actor (peers are cheap handles).
        let peer = connection.service.peer().clone();
        tokio::spawn(async move {
            let arguments = args.as_object().cloned().unwrap_or_default();
            let params = CallToolRequestParams::new(tool).with_arguments(arguments);
            let result = match peer.call_tool(params).await {
                Ok(result) => flatten_result(result),
                Err(err) => Err(format!("MCP call failed: {err}")),
            };
            let _ = reply.send(result);
        });
    }

    /// Upsert a server's status (and tools) in the shared snapshot.
    fn set_status(&self, name: &str, status: McpStatus, tools: Vec<ToolMeta>) {
        let mut snapshot = self.snapshot.lock().unwrap();
        if let Some(state) = snapshot.iter_mut().find(|s| s.name == name) {
            state.status = status;
            state.tools = tools;
        } else {
            snapshot.push(ServerState {
                name: name.to_string(),
                status,
                tools,
            });
        }
    }
}

/// Open and initialize a connection, then list its tools. Runs on the runtime,
/// off the actor, so a slow server can't stall command handling.
async fn connect(
    config: &McpServerConfig,
) -> Result<(RunningService<RoleClient, ()>, Vec<ToolMeta>), String> {
    let service = match config.transport {
        McpTransport::Stdio => {
            if config.command.is_empty() {
                return Err("no command set for stdio server".to_string());
            }
            let mut command = tokio::process::Command::new(&config.command);
            command.args(&config.args);
            for (key, value) in &config.env {
                command.env(key, value);
            }
            let transport =
                TokioChildProcess::new(command).map_err(|err| format!("could not start: {err}"))?;
            ().serve(transport)
                .await
                .map_err(|err| format!("initialize failed: {err}"))?
        }
        McpTransport::Http => {
            if config.url.is_empty() {
                return Err("no URL set for HTTP server".to_string());
            }
            let mut http = StreamableHttpClientTransportConfig::with_uri(config.url.clone());
            if !config.auth_token.is_empty() {
                http = http.auth_header(config.auth_token.clone());
            }
            let transport = StreamableHttpClientTransport::from_config(http);
            ().serve(transport)
                .await
                .map_err(|err| format!("initialize failed: {err}"))?
        }
    };

    let tools = service
        .list_all_tools()
        .await
        .map_err(|err| format!("could not list tools: {err}"))?;
    let metas = tools
        .into_iter()
        .map(|tool| ToolMeta {
            server: config.name.clone(),
            name: tool.name.to_string(),
            description: tool.description.as_deref().unwrap_or_default().to_string(),
            input_schema: tool.schema_as_json_value(),
        })
        .collect();
    Ok((service, metas))
}

/// Flatten a tool result into the single string the model consumes: structured
/// output verbatim when present, otherwise the text blocks joined together (with
/// a placeholder noting any non-text content). An `is_error` result is returned
/// as `Err` so the model sees the failure but the loop survives.
fn flatten_result(result: rmcp::model::CallToolResult) -> Result<String, String> {
    let mut text = if let Some(structured) = result.structured_content {
        structured.to_string()
    } else {
        result
            .content
            .iter()
            .map(|content| match content.as_text() {
                Some(text) => text.text.clone(),
                None => "[non-text content omitted]".to_string(),
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    tools::truncate(&mut text, MAX_OUTPUT_BYTES);

    if result.is_error == Some(true) {
        Err(text)
    } else {
        Ok(text)
    }
}
