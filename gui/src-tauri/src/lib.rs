use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use smr_core::{run_app, SharedApp, DEFAULT_CONFIG_YAML};
use tauri::Manager;
use tracing::info;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .setup(|app| {
            let config_path = resolve_config_path();
            let (shared, path) = SharedApp::load_or_create(&config_path, DEFAULT_CONFIG_YAML)
                .map_err(|e| format!("config error: {e}"))?;
            let listen = shared.config().server.listen.clone();
            info!(config = %path.display(), listen = %listen, "starting SecureModelRoute server");

            let shared = Arc::clone(&shared);
            tauri::async_runtime::spawn(async move {
                if let Err(err) = run_app(shared).await {
                    tracing::error!(error = %err, "server exited");
                }
            });

            std::thread::sleep(Duration::from_millis(600));

            if let Some(window) = app.get_webview_window("main") {
                let ui = format!("http://{listen}/ui");
                let _ = window.eval(&format!("window.location.replace('{ui}')"));
            }

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("failed to run SecureModelRoute GUI");
}

fn resolve_config_path() -> PathBuf {
    if let Ok(p) = std::env::var("SMR_CONFIG") {
        return PathBuf::from(p);
    }
    smr_core::paths::default_config_path()
}
