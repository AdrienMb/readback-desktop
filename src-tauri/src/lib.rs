mod chatgpt;
mod notes;
mod quit;

use std::sync::Arc;

use tauri::{AppHandle, Emitter, Manager};

use chatgpt::commands;
use notes::commands as note_commands;

/// Sends notes watcher results to the webview as `vault://…` events.
struct VaultEvents(AppHandle);

impl notes::Emit for VaultEvents {
    fn changed(&self, changes: &notes::watcher::ChangeSet) {
        let _ = self.0.emit("vault://changed", changes);
    }

    fn rescanned(&self, tree: &notes::scan::Tree) {
        let _ = self.0.emit("vault://rescanned", tree);
    }

    fn status(&self, vault: &notes::VaultView) {
        let _ = self.0.emit("vault://status", vault);
    }
}

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
        .manage(quit::QuitGuard::default())
        .menu(quit::menu)
        .on_menu_event(|app, event| {
            if event.id() == quit::QUIT_MENU_ID {
                quit::request(app);
            }
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event
                && !window.state::<quit::QuitGuard>().is_exiting()
            {
                // Save any pending edit first; the window answers through `quit_response`.
                api.prevent_close();
                quit::request(window.app_handle());
            }
        })
        // Used from Rust only (OAuth URL, Manage usage). The webview has no opener permission.
        .plugin(tauri_plugin_opener::init())
        // Folder picker, opened from Rust only. The webview has no dialog or fs permission.
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            let secrets = chatgpt::store::Keychain::new()?;
            let store = chatgpt::store::Store::new(dir, Box::new(secrets));
            let service = chatgpt::ChatGpt::new(app.handle().clone(), store)?;
            app.manage(Arc::new(service));

            let default_vault = app
                .path()
                .document_dir()
                .ok()
                .map(|documents| documents.join(notes::config::DEFAULT_VAULT_NAME));
            let notes = notes::Notes::new(
                app.path().app_data_dir()?,
                default_vault,
                Box::new(VaultEvents(app.handle().clone())),
            )?;
            app.manage(notes);
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
            note_commands::get_vault,
            note_commands::create_default_vault,
            note_commands::choose_vault,
            note_commands::list_tree,
            note_commands::read_note,
            note_commands::write_note,
            note_commands::create_note,
            note_commands::create_folder,
            note_commands::rename_entry,
            note_commands::move_entry,
            note_commands::trash_entry,
            note_commands::reveal_in_finder,
            note_commands::open_in_default_app,
            quit::quit_ack,
            quit::quit_response,
            quit::quit_now,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
