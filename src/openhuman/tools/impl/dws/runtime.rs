//! Lightweight, RPC-friendly wrappers around the local `dws` binary.
//!
//! These power the Skills-page DWS card: detection, install, login (via a
//! freshly-spawned terminal so the user can scan / press enter), and logout.
//! They run inside the core process with an extended PATH so the binary is
//! found even when the Tauri host inherits a minimal `PATH`.

use std::time::Duration;

use serde::Serialize;

use super::extended_path_for_dws;

/// Cap any single dws / shell command at this many seconds. The install
/// script downloads a ~10 MB binary from GitHub Releases, which can be
/// slow on poor networks (China especially), so this is generous.
const COMMAND_TIMEOUT_SECS: u64 = 180;

/// Result of `which dws`-style detection plus `--version` and `auth status`.
#[derive(Debug, Clone, Serialize)]
pub struct DwsRuntimeStatus {
    /// One of `not_installed` | `not_authenticated` | `authenticated`.
    pub status: &'static str,
    /// Resolved absolute path to the dws binary, when found.
    pub dws_path: Option<String>,
    /// Version string parsed out of `dws --version`, when reported.
    pub version: Option<String>,
    /// Trimmed combined stdout/stderr of the auth-status probe (debug aid).
    pub auth_output: Option<String>,
}

/// Generic shell-execution result returned to the UI.
#[derive(Debug, Clone, Serialize)]
pub struct DwsCommandResult {
    pub success: bool,
    pub exit_code: i32,
    pub output: String,
}

/// Run a `sh -c` command with the dws-friendly extended PATH and return the
/// captured stdout/stderr as a single string. Times out after
/// `COMMAND_TIMEOUT_SECS`.
async fn run_shell(command: &str) -> DwsCommandResult {
    run_shell_named("dws.shell", command).await
}

/// Same as [`run_shell`] but accepts a label used in tracing so the install /
/// login / status commands can be told apart in the logs.
async fn run_shell_named(label: &str, command: &str) -> DwsCommandResult {
    let path = extended_path_for_dws();
    tracing::debug!(
        op = label,
        command = %command,
        path = %path,
        "[dws:runtime] spawning shell command"
    );

    // Forward HOME / USER / SHELL / LANG / TMPDIR explicitly. The install
    // script (`set -eu`) needs HOME to compute its install dir; some Tauri
    // launch contexts strip the env, leaving us with only what we set here.
    let mut cmd_builder = tokio::process::Command::new("sh");
    cmd_builder.arg("-c").arg(command).env("PATH", &path);
    for key in [
        "HOME",
        "USER",
        "LOGNAME",
        "SHELL",
        "LANG",
        "LC_ALL",
        "TMPDIR",
        // Forward proxy env so users behind a corporate / China proxy can still
        // reach github.com from the curl call inside the install script.
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "NO_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
        "no_proxy",
    ] {
        if let Ok(value) = std::env::var(key) {
            cmd_builder.env(key, value);
        }
    }
    let exec = cmd_builder.output();
    match tokio::time::timeout(Duration::from_secs(COMMAND_TIMEOUT_SECS), exec).await {
        Err(_) => {
            tracing::warn!(op = label, "[dws:runtime] command timed out");
            DwsCommandResult {
                success: false,
                exit_code: -1,
                output: format!("command timed out after {COMMAND_TIMEOUT_SECS}s"),
            }
        }
        Ok(Err(spawn_error)) => {
            tracing::warn!(op = label, error = %spawn_error, "[dws:runtime] failed to spawn shell");
            DwsCommandResult {
                success: false,
                exit_code: -1,
                output: format!("failed to spawn shell: {spawn_error}"),
            }
        }
        Ok(Ok(out)) => {
            let stdout = String::from_utf8_lossy(&out.stdout).to_string();
            let stderr = String::from_utf8_lossy(&out.stderr).to_string();
            let exit_code = out.status.code().unwrap_or(-1);
            let success = out.status.success();
            let combined = match (stdout.trim().is_empty(), stderr.trim().is_empty()) {
                (true, true) => String::new(),
                (false, true) => stdout.clone(),
                (true, false) => stderr.clone(),
                (false, false) => format!("{stdout}\n{stderr}"),
            };

            // Detailed tracing: full stdout+stderr so the user can read the
            // failure cause from openhuman.<date>.log without having to repro.
            if success {
                tracing::info!(
                    op = label,
                    exit_code = exit_code,
                    stdout = %stdout.trim(),
                    stderr = %stderr.trim(),
                    "[dws:runtime] command ok"
                );
            } else {
                tracing::warn!(
                    op = label,
                    exit_code = exit_code,
                    stdout = %stdout.trim(),
                    stderr = %stderr.trim(),
                    "[dws:runtime] command failed"
                );
            }

            DwsCommandResult {
                success,
                exit_code,
                output: combined,
            }
        }
    }
}

/// Probe well-known install dirs for the dws binary and return the first
/// path that exists. Mirrors the frontend probe so backend and frontend
/// detection agree.
async fn locate_dws() -> Option<String> {
    let probe = "which dws 2>/dev/null || command -v dws 2>/dev/null || \
         ([ -x \"$HOME/.qoderwork/bin/dws\" ] && echo \"$HOME/.qoderwork/bin/dws\") || \
         ([ -x \"/usr/local/bin/dws\" ] && echo \"/usr/local/bin/dws\") || \
         ([ -x \"$HOME/.local/bin/dws\" ] && echo \"$HOME/.local/bin/dws\") || \
         ([ -x \"/opt/homebrew/bin/dws\" ] && echo \"/opt/homebrew/bin/dws\")";
    let result = run_shell_named("dws.locate", probe).await;
    if !result.success {
        return None;
    }
    let first = result.output.lines().next().unwrap_or_default().trim();
    if first.is_empty() {
        None
    } else {
        Some(first.to_string())
    }
}

fn shell_quote(value: &str) -> String {
    if value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '/'))
    {
        value.to_string()
    } else {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}

/// Detection: which dws + --version + auth status.
pub async fn status() -> DwsRuntimeStatus {
    let Some(path) = locate_dws().await else {
        return DwsRuntimeStatus {
            status: "not_installed",
            dws_path: None,
            version: None,
            auth_output: None,
        };
    };

    // Version probe — best effort, falls back to None on any error.
    let version_cmd = format!("{} --version 2>&1", shell_quote(&path));
    let version_result = run_shell_named("dws.version", &version_cmd).await;
    let version = if version_result.success {
        version_result
            .output
            .split_whitespace()
            .find_map(|tok| {
                let stripped = tok.trim_start_matches('v');
                let mut parts = stripped.split('.');
                let (a, b, c) = (parts.next()?, parts.next()?, parts.next()?);
                if a.chars().all(|c| c.is_ascii_digit())
                    && b.chars().all(|c| c.is_ascii_digit())
                    && c.chars().all(|c| c.is_ascii_digit() || c == '-' || c == '+')
                {
                    Some(stripped.to_string())
                } else {
                    None
                }
            })
            // Last-ditch regex-y fallback for "x.y.z" embedded in arbitrary text.
            .or_else(|| {
                let s = version_result.output.as_str();
                let bytes = s.as_bytes();
                let mut i = 0;
                while i < bytes.len() {
                    if bytes[i].is_ascii_digit() {
                        let start = i;
                        let mut dots = 0;
                        while i < bytes.len()
                            && (bytes[i].is_ascii_digit() || bytes[i] == b'.')
                        {
                            if bytes[i] == b'.' {
                                dots += 1;
                            }
                            i += 1;
                        }
                        if dots >= 2 {
                            return Some(s[start..i].to_string());
                        }
                    } else {
                        i += 1;
                    }
                }
                None
            })
    } else {
        None
    };

    // Auth status probe.
    let auth_cmd = format!("{} auth status --format json 2>&1", shell_quote(&path));
    let auth_result = run_shell_named("dws.auth_status", &auth_cmd).await;
    let auth_output = auth_result.output.trim().to_string();
    let lower = auth_output.to_lowercase();

    // Try to parse the JSON output first — newer dws versions return
    // `{"authenticated": false}` with exit code 0 when logged out.
    let logged_out_from_json = serde_json::from_str::<serde_json::Value>(&auth_output)
        .ok()
        .and_then(|v| v.get("authenticated")?.as_bool())
        .map(|authenticated| !authenticated);

    let logged_out = logged_out_from_json.unwrap_or_else(|| {
        // Fallback: heuristic keyword matching for older dws versions or
        // non-JSON output.
        !auth_result.success
            || lower.contains("not logged in")
            || lower.contains("not authenticated")
            || lower.contains("unauthorized")
            || lower.contains("login required")
            || lower.contains("未登录")
    });

    DwsRuntimeStatus {
        status: if logged_out {
            "not_authenticated"
        } else {
            "authenticated"
        },
        dws_path: Some(path),
        version,
        auth_output: Some(auth_output),
    }
}

/// Run the platform-appropriate install script. Idempotent: re-running it on
/// an existing install upgrades in place.
///
/// On non-Windows hosts we download to a temp file first and pipe to `sh -s`
/// instead of `curl ... | sh` so a curl failure (network blocked, DNS, etc.)
/// surfaces as a clean error message rather than the cryptic empty output of
/// a broken pipeline. The script is still streamed straight from GitHub —
/// no permanent file is written.
pub async fn install() -> DwsCommandResult {
    if cfg!(target_os = "windows") {
        return run_shell_named(
            "dws.install",
            "powershell -Command \"irm https://raw.githubusercontent.com/DingTalk-Real-AI/dingtalk-workspace-cli/main/scripts/install.ps1 | iex\" 2>&1",
        )
        .await;
    }

    // Sanity-check that curl is reachable before trying the long install.
    // If it's missing we get a clean error in the UI rather than "exit 127".
    let curl_check = run_shell_named("dws.install.curl_check", "command -v curl").await;
    if !curl_check.success {
        return DwsCommandResult {
            success: false,
            exit_code: -1,
            output: format!(
                "未找到 `curl`，无法下载安装脚本。\n\
                 请先在终端中安装 curl，或手动运行：\n\
                 curl -fsSL https://raw.githubusercontent.com/DingTalk-Real-AI/dingtalk-workspace-cli/main/scripts/install.sh | sh\n\n\
                 调试输出：{}",
                curl_check.output
            ),
        };
    }

    // Two-step install: download to temp, then run via `sh -s` so any curl
    // failure short-circuits with a real exit code.
    let cmd = "set -e; \
        tmp=\"$(mktemp -t dws-install.XXXXXX)\" || tmp=\"/tmp/dws-install.$$\"; \
        trap 'rm -f \"$tmp\"' EXIT; \
        curl --connect-timeout 15 --max-time 60 -fsSL \
          https://raw.githubusercontent.com/DingTalk-Real-AI/dingtalk-workspace-cli/main/scripts/install.sh \
          -o \"$tmp\"; \
        sh \"$tmp\" 2>&1";
    run_shell_named("dws.install", cmd).await
}

/// Run `dws auth logout` in the background — non-interactive, no terminal needed.
pub async fn logout() -> DwsCommandResult {
    let path = locate_dws().await.unwrap_or_else(|| "dws".to_string());
    let cmd = format!("{} auth logout 2>&1", shell_quote(&path));
    run_shell_named("dws.logout", &cmd).await
}

/// `dws auth login` is interactive (browser handoff + enter-to-continue), so
/// instead of running it inline we open a fresh terminal window pointed at it.
/// The user completes login there, then comes back to the UI and clicks "刷新".
pub async fn open_login_terminal() -> DwsCommandResult {
    let path = locate_dws().await.unwrap_or_else(|| "dws".to_string());
    let inner = format!("{} auth login", shell_quote(&path));
    let cmd = if cfg!(target_os = "macos") {
        // osascript fires Terminal.app and runs `do script` in a new tab.
        format!(
            "osascript \
             -e 'tell application \"Terminal\" to do script \"{escaped}\"' \
             -e 'tell application \"Terminal\" to activate'",
            escaped = applescript_escape(&inner)
        )
    } else if cfg!(target_os = "windows") {
        format!(
            "start \"DingTalk DWS Login\" cmd /k {}",
            inner.replace('"', "\\\"")
        )
    } else {
        // Try the freedesktop default first, then a common fallback chain.
        // `bash -c "...; exec bash"` keeps the window open for the user.
        format!(
            "(x-terminal-emulator -e bash -c '{cmd}; exec bash' 2>/dev/null) || \
             (gnome-terminal -- bash -c '{cmd}; exec bash' 2>/dev/null) || \
             (konsole -e bash -c '{cmd}; exec bash' 2>/dev/null) || \
             (xterm -e bash -c '{cmd}; exec bash' 2>/dev/null) || \
             (echo 'Could not auto-spawn a terminal — please run this yourself:' && echo '  {cmd}')",
            cmd = inner.replace('\'', "'\\''")
        )
    };
    let mut result = run_shell_named("dws.open_login", &cmd).await;
    if result.success && result.output.trim().is_empty() {
        result.output = "已在新终端窗口中打开 dws auth login，请在终端完成钉钉扫码登录后回到这里点「刷新状态」。"
            .to_string();
    }
    result
}

fn applescript_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}
