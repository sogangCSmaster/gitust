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
                        "description": "Base branch to fork from. Omission requires the repository's current committed symbolic default; unborn or detached defaults require an explicit existing base."
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
                    "verify": {
                        "type": "boolean",
                        "default": false,
                        "description": "Require a clean committed checkout, run the requested command followed by required commands from the merge target's committed .gitust-checks.json, and record the exact SHA and policy for human approval. Ordinary editing commands must omit this or use false."
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
            name: "gitust_worktree_commit",
            description: "Commit explicitly listed files in your managed agent worktree using the expected HEAD SHA. Requires trusted single-user execution and SiteAdmin ownership. Does not verify, approve, merge, or publish; run explicit verification afterward.",
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "worktree_id": { "type": "string", "format": "uuid" },
                    "expected_head": { "type": "string", "pattern": "^[0-9a-fA-F]{40}$", "description": "Current branch commit SHA. A stale value is rejected; inspect the branch after an indeterminate result." },
                    "paths": { "type": "array", "minItems": 1, "maxItems": 128, "items": { "type": "string", "minLength": 1, "maxLength": 4096 }, "description": "Explicit repository-relative file paths, including deletions; directory recursion is not supported." },
                    "message": { "type": "string", "minLength": 1, "maxLength": 8192 }
                },
                "required": ["worktree_id", "expected_head", "paths", "message"],
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
