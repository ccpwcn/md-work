//! Keeps the loopback MCP HTTP server (`md-work-cli mcp serve --http`) alive
//! for as long as the GUI runs, and stops it when md-work exits.

use std::path::Path;
use std::process::{Child, Command, Stdio};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub struct McpHttp {
    pub port: u16,
    child: Option<Child>,
    pub error: Option<String>,
}

impl McpHttp {
    pub fn new(port: u16) -> Self {
        Self {
            port,
            child: None,
            error: None,
        }
    }

    /// Starts `md-work-cli mcp serve --http` on `port` for `workspace`.
    ///
    /// The workspace root is passed explicitly: the child would otherwise read
    /// it from `config/runtime.toml`, which the GUI saves asynchronously, so a
    /// freshly switched workspace could serve the previous root.
    pub fn start(&mut self, port: u16, workspace: Option<&Path>) -> bool {
        self.stop();
        self.port = port;
        match std::net::TcpListener::bind(("127.0.0.1", port)) {
            Ok(listener) => drop(listener),
            Err(err) if err.kind() == std::io::ErrorKind::AddrInUse => {
                tracing::warn!(port, "无法启动 MCP 服务：端口已被占用");
                self.error = Some(format!("端口 {port} 已被占用，请换一个端口。"));
                return false;
            }
            Err(err) => {
                tracing::warn!(port, error = %err, "无法监听 MCP 端口");
                self.error = Some(format!("无法监听端口 {port}：{err}"));
                return false;
            }
        }
        let cli = match crate::app::cli_path() {
            Ok(path) => path,
            Err(message) => {
                tracing::warn!(error = %message, "无法定位 MCP 可执行文件");
                self.error = Some(message);
                return false;
            }
        };
        let mut command = Command::new(&cli);
        command
            .args(["mcp", "serve", "--http", "--port", &port.to_string()])
            .env("MDWORK_PARENT_PID", std::process::id().to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if let Some(workspace) = workspace {
            command.env("MDWORK_WORKSPACE", workspace);
        }
        #[cfg(windows)]
        command.creation_flags(CREATE_NO_WINDOW);
        match command.spawn() {
            Ok(child) => {
                tracing::info!(port, pid = child.id(), cli = %cli.display(), "MCP HTTP 服务已启动");
                self.child = Some(child);
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
                tracing::warn!(port = self.port, status = %status, "MCP 服务已退出");
                self.error = Some(format!("MCP 服务已退出（{status}）"));
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
