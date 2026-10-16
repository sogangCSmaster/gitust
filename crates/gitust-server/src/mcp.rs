//! Built-in Model Context Protocol (MCP) JSON-RPC 2.0 Server (Step 2-3) supporting both
//! Streamable HTTP + OAuth 2.0 Bearer auth (`POST /mcp`, `POST /api/v1/mcp`) and local
//! `stdio` (`gitust mcp`).

use std::path::PathBuf;

use axum::extract::{Request, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use gitust_core::{AgentKind, AuthenticatedPrincipal, EnterpriseRole, UserId};
use gitust_db::AuthStore;
use gitust_git::GitEngine;
use gitust_sandbox::{builtin_mcp_tools, McpToolDescriptor};
use serde::Deserialize;
use uuid::Uuid;

use crate::auth::extract_principal_async;
use crate::state::{filter_authorized_worktrees, AppState, MAX_MCP_LINE_BYTES};
use crate::worktrees::{discard_worktree_core, exec_worktree_sandbox_core, spawn_worktree_core};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct McpSpawnToolArgs {
    branch: Option<String>,
    base_branch: Option<String>,
    agent: Option<String>,
    prompt_summary: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct McpSandboxExecToolArgs {
    worktree_id: Uuid,
    command: String,
    timeout_secs: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct McpWorktreeListToolArgs {}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct McpWorktreeDiscardToolArgs {
    worktree_id: Uuid,
}

fn parse_mcp_agent_kind(raw: Option<&str>) -> Result<(AgentKind, &'static str), String> {
    match raw.map(str::trim) {
        None | Some("") | Some("claude" | "claude_code" | "ClaudeCode") => {
            Ok((AgentKind::ClaudeCode, "claude"))
        }
        Some("cursor" | "cursor_agent" | "CursorAgent") => Ok((AgentKind::CursorAgent, "cursor")),
        Some("codex" | "codex_swarm" | "CodexSwarm") => Ok((AgentKind::CodexSwarm, "codex")),
        Some("custom" | "internal_bot" | "InternalBot") => Ok((AgentKind::InternalBot, "custom")),
        Some(other) => Err(format!(
            "unsupported agent '{other}'; expected one of: claude_code, cursor, codex, custom"
        )),
    }
}

fn jsonrpc_ok(id: serde_json::Value, result: serde_json::Value) -> String {
    serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": result }).to_string()
}

fn jsonrpc_err(id: serde_json::Value, code: i64, message: impl Into<String>) -> String {
    serde_json::json!({
        "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message.into() }
    })
    .to_string()
}

fn mcp_tool_ok(id: serde_json::Value, payload: serde_json::Value, is_error: bool) -> String {
    jsonrpc_ok(
        id,
        serde_json::json!({
            "content": [{ "type": "text", "text": payload.to_string() }],
            "isError": is_error,
        }),
    )
}

fn mcp_tool_http_err(id: serde_json::Value, status: StatusCode, msg: String) -> String {
    mcp_tool_ok(
        id,
        serde_json::json!({ "error": msg, "http_status": status.as_u16() }),
        true,
    )
}

fn parse_tool_args<T: serde::de::DeserializeOwned>(
    id: &serde_json::Value,
    raw_args: serde_json::Value,
) -> Result<T, String> {
    serde_json::from_value(raw_args)
        .map_err(|e| jsonrpc_err(id.clone(), -32602, format!("Invalid params: {e}")))
}

async fn resolve_default_base_branch(state: &AppState) -> String {
    let engine = state.git_engine.clone();
    match tokio::task::spawn_blocking(move || engine.inspect_head_zero_copy()).await {
        Ok(Ok(s))
            if !s.branch_name.is_empty()
                && !matches!(s.branch_name.as_str(), "HEAD" | "detached") =>
        {
            s.branch_name
        }
        _ => "main".to_owned(),
    }
}

pub(crate) async fn mcp_tools_handler(
    _caller: AuthenticatedPrincipal,
) -> Json<Vec<McpToolDescriptor>> {
    Json(builtin_mcp_tools())
}

fn json_response(status: StatusCode, body: String) -> Response {
    (status, [(header::CONTENT_TYPE, "application/json")], body).into_response()
}

/// Streamable HTTP MCP JSON-RPC 2.0 endpoint (`POST /mcp` and `POST /api/v1/mcp`).
///
/// Authenticates OAuth 2.0 Bearer sessions (`gitust_ses_...`) or PATs (`gitust_pat_...`)
/// BEFORE reading the request body (returning `401` + RFC 9728 `WWW-Authenticate` without
/// buffering unauthenticated payloads), and re-authenticates AFTER `to_bytes().await` so a
/// slow-loris body stream cannot bypass mid-flight account deactivation or token revocation.
pub(crate) async fn mcp_http_post_handler(State(state): State<AppState>, req: Request) -> Response {
    let (parts, request_body) = req.into_parts();
    match extract_principal_async(&parts.headers, &state.auth_store).await {
        Ok(_) => {}
        Err((StatusCode::UNAUTHORIZED, msg)) => {
            return (
                StatusCode::UNAUTHORIZED,
                [(
                    header::WWW_AUTHENTICATE,
                    "Bearer resource_metadata=\"/.well-known/oauth-protected-resource\", scope=\"mcp\"",
                )],
                msg,
            )
                .into_response();
        }
        Err((status, msg)) => return (status, msg).into_response(),
    }

    // fail-open-ok: oversized body immediately fails closed with 413 PAYLOAD_TOO_LARGE.
    let Ok(body) = axum::body::to_bytes(request_body, MAX_MCP_LINE_BYTES).await else {
        return json_response(
            StatusCode::PAYLOAD_TOO_LARGE,
            jsonrpc_err(
                serde_json::Value::Null,
                -32600,
                format!("MCP request body exceeds {MAX_MCP_LINE_BYTES} bytes"),
            ),
        );
    };

    let caller = match extract_principal_async(&parts.headers, &state.auth_store).await {
        Ok(c) => c,
        Err((status, msg)) => return (status, msg).into_response(),
    };

    // fail-open-ok: invalid UTF-8 immediately fails closed with 400 BAD_REQUEST.
    let Ok(line) = std::str::from_utf8(&body) else {
        return json_response(
            StatusCode::BAD_REQUEST,
            jsonrpc_err(
                serde_json::Value::Null,
                -32700,
                "Parse error: invalid UTF-8",
            ),
        );
    };

    match handle_mcp_json_line_with_options(&state, &caller, line.trim(), false).await {
        Some(resp) => json_response(StatusCode::OK, resp),
        None => StatusCode::ACCEPTED.into_response(),
    }
}

/// Processes a single newline-delimited MCP JSON-RPC 2.0 message line (with local paths enabled).
/// Returns `None` for JSON-RPC 2.0 notifications (requests without an `"id"` field).
pub async fn handle_mcp_json_line(
    state: &AppState,
    caller: &AuthenticatedPrincipal,
    line: &str,
) -> Option<String> {
    handle_mcp_json_line_with_options(state, caller, line, true).await
}

/// Processes a single MCP JSON-RPC 2.0 message.
/// When `include_local_paths` is `false` (HTTP transport), host filesystem paths (`worktree_dir`)
/// are omitted from tool responses to preserve path privacy over the network.
pub async fn handle_mcp_json_line_with_options(
    state: &AppState,
    caller: &AuthenticatedPrincipal,
    line: &str,
    include_local_paths: bool,
) -> Option<String> {
    let parsed: serde_json::Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(_) => return Some(jsonrpc_err(serde_json::Value::Null, -32700, "Parse error")), // fail-open-ok: malformed JSON immediately fails closed with JSON-RPC -32700 Parse error.
    };
    let Some(obj) = parsed.as_object() else {
        return Some(jsonrpc_err(
            serde_json::Value::Null,
            -32600,
            "Invalid Request: expected JSON object",
        ));
    };

    let has_id = obj.contains_key("id");
    let id = obj.get("id").cloned().unwrap_or(serde_json::Value::Null);
    if !matches!(
        id,
        serde_json::Value::Null | serde_json::Value::String(_) | serde_json::Value::Number(_)
    ) {
        return Some(jsonrpc_err(
            serde_json::Value::Null,
            -32600,
            "Invalid Request: id must be a string, number, or null",
        ));
    }

    let jsonrpc_ver = obj.get("jsonrpc").and_then(serde_json::Value::as_str);
    let method = obj.get("method").and_then(serde_json::Value::as_str);
    if jsonrpc_ver != Some("2.0") || method.is_none_or(str::is_empty) {
        return Some(jsonrpc_err(id, -32600, "Invalid Request"));
    }
    let method = method.unwrap_or_default(); // fail-open-ok: `method.is_none_or(str::is_empty)` above already returned `-32600 Invalid Request`.

    // Per JSON-RPC 2.0 spec, notifications (no `id` member) must not receive a response.
    if !has_id {
        return None;
    }

    let params = obj
        .get("params")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));

    match method {
        "initialize" => {
            let requested_version = params
                .get("protocolVersion")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("2024-11-05");
            let negotiated = match requested_version {
                "2025-03-26" => "2025-03-26",
                _ => "2024-11-05",
            };
            Some(jsonrpc_ok(
                id,
                serde_json::json!({
                    "protocolVersion": negotiated,
                    "capabilities": { "tools": { "listChanged": false } },
                    "serverInfo": { "name": "gitust-mcp", "version": env!("CARGO_PKG_VERSION") }
                }),
            ))
        }
        "ping" => Some(jsonrpc_ok(id, serde_json::json!({}))),
        "tools/list" => Some(jsonrpc_ok(
            id,
            serde_json::json!({ "tools": builtin_mcp_tools() }),
        )),
        "tools/call" => {
            let Some(params_obj) = params.as_object() else {
                return Some(jsonrpc_err(
                    id,
                    -32602,
                    "Invalid params: expected object for tools/call",
                ));
            };
            let Some(tool_name) = params_obj.get("name").and_then(serde_json::Value::as_str) else {
                return Some(jsonrpc_err(
                    id,
                    -32602,
                    "Invalid params: missing string field 'name'",
                ));
            };
            let raw_args = match params_obj.get("arguments") {
                None | Some(serde_json::Value::Null) => serde_json::json!({}),
                Some(v) if v.is_object() => v.clone(),
                Some(_) => {
                    return Some(jsonrpc_err(
                        id,
                        -32602,
                        "Invalid params: 'arguments' must be a JSON object",
                    ));
                }
            };

            match tool_name {
                "gitust_worktree_spawn" => {
                    let args: McpSpawnToolArgs = match parse_tool_args(&id, raw_args) {
                        Ok(a) => a,
                        Err(resp) => return Some(resp),
                    };
                    let (agent, slug) = match parse_mcp_agent_kind(args.agent.as_deref()) {
                        Ok(pair) => pair,
                        Err(e) => {
                            return Some(jsonrpc_err(id, -32602, format!("Invalid params: {e}")))
                        }
                    };
                    let branch = match args.branch.as_deref().map(str::trim) {
                        Some(b) if !b.is_empty() => b.to_owned(),
                        _ => format!("agent/{slug}/{}", Uuid::new_v4().simple()),
                    };
                    let base_branch = match args.base_branch.as_deref().map(str::trim) {
                        Some(b) if !b.is_empty() => b.to_owned(),
                        _ => resolve_default_base_branch(state).await,
                    };
                    let prompt_summary = args
                        .prompt_summary
                        .unwrap_or_else(|| "MCP agent workspace".to_owned());

                    match spawn_worktree_core(
                        state,
                        caller,
                        &branch,
                        base_branch,
                        agent,
                        prompt_summary,
                        state.repo_id,
                    )
                    .await
                    {
                        Ok((wt, worktree_dir)) => {
                            let mut payload = serde_json::json!({
                                "worktree_id": wt.id.0,
                                "branch": wt.branch,
                                "base_branch": wt.base_branch,
                                "base_commit": wt.base_commit.0,
                                "agent": wt.agent,
                                "status": wt.status,
                                "materialize_micros": wt.materialize_micros,
                            });
                            if include_local_paths {
                                if let Some(map) = payload.as_object_mut() {
                                    map.insert(
                                        "worktree_dir".to_owned(),
                                        serde_json::json!(worktree_dir),
                                    );
                                }
                            }
                            Some(mcp_tool_ok(id, payload, false))
                        }
                        Err((status, msg)) => Some(mcp_tool_http_err(id, status, msg)),
                    }
                }
                "gitust_sandbox_exec" => {
                    let args: McpSandboxExecToolArgs = match parse_tool_args(&id, raw_args) {
                        Ok(a) => a,
                        Err(resp) => return Some(resp),
                    };
                    match exec_worktree_sandbox_core(
                        state,
                        caller,
                        args.worktree_id,
                        args.command,
                        args.timeout_secs,
                    )
                    .await
                    {
                        Ok((res, status)) => Some(mcp_tool_ok(
                            id,
                            serde_json::json!({
                                "worktree_id": res.worktree_id.0,
                                "run_id": res.run_id.0,
                                "exit_code": res.exit_code,
                                "duration_ms": res.duration_ms,
                                "backend": res.backend,
                                "network_isolated": res.network_isolated,
                                "stdout": res.stdout,
                                "stderr": res.stderr,
                                "status": status,
                            }),
                            res.exit_code != 0,
                        )),
                        Err((status, msg)) => Some(mcp_tool_http_err(id, status, msg)),
                    }
                }
                "gitust_worktree_list" => {
                    let _: McpWorktreeListToolArgs = match parse_tool_args(&id, raw_args) {
                        Ok(a) => a,
                        Err(resp) => return Some(resp),
                    };
                    let authorized = match filter_authorized_worktrees(state, caller).await {
                        Ok(list) => list,
                        Err((status, msg)) => return Some(mcp_tool_http_err(id, status, msg)),
                    };
                    let runtimes = state.worktree_runtime.read();
                    let items: Vec<serde_json::Value> = authorized
                        .into_iter()
                        .map(|wt| {
                            let mut entry = serde_json::json!({
                                "worktree_id": wt.id.0,
                                "branch": wt.branch,
                                "base_branch": wt.base_branch,
                                "base_commit": wt.base_commit.0,
                                "agent": wt.agent,
                                "status": wt.status,
                                "last_sandbox_duration_ms": wt.last_sandbox_duration_ms,
                            });
                            if include_local_paths {
                                let dir = runtimes.get(&wt.id).map(|r| r.worktree_dir.clone());
                                if let Some(map) = entry.as_object_mut() {
                                    map.insert("worktree_dir".to_owned(), serde_json::json!(dir));
                                }
                            }
                            entry
                        })
                        .collect();
                    Some(mcp_tool_ok(
                        id,
                        serde_json::json!({ "worktrees": items }),
                        false,
                    ))
                }
                "gitust_worktree_discard" => {
                    let args: McpWorktreeDiscardToolArgs = match parse_tool_args(&id, raw_args) {
                        Ok(a) => a,
                        Err(resp) => return Some(resp),
                    };
                    match discard_worktree_core(state, caller, args.worktree_id).await {
                        Ok(_) => Some(mcp_tool_ok(
                            id,
                            serde_json::json!({ "worktree_id": args.worktree_id, "status": "discarded" }),
                            false,
                        )),
                        Err((status, msg)) => Some(mcp_tool_http_err(id, status, msg)),
                    }
                }
                other => Some(jsonrpc_err(
                    id,
                    -32602,
                    format!("Unknown MCP tool '{other}'"),
                )),
            }
        }
        other => Some(jsonrpc_err(
            id,
            -32601,
            format!("Method not found: '{other}'"),
        )),
    }
}

enum BoundedLine {
    Eof,
    Line(String),
    TooLong,
    InvalidUtf8,
}

async fn read_bounded_line<R: tokio::io::AsyncBufRead + Unpin>(
    reader: &mut R,
    max_bytes: usize,
) -> Result<BoundedLine, std::io::Error> {
    use tokio::io::AsyncBufReadExt;

    let mut buf = Vec::new();
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            if buf.is_empty() {
                return Ok(BoundedLine::Eof);
            }
            return Ok(String::from_utf8(buf)
                .map(BoundedLine::Line)
                .unwrap_or(BoundedLine::InvalidUtf8));
        }
        if let Some(newline_pos) = available.iter().position(|&b| b == b'\n') {
            let to_consume = newline_pos + 1;
            if buf.len().saturating_add(newline_pos) > max_bytes {
                reader.consume(to_consume);
                return Ok(BoundedLine::TooLong);
            }
            buf.extend_from_slice(&available[..newline_pos]);
            reader.consume(to_consume);
            if buf.last() == Some(&b'\r') {
                buf.pop();
            }
            return Ok(String::from_utf8(buf)
                .map(BoundedLine::Line)
                .unwrap_or(BoundedLine::InvalidUtf8));
        }
        let chunk_len = available.len();
        if buf.len().saturating_add(chunk_len) > max_bytes {
            reader.consume(chunk_len);
            loop {
                let rem = reader.fill_buf().await?;
                if rem.is_empty() {
                    break;
                }
                if let Some(pos) = rem.iter().position(|&b| b == b'\n') {
                    reader.consume(pos + 1);
                    break;
                }
                let n = rem.len();
                reader.consume(n);
            }
            return Ok(BoundedLine::TooLong);
        }
        buf.extend_from_slice(available);
        reader.consume(chunk_len);
    }
}

/// Serves the MCP JSON-RPC 2.0 protocol over an arbitrary async buffered reader and writer.
pub async fn serve_mcp_stream<R, W>(
    state: AppState,
    mut reader: R,
    mut writer: W,
) -> Result<(), std::io::Error>
where
    R: tokio::io::AsyncBufRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    use tokio::io::AsyncWriteExt;

    let caller = AuthenticatedPrincipal {
        user_id: UserId::new(),
        username: "mcp-local-agent".to_owned(),
        role: EnterpriseRole::SiteAdmin,
        team_ids: Vec::new(),
    };

    loop {
        let out_opt = match read_bounded_line(&mut reader, MAX_MCP_LINE_BYTES).await? {
            BoundedLine::Eof => break,
            BoundedLine::TooLong => Some(jsonrpc_err(
                serde_json::Value::Null,
                -32600,
                format!("MCP request line exceeds {MAX_MCP_LINE_BYTES} bytes"),
            )),
            BoundedLine::InvalidUtf8 => Some(jsonrpc_err(
                serde_json::Value::Null,
                -32700,
                "Parse error: invalid UTF-8",
            )),
            BoundedLine::Line(line) if line.trim().is_empty() => None,
            BoundedLine::Line(line) => handle_mcp_json_line(&state, &caller, line.trim()).await,
        };
        if let Some(resp) = out_opt {
            writer.write_all(resp.as_bytes()).await?;
            writer.write_all(b"\n").await?;
            writer.flush().await?;
        }
    }
    Ok(())
}

/// Starts the built-in MCP JSON-RPC 2.0 server over `stdin`/`stdout` for Claude Code / Cursor.
pub async fn serve_mcp_stdio(
    repo_path: PathBuf,
    github_mirror_url: String,
) -> Result<(), std::io::Error> {
    let (state, _service_lock) = tokio::task::spawn_blocking(move || {
        let engine = GitEngine::new(&repo_path);
        engine
            .ensure_config_has_no_credentials()
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::PermissionDenied, e))?;
        let lock = engine
            .lock_worktree_service()
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::WouldBlock, e))?;
        let state = AppState::with_engine_and_auth_store(
            engine,
            github_mirror_url,
            AuthStore::new_in_memory(),
        )
        .with_trusted_sandbox_exec(true);
        Ok::<_, std::io::Error>((state, lock))
    })
    .await
    .map_err(std::io::Error::other)??;

    let stdin = tokio::io::BufReader::new(tokio::io::stdin());
    serve_mcp_stream(state, stdin, tokio::io::stdout()).await
}
