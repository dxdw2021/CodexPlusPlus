pub mod commands;
pub mod install;

use std::sync::atomic::{AtomicBool, Ordering};

use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{Manager, WindowEvent};

const TRAY_ID: &str = "codex_plus_tray";

static APP_EXITING: AtomicBool = AtomicBool::new(false);
const TRAY_MENU_SHOW: &str = "tray_show_main";
const TRAY_MENU_QUIT: &str = "tray_quit_app";

pub fn run() {
    install_panic_logger();
    let _ = codex_plus_core::diagnostic_log::append_diagnostic_log(
        "manager.start",
        serde_json::json!({
            "version": env!("CARGO_PKG_VERSION")
        }),
    );
    let Some(_guard) = acquire_single_instance_guard() else {
        return;
    };
    let show_update = commands::startup_should_show_update();
    let run_result = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(move |app| {
            let url = if show_update {
                "/index.html?showUpdate=1"
            } else {
                "/index.html"
            };
            let mut main_window_builder =
                tauri::WebviewWindowBuilder::new(app, "main", tauri::WebviewUrl::App(url.into()))
                    .title("Codex++ 管理工具")
                    .inner_size(1180.0, 820.0)
                    .min_inner_size(960.0, 720.0);
            if let Some(icon) = app.default_window_icon().cloned() {
                main_window_builder = main_window_builder.icon(icon)?;
            }
            let main_window = main_window_builder.build()?;
            #[cfg(debug_assertions)]
            main_window.open_devtools();
            install_tray(app)?;
            register_main_window_events(main_window);
            // 在管理工具启动时，自动在后台启动 dream skin 注入
            // 等待 CDP 就绪后获取 browser ID 并启动 injector
            let default_port = 9229u16;
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_secs(5));
                let theme_ok = codex_plus_core::dream_skin::check_base_theme_installed();
                let _ = codex_plus_core::diagnostic_log::append_diagnostic_log(
                    "manager.auto_dream_skin",
                    serde_json::json!({"debug_port": default_port, "theme_installed": theme_ok}),
                );
                if !theme_ok {
                    return;
                }
                // 等待 CDP 端点就绪（最多 60 秒，每 1 秒重试一次）
                let browser_id = (|| -> Option<String> {
                    use std::io::{Read, Write};
                    use std::net::TcpStream;
                    use std::time::Duration;
                    for attempt in 0..60 {
                        // 尝试连接 CDP 端点
                        let mut stream = match TcpStream::connect_timeout(
                            &"127.0.0.1:9229".parse().ok()?,
                            Duration::from_secs(2),
                        ) {
                            Ok(s) => s,
                            Err(e) => {
                                if attempt == 0 || attempt == 10 || attempt == 30 || attempt == 50 {
                                    let _ = codex_plus_core::diagnostic_log::append_diagnostic_log(
                                        "manager.auto_dream_skin_wait",
                                        serde_json::json!({"debug_port": default_port, "attempt": attempt, "error": e.to_string()}),
                                    );
                                }
                                std::thread::sleep(Duration::from_secs(1));
                                continue;
                            }
                        };
                        // 连接成功，设置读超时
                        let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                        let request = "GET /json/version HTTP/1.1\r\nHost: 127.0.0.1:9229\r\nConnection: close\r\n\r\n";
                        if stream.write_all(request.as_bytes()).is_err() {
                            std::thread::sleep(Duration::from_secs(1));
                            continue;
                        }
                        // 读取响应（循环读取，超时即停止）
                        let mut response = Vec::new();
                        let mut buf = [0u8; 4096];
                        loop {
                            match stream.read(&mut buf) {
                                Ok(0) => break,
                                Ok(n) => response.extend_from_slice(&buf[..n]),
                                Err(_) => break,
                            }
                            if response.len() > 4096 { break; }
                        }
                        if response.is_empty() {
                            std::thread::sleep(Duration::from_secs(1));
                            continue;
                        }
                        let response_str = String::from_utf8_lossy(&response);
                        let _ = codex_plus_core::diagnostic_log::append_diagnostic_log(
                            "manager.auto_dream_skin_cdp_raw",
                            serde_json::json!({"bytes_read": response.len(), "attempt": attempt}),
                        );
                        // 从响应体中提取 webSocketDebuggerUrl
                        if let Some(body_start) = response_str.find("\r\n\r\n") {
                            let body = &response_str[body_start + 4..];
                            let key = "\"webSocketDebuggerUrl\"";
                            if let Some(val_start) = body.find(key) {
                                let after_key = &body[val_start + key.len()..];
                                if let Some(quote_start) = after_key.find('"') {
                                    let after_quote = &after_key[quote_start + 1..];
                                    if let Some(quote_end) = after_quote.find('"') {
                                        let url = &after_quote[..quote_end];
                                        if let Some(id_start) = url.rfind('/') {
                                            let id = &url[id_start + 1..];
                                            if !id.is_empty() && id.chars().all(|c| c.is_alphanumeric() || c == '.' || c == '_' || c == '-') {
                                                return Some(id.to_string());
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        std::thread::sleep(Duration::from_secs(1));
                    }
                    None
                })();
                let Some(browser_id) = browser_id else {
                    let _ = codex_plus_core::diagnostic_log::append_diagnostic_log(
                        "manager.auto_dream_skin_no_browser_id",
                        serde_json::json!({"debug_port": default_port}),
                    );
                    return;
                };
                // 查找 Node.js 和 injector 脚本路径
                let Some(node_path) = codex_plus_core::dream_skin::find_node() else {
                    let _ = codex_plus_core::diagnostic_log::append_diagnostic_log(
                        "manager.auto_dream_skin_no_node",
                        serde_json::json!({"debug_port": default_port}),
                    );
                    return;
                };
                let assets_dir = codex_plus_core::dream_skin::dream_skin_assets_dir();
                let injector_script = assets_dir.join("scripts").join("injector.mjs");
                if !injector_script.exists() {
                    let _ = codex_plus_core::diagnostic_log::append_diagnostic_log(
                        "manager.auto_dream_skin_no_script",
                        serde_json::json!({"debug_port": default_port, "path": injector_script.to_string_lossy()}),
                    );
                    return;
                }
                // 直接启动 injector（不等待，detached 独立进程）
                match std::process::Command::new(&node_path)
                    .arg(injector_script.to_string_lossy().as_ref())
                    .args(["--watch", "--port", &default_port.to_string(), "--browser-id", &browser_id])
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn()
                {
                    Ok(_) => {
                        let _ = codex_plus_core::diagnostic_log::append_diagnostic_log(
                            "manager.auto_dream_skin_ok",
                            serde_json::json!({"debug_port": default_port, "browser_id": browser_id, "node": node_path.to_string_lossy(), "injector": injector_script.to_string_lossy()}),
                        );
                    }
                    Err(e) => {
                        let _ = codex_plus_core::diagnostic_log::append_diagnostic_log(
                            "manager.auto_dream_skin_spawn_fail",
                            serde_json::json!({"debug_port": default_port, "error": e.to_string(), "node": node_path.to_string_lossy(), "injector": injector_script.to_string_lossy()}),
                        );
                    }
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::backend_version,
            commands::startup_options,
            commands::load_overview,
            commands::launch_codex_plus,
            commands::restart_codex_plus,
            commands::load_settings,
            commands::save_settings,
            commands::load_ccs_providers,
            commands::import_ccs_providers,
            commands::load_pending_provider_import,
            commands::confirm_pending_provider_import,
            commands::dismiss_pending_provider_import,
            commands::list_local_sessions,
            commands::list_zed_remote_projects,
            commands::open_zed_remote,
            commands::forget_zed_remote_project,
            commands::delete_local_session,
            commands::load_provider_sync_targets,
            commands::preview_session_index_cleanup,
            commands::apply_session_index_cleanup,
            commands::sync_providers_now,
            commands::load_ads,
            commands::refresh_script_market,
            commands::install_market_script,
            commands::set_user_script_enabled,
            commands::delete_user_script,
            commands::open_external_url,
            commands::install_entrypoints,
            commands::uninstall_entrypoints,
            commands::repair_shortcuts,
            commands::plugin_marketplace_status,
            commands::repair_plugin_marketplace,
            commands::remote_plugin_marketplace_status,
            commands::repair_remote_plugin_marketplace,
            commands::check_update,
            commands::perform_update,
            commands::load_watcher_state,
            commands::install_watcher,
            commands::uninstall_watcher,
            commands::enable_watcher,
            commands::disable_watcher,
            commands::read_latest_logs,
            commands::copy_diagnostics,
            commands::reset_settings,
            commands::reset_image_overlay_settings,
            commands::relay_status,
            commands::read_relay_files,
            commands::check_env_conflicts,
            commands::check_relay_environment,
            commands::remove_env_conflicts,
            commands::save_relay_file,
            commands::write_diagnostic_event,
            commands::backfill_relay_profile_from_live,
            commands::list_context_entries,
            commands::read_live_context_entries,
            commands::sync_live_context_entries,
            commands::upsert_context_entry,
            commands::delete_context_entry,
            commands::extract_relay_common_config,
            commands::test_relay_profile,
            commands::diagnose_relay_profile,
            commands::test_stepwise_settings,
            commands::fetch_relay_profile_models,
            commands::switch_relay_profile,
            commands::apply_relay_injection,
            commands::apply_pure_api_injection,
            commands::clear_relay_injection,
            commands::get_dream_skin_status,
            commands::install_dream_skin,
            commands::restore_dream_skin_base,
            commands::start_dream_skin_injector,
            commands::stop_dream_skin_injector,
            manager_exit_app,
            manager_hide_to_tray,
            update_tray_labels
        ])
        .run(tauri::generate_context!());
    if let Err(error) = run_result {
        let _ = codex_plus_core::diagnostic_log::append_diagnostic_log(
            "manager.run_failed",
            serde_json::json!({
                "error": error.to_string()
            }),
        );
    }
}

fn install_tray<R: tauri::Runtime>(app: &tauri::App<R>) -> tauri::Result<()> {
    let show_item = MenuItem::with_id(app, TRAY_MENU_SHOW, "显示主窗口", true, None::<&str>)?;
    let quit_item = MenuItem::with_id(app, TRAY_MENU_QUIT, "退出程序", true, None::<&str>)?;
    let tray_menu = Menu::with_items(app, &[&show_item, &quit_item])?;

    let mut tray_builder = TrayIconBuilder::with_id(TRAY_ID)
        .menu(&tray_menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            TRAY_MENU_SHOW => {
                show_main_window(app);
            }
            TRAY_MENU_QUIT => {
                APP_EXITING.store(true, Ordering::SeqCst);
                app.exit(0);
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| match event {
            TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            }
            | TrayIconEvent::DoubleClick {
                button: MouseButton::Left,
                ..
            } => {
                show_main_window(&tray.app_handle());
            }
            _ => {}
        });

    if let Some(icon) = app.default_window_icon().cloned() {
        tray_builder = tray_builder.icon(icon);
    }

    let _ = tray_builder.build(app)?;
    Ok(())
}

fn register_main_window_events<R: tauri::Runtime>(window: tauri::WebviewWindow<R>) {
    let event_window = window.clone();
    let minimized_window = event_window.clone();
    let close_event_window = event_window.clone();

    event_window.on_window_event(move |event| match event {
        WindowEvent::Resized(_) => {
            if matches!(minimized_window.is_minimized(), Ok(true)) {
                let _ = minimized_window.hide();
            }
        }
        WindowEvent::CloseRequested { api, .. } => {
            if APP_EXITING.load(Ordering::SeqCst) {
                return;
            }

            api.prevent_close();
            let _ = close_event_window.hide();
        }
        _ => {}
    });
}

#[tauri::command]
fn manager_exit_app<R: tauri::Runtime>(app: tauri::AppHandle<R>) {
    APP_EXITING.store(true, Ordering::SeqCst);
    app.exit(0);
}

#[tauri::command]
fn manager_hide_to_tray<R: tauri::Runtime>(window: tauri::WebviewWindow<R>) {
    let _ = window.hide();
}

#[tauri::command]
fn update_tray_labels<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    show_label: String,
    quit_label: String,
    window_title: String,
) {
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        let show_item = MenuItem::with_id(&app, TRAY_MENU_SHOW, &show_label, true, None::<&str>);
        let quit_item = MenuItem::with_id(&app, TRAY_MENU_QUIT, &quit_label, true, None::<&str>);
        if let (Ok(show), Ok(quit)) = (show_item, quit_item) {
            if let Ok(menu) = Menu::with_items(&app, &[&show, &quit]) {
                let _ = tray.set_menu(Some(menu));
            }
        }
    }
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.set_title(&window_title);
    }
}

fn show_main_window<R: tauri::Runtime>(app_handle: &tauri::AppHandle<R>) {
    if let Some(window) = app_handle.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

/// Restores and focuses an existing manager window on Windows.
///
/// This is a no-op on other platforms.
pub fn focus_existing_manager_window() {
    #[cfg(windows)]
    {
        let current_process_id = std::process::id();
        for process in codex_plus_core::windows_enumerate_processes() {
            if process.process_id == current_process_id {
                continue;
            }
            if process
                .exe_file
                .eq_ignore_ascii_case("codex-plus-plus-manager.exe")
            {
                let _ = codex_plus_core::windows_activate_process_window(process.process_id);
                break;
            }
        }
    }
}

fn install_panic_logger() {
    std::panic::set_hook(Box::new(|panic_info| {
        let payload = panic_info
            .payload()
            .downcast_ref::<&str>()
            .map(|message| (*message).to_string())
            .or_else(|| panic_info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "非字符串 panic payload".to_string());
        let location = panic_info.location().map(|location| {
            serde_json::json!({
                "file": location.file(),
                "line": location.line(),
                "column": location.column()
            })
        });
        let _ = codex_plus_core::diagnostic_log::append_diagnostic_log(
            "manager.panic",
            serde_json::json!({
                "payload": payload,
                "location": location
            }),
        );
    }));
}

fn acquire_single_instance_guard() -> Option<codex_plus_core::ports::LoopbackPortGuard> {
    match codex_plus_core::ports::acquire_resilient_loopback_port_guard(
        codex_plus_core::ports::manager_guard_port(),
    ) {
        Ok(guard) => {
            if let Some(fallback_lock_path) = guard.fallback_path() {
                let _ = codex_plus_core::diagnostic_log::append_diagnostic_log(
                    "manager.guard_fallback",
                    serde_json::json!({
                        "requested_guard_port": codex_plus_core::ports::manager_guard_port(),
                        "fallback_lock_path": fallback_lock_path
                    }),
                );
            }
            Some(guard)
        }
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::AddrInUse | std::io::ErrorKind::WouldBlock
            ) =>
        {
            let _ = codex_plus_core::diagnostic_log::append_diagnostic_log(
                "manager.already_running",
                serde_json::json!({
                    "guard_port": codex_plus_core::ports::manager_guard_port()
                }),
            );
            focus_existing_manager_window();
            None
        }
        Err(error) => {
            let _ = codex_plus_core::diagnostic_log::append_diagnostic_log(
                "manager.guard_failed",
                serde_json::json!({
                    "guard_port": codex_plus_core::ports::manager_guard_port(),
                    "error": error.to_string()
                }),
            );
            match std::net::TcpListener::bind(("127.0.0.1", 0)) {
                Ok(listener) => Some(codex_plus_core::ports::LoopbackPortGuard::listener(
                    listener,
                )),
                Err(fallback_error) => {
                    let _ = codex_plus_core::diagnostic_log::append_diagnostic_log(
                        "manager.guard_fallback_failed",
                        serde_json::json!({
                            "error": fallback_error.to_string()
                        }),
                    );
                    None
                }
            }
        }
    }
}
