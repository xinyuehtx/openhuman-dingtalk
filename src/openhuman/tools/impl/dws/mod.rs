pub mod runtime;
pub mod sync;

use crate::openhuman::tools::traits::{PermissionLevel, Tool, ToolResult};
use async_trait::async_trait;
use serde_json::json;
use std::time::Duration;

/// Maximum dws command execution time before kill (120s to allow slow API calls).
const DWS_TIMEOUT_SECS: u64 = 120;
/// Maximum output size in bytes (1MB).
const MAX_OUTPUT_BYTES: usize = 1_048_576;

/// Build an extended PATH that includes common dws installation directories
/// AND the standard Unix system directories.
///
/// The Tauri host or `cargo run` often inherits a minimal PATH that does not
/// include user shell profile additions (e.g. `~/.qoderwork/bin`) — and on
/// macOS, when the app is launched from Finder, even `/usr/bin` and `/bin`
/// may be missing. The install script needs `curl`, `tar`, `unzip`, etc., so
/// we prepend the standard system dirs along with the dws install locations.
pub fn extended_path_for_dws() -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    let mut paths: Vec<String> = Vec::new();
    if !home.is_empty() {
        paths.push(format!("{}/.qoderwork/bin", home));
        paths.push(format!("{}/.local/bin", home));
    }
    paths.push("/usr/local/bin".to_string());
    paths.push("/opt/homebrew/bin".to_string());
    // System dirs — some launch contexts (Finder-launched .app) drop these.
    paths.push("/usr/bin".to_string());
    paths.push("/bin".to_string());
    paths.push("/usr/sbin".to_string());
    paths.push("/sbin".to_string());

    let current_path = std::env::var("PATH").unwrap_or_default();
    if current_path.is_empty() {
        paths.join(":")
    } else {
        format!("{}:{}", paths.join(":"), current_path)
    }
}

/// DingTalk Workspace CLI (dws) tool.
///
/// Executes `dws` commands to manage DingTalk product capabilities including
/// AI tables, calendar, contacts, group chats, todos, approvals, attendance,
/// documents, cloud drive, and more. See
/// <https://github.com/DingTalk-Real-AI/dingtalk-workspace-cli> for details.
///
/// The tool automatically appends `--format json` to ensure structured output
/// suitable for AI consumption.
pub struct DwsTool;

impl DwsTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for DwsTool {
    fn name(&self) -> &str {
        "dws"
    }

    fn description(&self) -> &str {
        "Execute DingTalk Workspace CLI (dws) commands to manage DingTalk services: \
         AI tables (aitable), calendar, contacts, group chats & bots (chat), \
         todos, approvals (oa), attendance, reports, DING messages, documents (doc), \
         cloud drive, AI minutes, email, online sheets, wiki, and more. \
         All commands must use dws subcommands (e.g. 'contact user search --keyword Alice'). \
         The tool automatically appends '--format json'. \
         Use 'dws schema' or 'dws <command> --help' for command discovery. \
         Authentication: run 'dws auth login' first if not authenticated."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The dws subcommand to execute (without the leading 'dws' prefix). \
                                    Examples: 'contact user search --keyword Alice', \
                                    'calendar event list', 'todo task list', 'auth status', \
                                    'schema', 'schema calendar.list_events'."
                }
            },
            "required": ["command"]
        })
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::Execute
    }

    /// Cap dws output at ~30k chars to avoid blowing the context window.
    fn max_result_size_chars(&self) -> Option<usize> {
        Some(30_000)
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        let subcommand = args
            .get("command")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing 'command' parameter"))?;

        tracing::debug!(subcommand = %subcommand, "[dws] executing command");

        // Build the full command. Automatically append --format json unless
        // the user already specified it or the command is a help/schema/auth
        // command that doesn't support it.
        let needs_format_json = !subcommand.contains("--format")
            && !subcommand.starts_with("auth login")
            && !subcommand.starts_with("schema")
            && !subcommand.ends_with("--help")
            && !subcommand.contains("--help ");

        let full_command = if needs_format_json {
            format!("dws {} --format json", subcommand)
        } else {
            format!("dws {}", subcommand)
        };

        tracing::debug!(full_command = %full_command, "[dws] resolved command");

        // Execute via shell with extended PATH so dws is discoverable even when
        // the core process was launched without full user shell profile (e.g. from
        // Tauri or launchd where ~/.qoderwork/bin is absent from PATH).
        let extended_path = extended_path_for_dws();
        tracing::debug!(extended_path = %extended_path, "[dws] using extended PATH");

        let output = tokio::time::timeout(
            Duration::from_secs(DWS_TIMEOUT_SECS),
            tokio::process::Command::new("sh")
                .arg("-c")
                .arg(&full_command)
                .env("PATH", &extended_path)
                .output(),
        )
        .await;

        match output {
            Err(_elapsed) => {
                tracing::warn!(subcommand = %subcommand, "[dws] command timed out after {DWS_TIMEOUT_SECS}s");
                Ok(ToolResult::error(format!(
                    "dws command timed out after {}s: {}",
                    DWS_TIMEOUT_SECS, subcommand
                )))
            }
            Ok(Err(exec_error)) => {
                // Binary not found or spawn failure
                let message = exec_error.to_string();
                tracing::warn!(error = %message, "[dws] failed to execute");
                if message.contains("No such file or directory")
                    || message.contains("not found")
                    || message.contains("cannot find")
                {
                    Ok(ToolResult::error(
                        "dws CLI is not installed.\n\
                         \n\
                         macOS / Linux:\n\
                         curl -fsSL https://raw.githubusercontent.com/DingTalk-Real-AI/dingtalk-workspace-cli/main/scripts/install.sh | sh\n\
                         \n\
                         Windows (PowerShell):\n\
                         irm https://raw.githubusercontent.com/DingTalk-Real-AI/dingtalk-workspace-cli/main/scripts/install.ps1 | iex\n\
                         \n\
                         Or visit: https://github.com/DingTalk-Real-AI/dingtalk-workspace-cli\n\
                         \n\
                         After installation, authenticate with: dws auth login",
                    ))
                } else {
                    Ok(ToolResult::error(format!(
                        "Failed to execute dws command: {}",
                        message
                    )))
                }
            }
            Ok(Ok(process_output)) => {
                let mut stdout = String::from_utf8_lossy(
                    &process_output.stdout[..process_output.stdout.len().min(MAX_OUTPUT_BYTES)],
                )
                .to_string();
                let stderr = String::from_utf8_lossy(
                    &process_output.stderr[..process_output.stderr.len().min(MAX_OUTPUT_BYTES)],
                )
                .to_string();

                let exit_code = process_output.status.code().unwrap_or(-1);

                tracing::debug!(
                    exit_code = exit_code,
                    stdout_len = stdout.len(),
                    stderr_len = stderr.len(),
                    "[dws] command completed"
                );

                if !process_output.status.success() {
                    // Include both stdout and stderr for error diagnostics
                    let mut error_output = String::new();
                    if !stderr.is_empty() {
                        error_output.push_str(&stderr);
                    }
                    if !stdout.is_empty() {
                        if !error_output.is_empty() {
                            error_output.push_str("\n\n");
                        }
                        error_output.push_str(&stdout);
                    }
                    if error_output.is_empty() {
                        error_output = format!("dws command exited with code {}", exit_code);
                    }
                    return Ok(ToolResult::error(error_output));
                }

                // Append stderr as a note if present (warnings, verbose logs)
                if !stderr.is_empty() {
                    stdout.push_str("\n\n[stderr]\n");
                    stdout.push_str(&stderr);
                }

                if stdout.is_empty() {
                    stdout = "Command completed successfully (no output).".to_string();
                }

                Ok(ToolResult::success(stdout))
            }
        }
    }
}
