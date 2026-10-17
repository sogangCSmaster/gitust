//! Bounded local MCP framing and repository service ownership.
use super::{commit::CommitAuthority, handle_mcp_json_line_authorized, jsonrpc_err};
use crate::state::{AppState, MAX_MCP_LINE_BYTES};
use gitust_core::{AuthenticatedPrincipal, EnterpriseRole, UserId};
use gitust_db::AuthStore;
use gitust_git::GitEngine;
use std::path::PathBuf;

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
                // fail-open-ok: decoding failure becomes an explicit InvalidUtf8 frame that the reader rejects.
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
                // fail-open-ok: decoding failure becomes an explicit InvalidUtf8 frame that the reader rejects.
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

    let automatic_checks = crate::automatic_checks::start(&state).await?;

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
            BoundedLine::Line(line) => {
                handle_mcp_json_line_authorized(
                    &state,
                    &caller,
                    line.trim(),
                    true,
                    CommitAuthority::LocalOperator,
                )
                .await
            }
        };
        if let Some(resp) = out_opt {
            writer.write_all(resp.as_bytes()).await?;
            writer.write_all(b"\n").await?;
            writer.flush().await?;
        }
    }
    automatic_checks.shutdown().await?;
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
        .map_err(std::io::Error::other)?
        .with_trusted_sandbox_exec(true);
        Ok::<_, std::io::Error>((state, lock))
    })
    .await
    .map_err(std::io::Error::other)??;

    let stdin = tokio::io::BufReader::new(tokio::io::stdin());
    serve_mcp_stream(state, stdin, tokio::io::stdout()).await
}
