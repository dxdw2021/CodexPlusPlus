use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Serialize;

/// Dream Skin 状态信息
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DreamSkinStatus {
    pub base_theme_installed: bool,
    pub injector_running: bool,
    pub injector_pid: Option<u32>,
    pub port: u16,
    pub message: String,
}

/// 查找 assets 目录下的 dream-skin 文件
pub fn dream_skin_assets_dir() -> PathBuf {
    // 开发模式下，相对于项目根目录
    let dev_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.join("assets").join("dream-skin"))
        .unwrap_or_default();

    if dev_path.exists() {
        return dev_path;
    }

    // 生产模式下，相对于可执行文件
    let exe_path = std::env::current_exe()
        .unwrap_or_default()
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_default()
        .join("assets")
        .join("dream-skin");

    exe_path
}

/// 检查基础主题是否已安装到 config.toml
pub fn check_base_theme_installed() -> bool {
    let home = crate::codex_home::default_codex_home_dir();
    let config_path = home.join("config.toml");
    let contents = match std::fs::read_to_string(&config_path) {
        Ok(c) => c,
        Err(_) => return false,
    };

    // 检查是否包含 Dream Skin 主题标记（任意一个即可）
    contents.contains("appearanceLightChromeTheme")
        && (contents.contains("#B65CFF") || contents.contains("appearanceTheme"))
}

/// 检查 injector 是否正在运行
fn check_injector_running() -> (bool, Option<u32>) {
    let state_root = state_root_dir();
    let state_path = state_root.join("state.json");

    let state = match std::fs::read_to_string(&state_path) {
        Ok(c) => c,
        Err(_) => return (false, None),
    };

    let parsed: serde_json::Value = match serde_json::from_str(&state) {
        Ok(v) => v,
        Err(_) => return (false, None),
    };

    let pid = parsed
        .get("injectorPid")
        .and_then(|v| v.as_u64())
        .map(|p| p as u32);
    let platform = parsed
        .get("platform")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if platform != "windows" {
        return (false, pid);
    }

    if let Some(pid) = pid {
        #[cfg(windows)]
        {
            let running = std::process::Command::new("tasklist")
                .args(&["/FI", &format!("PID eq {}", pid), "/NH"])
                .output()
                .map(|o| {
                    let out = String::from_utf8_lossy(&o.stdout);
                    out.contains(&format!("{}", pid))
                })
                .unwrap_or(false);
            return (running, Some(pid));
        }
        #[cfg(not(windows))]
        {
            return (false, Some(pid));
        }
    }

    (false, None)
}

fn state_root_dir() -> PathBuf {
    std::env::var("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("CodexDreamSkin")
}

/// 查找 Node.js 运行时路径
pub fn find_node() -> Option<PathBuf> {
    // 尝试 PATH 中的 node
    let output = std::process::Command::new("node")
        .arg("--version")
        .output()
        .ok()?;
    if output.status.success() {
        return Some(PathBuf::from("node"));
    }
    None
}

/// 获取 Dream Skin 状态
pub fn get_dream_skin_status() -> DreamSkinStatus {
    let base_theme_installed = check_base_theme_installed();
    let (injector_running, injector_pid) = check_injector_running();
    let port = 9335;

    DreamSkinStatus {
        base_theme_installed,
        injector_running,
        injector_pid,
        port,
        message: if injector_running {
            "Codex 主题已激活 ✓".to_string()
        } else if base_theme_installed {
            "基础主题已安装，点击「重启 Codex++」🚀 自动注入主题".to_string()
        } else {
            "Codex 主题未安装，请先安装基础主题".to_string()
        },
    }
}

/// 安装 Dream Skin 基础主题
pub fn install_dream_skin() -> anyhow::Result<DreamSkinStatus> {
    let home = crate::codex_home::default_codex_home_dir();
    crate::relay_config::apply_dream_skin_base_theme(&home)?;
    Ok(get_dream_skin_status())
}

/// 恢复 Dream Skin 基础主题
pub fn restore_dream_skin_base() -> anyhow::Result<DreamSkinStatus> {
    let home = crate::codex_home::default_codex_home_dir();
    crate::relay_config::restore_dream_skin_base_theme(&home)?;
    Ok(get_dream_skin_status())
}

/// 全局互斥锁，防止并发注入
static INJECTION_LOCK: std::sync::OnceLock<std::sync::Mutex<bool>> = std::sync::OnceLock::new();
fn injection_lock() -> &'static std::sync::Mutex<bool> {
    INJECTION_LOCK.get_or_init(|| std::sync::Mutex::new(false))
}

/// 启动 Dream Skin 注入（连接到已启动的 Codex CDP 端点，注入 CSS/JS）
pub fn start_dream_skin_injector(port: u16) -> anyhow::Result<DreamSkinStatus> {
    // 获取互斥锁，防止并发注入
    let mut lock = injection_lock().lock().unwrap();
    if *lock {
        anyhow::bail!("Dream Skin 注入正在运行中，请等待完成");
    }
    *lock = true;
    drop(lock);

    // 只清理旧 injector 进程，不杀 Codex（Codex 由 CodexPlusPlus 启动流程管理）
    let (_, old_pid) = check_injector_running();
    if let Some(pid) = old_pid {
        #[cfg(windows)]
        {
            let _ = std::process::Command::new("taskkill")
                .args(&["/F", "/PID", &pid.to_string()])
                .output();
        }
    }
    // 删除旧的 state.json
    let state_root = state_root_dir();
    let _ = std::fs::remove_file(state_root.join("state.json"));

    let assets_dir = dream_skin_assets_dir();
    let injector_script = assets_dir.join("scripts").join("injector.mjs");
    let css_path = assets_dir.join("dream-skin.css");
    let js_path = assets_dir.join("renderer-inject.js");

    if !injector_script.exists() {
        *injection_lock().lock().unwrap() = false;
        anyhow::bail!("injector.mjs 未找到: {}", injector_script.display());
    }
    if !css_path.exists() {
        *injection_lock().lock().unwrap() = false;
        anyhow::bail!("dream-skin.css 未找到: {}", css_path.display());
    }
    if !js_path.exists() {
        *injection_lock().lock().unwrap() = false;
        anyhow::bail!("renderer-inject.js 未找到: {}", js_path.display());
    }

    // 安装基础主题
    let home = crate::codex_home::default_codex_home_dir();
    crate::relay_config::apply_dream_skin_base_theme(&home)?;

    // 查找 Node.js
    let node = find_node().ok_or_else(|| anyhow::anyhow!("未找到 Node.js，请先安装 Node.js 22+"))?;

    // 创建 state 目录
    let state_root = state_root_dir();
    std::fs::create_dir_all(&state_root)?;

    // 等待 CDP 端点可用（最多等 45 秒—Codex 启动可能需要较长时间）
    let cdp_available = wait_for_cdp_endpoint(port, Duration::from_secs(45))?;
    if !cdp_available {
        anyhow::bail!("Codex CDP 端点未在 45 秒内就绪，请确认 Codex 已启动且端口 {} 正确", port);
    }

    // 获取 CDP browser ID
    let browser_id = fetch_cdp_browser_id(port)?;

    // 启动 injector 守护进程
    let injector_path = node.to_string_lossy().to_string();
    let injector_args = vec![
        injector_script.to_string_lossy().to_string(),
        "--watch".to_string(),
        "--port".to_string(),
        port.to_string(),
        "--browser-id".to_string(),
        browser_id.clone(),
    ];

    let state_root_str = state_root.to_string_lossy().to_string();
    let stdout_path = format!("{}\\injector.log", state_root_str);
    let stderr_path = format!("{}\\injector-error.log", state_root_str);

    let injector_stdout = std::fs::File::create(&stdout_path)?;
    let injector_stderr = std::fs::File::create(&stderr_path)?;

    let mut injector_child = std::process::Command::new(&injector_path)
        .args(&injector_args)
        .stdout(injector_stdout)
        .stderr(injector_stderr)
        .spawn()
        .map_err(|e| anyhow::anyhow!("启动 injector 失败: {}", e))?;

    let injector_pid = injector_child.id();

    // 等待 injector 启动
    std::thread::sleep(Duration::from_secs(2));
    if let Ok(Some(exit_code)) = injector_child.try_wait() {
        // injector 已退出，读取错误日志
        let stderr = std::fs::read_to_string(&stderr_path).unwrap_or_default();
        anyhow::bail!("injector 启动后立即退出 (exit code: {})。错误: {}", exit_code, stderr);
    }

    // 保存状态到 state.json
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let now_str = format!("2026-07-17T{:02}:{:02}:{:02}+00:00",
        (now / 3600) % 24, (now / 60) % 60, now % 60);
    let state = serde_json::json!({
        "schemaVersion": 3,
        "platform": "windows",
        "port": port,
        "injectorPid": injector_pid,
        "injectorStartedAt": now_str,
        "injectorPath": injector_script.to_string_lossy().to_string(),
        "nodePath": injector_path,
        "codexExe": "",
        "codexPackageRoot": "",
        "codexVersion": "0.0.0",
        "browserId": browser_id,
        "createdAt": now,
    });
    let state_json = serde_json::to_string_pretty(&state)?;
    std::fs::write(state_root.join("state.json"), state_json)?;

    // 释放互斥锁
    *injection_lock().lock().unwrap() = false;

    Ok(get_dream_skin_status())
}

/// 停止 Dream Skin 注入（公共 API，带锁保护）
pub fn stop_dream_skin_injector() -> anyhow::Result<DreamSkinStatus> {
    let _lock = injection_lock().lock().unwrap();
    stop_dream_skin_injector_inner()
}

/// 等待 CDP 端点就绪
fn wait_for_cdp_endpoint(port: u16, timeout: Duration) -> anyhow::Result<bool> {
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        if http_get_ok(port, "/json/version") {
            return Ok(true);
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    Ok(false)
}

/// 获取 CDP browser ID
fn fetch_cdp_browser_id(port: u16) -> anyhow::Result<String> {
    let body = http_get(port, "/json/version")
        .map_err(|e| anyhow::anyhow!("查询 CDP version 失败: {}", e))?;
    let parsed: serde_json::Value = serde_json::from_str(&body)
        .map_err(|e| anyhow::anyhow!("解析 CDP version 失败: {}", e))?;

    let web_socket_url = parsed
        .get("webSocketDebuggerUrl")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("CDP 响应中缺少 webSocketDebuggerUrl"))?;

    // 从 URL 中提取 browser ID
    // 格式: ws://127.0.0.1:9335/devtools/browser/<id>
    let browser_id = web_socket_url
        .rsplit('/')
        .next()
        .ok_or_else(|| anyhow::anyhow!("无法从 CDP URL 解析 browser ID"))?;

    Ok(browser_id.to_string())
}

/// 简单的 HTTP GET 请求（只检查状态码 200）
fn http_get_ok(port: u16, path: &str) -> bool {
    TcpStream::connect_timeout(
        &format!("127.0.0.1:{}", port).parse().unwrap_or_else(|_| ([127, 0, 0, 1], port).into()),
        Duration::from_secs(2),
    )
    .and_then(|mut stream| {
        let request = format!("GET {} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n", path, port);
        stream.write_all(request.as_bytes())?;
        let mut response = String::new();
        stream.read_to_string(&mut response)?;
        Ok(response.starts_with("HTTP/1.1 200") || response.starts_with("HTTP/1.0 200"))
    })
    .unwrap_or(false)
}

/// 简单的 HTTP GET 请求（返回响应体）
fn http_get(port: u16, path: &str) -> Result<String, String> {
    let mut stream = TcpStream::connect_timeout(
        &format!("127.0.0.1:{}", port).parse().map_err(|e| format!("地址解析错误: {}", e))?,
        Duration::from_secs(5),
    )
    .map_err(|e| format!("连接失败: {}", e))?;

    let request = format!("GET {} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n", path, port);
    stream.write_all(request.as_bytes()).map_err(|e| format!("发送请求失败: {}", e))?;

    let mut response = String::new();
    stream.read_to_string(&mut response).map_err(|e| format!("读取响应失败: {}", e))?;

    // 分离 header 和 body
    if let Some(body_start) = response.find("\r\n\r\n") {
        Ok(response[body_start + 4..].to_string())
    } else {
        Err("无法解析 HTTP 响应".to_string())
    }
}

/// 停止 Dream Skin 注入（内部函数，不操作锁）
fn stop_dream_skin_injector_inner() -> anyhow::Result<DreamSkinStatus> {
    let state_root = state_root_dir();
    let state_path = state_root.join("state.json");

    // 停止 injector 进程
    let (_, pid) = check_injector_running();
    if let Some(pid) = pid {
        #[cfg(windows)]
        {
            let _ = std::process::Command::new("taskkill")
                .args(&["/F", "/PID", &pid.to_string()])
                .output();
        }
    }

    // 停止 Codex 进程
    let state = std::fs::read_to_string(&state_path).ok();
    if let Some(state_str) = state {
        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&state_str) {
            if let Some(_codex_exe) = parsed.get("codexExe").and_then(|v| v.as_str()) {
                #[cfg(windows)]
                {
                    let _ = std::process::Command::new("taskkill")
                        .args(&["/F", "/IM", "ChatGPT.exe"])
                        .output();
                }
            }
        }
    }

    // 删除 state.json
    let _ = std::fs::remove_file(&state_path);

    // 恢复基础主题
    let home = crate::codex_home::default_codex_home_dir();
    let _ = crate::relay_config::restore_dream_skin_base_theme(&home);

    Ok(get_dream_skin_status())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_base_theme_installed_returns_true_when_theme_markers_present() {
        let result = check_base_theme_installed();
        let home = crate::codex_home::default_codex_home_dir();
        let config_path = home.join("config.toml");
        let contents = std::fs::read_to_string(&config_path).unwrap_or_default();
        eprintln!("config.toml path: {}", config_path.display());
        eprintln!("has appearanceLightChromeTheme: {}", contents.contains("appearanceLightChromeTheme"));
        eprintln!("has #B65CFF: {}", contents.contains("#B65CFF"));
        eprintln!("has appearanceTheme: {}", contents.contains("appearanceTheme"));
        assert!(result, "check_base_theme_installed() should return true when config.toml contains theme markers");
    }
}