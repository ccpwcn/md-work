//! Keeps the loopback MCP HTTP server (`md-work-cli mcp serve --http`) alive
//! for as long as the GUI runs, and stops it when md-work exits.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub struct McpHttp {
    pub port: u16,
    child: Option<Child>,
    stderr_log: Option<PathBuf>,
    pub error: Option<String>,
}

impl McpHttp {
    pub fn new(port: u16) -> Self {
        Self {
            port,
            child: None,
            stderr_log: None,
            error: None,
        }
    }

    /// Starts `md-work-cli mcp serve --http` on `port` for `workspace`.
    ///
    /// The workspace root and the token policy are passed explicitly: the child
    /// would otherwise read them from `config/*.toml`, which the GUI saves
    /// asynchronously, so a freshly switched setting could serve stale values.
    pub fn start(&mut self, port: u16, workspace: Option<&Path>, require_token: bool) -> bool {
        self.stop();
        self.port = port;
        // 不在父进程里预占端口：bind→drop→spawn 之间存在竞态，且子进程
        // 真正绑定失败时只靠轮询发现。直接让子进程绑定，并保留 stderr。
        let log_path =
            std::env::temp_dir().join(format!("md-work-mcp-{}-{port}.log", std::process::id()));
        let stderr = match std::fs::File::create(&log_path) {
            Ok(file) => file,
            Err(err) => {
                tracing::warn!(port, error = %err, "无法创建 MCP 日志");
                self.error = Some(format!("无法创建 MCP 日志：{err}"));
                return false;
            }
        };
        let cli = match crate::app::cli_path() {
            Ok(path) => path,
            Err(message) => {
                tracing::warn!(error = %message, "无法定位 MCP 可执行文件");
                self.error = Some(message);
                return false;
            }
        };
        let mut command = Command::new(&cli);
        let token_flag = if require_token {
            "--require-token"
        } else {
            "--no-require-token"
        };
        command
            .args([
                "mcp",
                "serve",
                "--http",
                "--port",
                &port.to_string(),
                token_flag,
            ])
            .env("MDWORK_PARENT_PID", std::process::id().to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::from(stderr));
        if let Some(workspace) = workspace {
            command.env("MDWORK_WORKSPACE", workspace);
        }
        #[cfg(windows)]
        command.creation_flags(CREATE_NO_WINDOW);
        match command.spawn() {
            Ok(child) => {
                tracing::info!(port, pid = child.id(), cli = %cli.display(), "MCP HTTP 服务已启动");
                self.child = Some(child);
                self.stderr_log = Some(log_path);
                self.error = None;
                true
            }
            Err(err) => {
                tracing::warn!(port, error = %err, "无法启动 MCP 服务");
                self.error = Some(format!("无法启动 MCP 服务：{err}"));
                false
            }
        }
    }

    fn startup_hint(&self) -> String {
        let Some(path) = &self.stderr_log else {
            return String::new();
        };
        let Ok(text) = std::fs::read_to_string(path) else {
            return String::new();
        };
        let tail: Vec<&str> = text.lines().rev().take(3).collect();
        tail.into_iter().rev().collect::<Vec<_>>().join(" | ")
    }

    pub fn stop(&mut self) {
        if let Some(mut child) = self.child.take() {
            tracing::info!(port = self.port, pid = child.id(), "停止 MCP HTTP 服务");
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    /// Checks liveness and forgets a child that already exited.
    pub fn running(&mut self) -> bool {
        let Some(child) = self.child.as_mut() else {
            return false;
        };
        match child.try_wait() {
            Ok(None) => true,
            Ok(Some(status)) => {
                let hint = self.startup_hint();
                tracing::warn!(port = self.port, status = %status, hint = %hint, "MCP 服务已退出");
                self.error = Some(if hint.is_empty() {
                    format!("MCP 服务已退出（{status}）")
                } else {
                    format!("MCP 服务已退出（{status}）：{hint}")
                });
                self.child = None;
                false
            }
            Err(err) => {
                tracing::warn!(port = self.port, error = %err, "MCP 服务状态检查失败");
                self.error = Some(err.to_string());
                self.child = None;
                false
            }
        }
    }
}

impl Drop for McpHttp {
    fn drop(&mut self) {
        self.stop();
    }
}
