//! Built-in Model Context Protocol (MCP) tool definitions (replaces `container-use`).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolDescriptor {
    pub name: &'static str,
    pub description: &'static str,
    #[serde(rename = "inputSchema")]
    pub input_schema: serde_json::Value,
}

#[must_use]
pub fn builtin_mcp_tools() -> Vec<McpToolDescriptor> {
    vec![
        McpToolDescriptor {
            name: "gitust_worktree_spawn",
            description: "Create a new agent/ branch and an isolated Git linked worktree for an AI coding agent without touching the main working tree.",
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "branch": {
                        "type": "string",
                        "description": "Agent branch name under 'agent/' (e.g. 'agent/claude/fix-auth'). Auto-generated if omitted."
                    },
                    "base_branch": {
                        "type": "string",
                        "description": "Base branch to fork from (defaults to the repository's current HEAD branch, or 'main')."
                    },
                    "agent": {
                        "type": "string",
                        "enum": ["claude_code", "cursor", "codex", "custom"],
                        "description": "Coding agent kind (defaults to 'claude_code')."
                    },
                    "prompt_summary": {
                        "type": "string",
                        "description": "Short summary of the task assigned to this worktree."
                    }
                },
                "additionalProperties": false
            }),
        },
        McpToolDescriptor {
            name: "gitust_sandbox_exec",
            description: "Run build, test, or lint commands inside an agent worktree under an OS sandbox (network denied, writes confined to the worktree).",
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "worktree_id": {
                        "type": "string",
                        "description": "UUID of the target agent worktree."
                    },
                    "command": {
                        "type": "string",
                        "description": "Shell command to execute inside the sandboxed worktree."
                    },
                    "timeout_secs": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 600,
                        "description": "Wall-clock timeout in seconds (1..=600, default 120)."
                    }
                },
                "required": ["worktree_id", "command"],
                "additionalProperties": false
            }),
        },
        McpToolDescriptor {
            name: "gitust_worktree_list",
            description: "List active agent worktrees managed by Gitust in this repository.",
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
        },
        McpToolDescriptor {
            name: "gitust_worktree_discard",
            description: "Remove an agent worktree checkout (preserving the agent/ branch ref if commits were made ahead of base_commit).",
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "worktree_id": {
                        "type": "string",
                        "description": "UUID of the agent worktree to discard."
                    }
                },
                "required": ["worktree_id"],
                "additionalProperties": false
            }),
        },
    ]
}
