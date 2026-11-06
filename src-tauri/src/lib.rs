mod chatgpt;

use std::sync::Arc;

use tauri::Manager;

use chatgpt::commands;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Fixed filter, deliberately not read from RUST_LOG: third-party crates stay at `warn` so
    // HTTP internals are never traced. Our own logs never contain credentials.
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new(
            "warn,readback_desktop_lib=info",
        ))
        .with_target(false)
        .init();

    tauri::Builder::default()
        // Used from Rust only (OAuth URL, Manage usage). The webview has no opener permission.
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            let secrets = chatgpt::store::Keychain::new()?;
            let store = chatgpt::store::Store::new(dir, Box::new(secrets));
            let service = chatgpt::ChatGpt::new(app.handle().clone(), store)?;
            app.manage(Arc::new(service));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_session,
            commands::sign_in,
            commands::cancel_sign_in,
            commands::sign_out,
            commands::list_models,
            commands::send_prompt,
            commands::cancel_prompt,
            commands::open_usage_settings,
            commands::dismiss_welcome,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
