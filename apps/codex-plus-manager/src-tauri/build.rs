fn main() {
    // 复制项目根的 assets/dream-skin/ 到 exe 旁，供生产 exe 运行时查找
    copy_dream_skin_assets();

    let windows = tauri_build::WindowsAttributes::new()
        .app_manifest(include_str!("windows-app-manifest.xml"));
    let attrs = tauri_build::Attributes::new().windows_attributes(windows);
    tauri_build::try_build(attrs).expect("failed to run Tauri build script");
}

/// 将项目根的 assets/dream-skin/ 复制到 target/<profile>/assets/dream-skin/
/// 这样 exe 运行时 dream_skin_assets_dir() 的 exe 旁路径分支能命中
fn copy_dream_skin_assets() {

    // 项目根目录 = Cargo.toml 所在目录的上级的上级的上级
    // manifest_dir = .../apps/codex-plus-manager/src-tauri
    // parent×3 = .../ (项目根)
    let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let project_root = manifest_dir
        .parent()        // apps/codex-plus-manager
        .and_then(|p| p.parent())   // apps
        .and_then(|p| p.parent())   // 项目根
        .expect("cannot locate project root");
    let src_assets = project_root.join("assets").join("dream-skin");
    if !src_assets.exists() {
        println!("cargo:warning=[dream-skin] src assets NOT FOUND at {}", src_assets.display());
        return;
    }
    println!("cargo:warning=[dream-skin] src assets FOUND at {}", src_assets.display());

    // 目标目录 = target/<profile>/
    // Cargo 未公开稳定方式获取 profile，OUT_DIR 也不一定可用
    // 采用探测法：同时尝试 debug 和 release 两个目标目录
    let target_dir = std::env::var("CARGO_TARGET_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| project_root.join("target"));

    // 用 OUT_DIR 若可用则用，否则探测 debug + release
    let dest_root = if let Ok(out_dir) = std::env::var("OUT_DIR") {
        std::path::PathBuf::from(out_dir)
            .parent()
            .and_then(|p| p.parent())
            .and_then(|p| p.parent())
            .and_then(|p| p.parent())
            .map(|p| p.to_path_buf())
            .unwrap_or(target_dir.join("debug"))
    } else {
        target_dir.join("debug")
    };

    // 同时复制到 debug 和 release 两个目录，避免 profile 探测不准
    for profile in ["debug", "release"] {
        let dest = target_dir.join(profile).join("assets").join("dream-skin");
        let _ = copy_dir_all(&src_assets, &dest);
    }
    // 同时也复制到 OUT_DIR 推算的目录（双保险）
    let dest_assets = dest_root.join("assets").join("dream-skin");
    let _ = copy_dir_all(&src_assets, &dest_assets);
}

fn copy_dir_all(src: &std::path::Path, dest: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dest)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let path = entry.path();
        let dest_path = dest.join(entry.file_name());
        if path.is_dir() {
            copy_dir_all(&path, &dest_path)?;
        } else {
            let _ = std::fs::remove_file(&dest_path);
            std::fs::copy(&path, &dest_path)?;
        }
    }
    Ok(())
}
