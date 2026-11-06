//! The Tauri command surface for notes. Every value returned here is display data, and every
//! path the webview sends is relative to the vault (see `paths.rs`).
//!
//! File work runs on a blocking thread, never on the main thread. Reads also have a timeout,
//! since an offloaded or network file can block.

use std::time::Duration;

use tauri::{AppHandle, Manager, Runtime, State};
use tauri_plugin_dialog::DialogExt;

use super::errors::{Error, Result};
use super::scan::{Entry, Tree};
use super::{NoteView, Notes, VaultView, WriteView};

/// How long `read_note` waits before giving up on a slow file.
pub const READ_TIMEOUT: Duration = Duration::from_secs(10);

type Service<'a> = State<'a, Notes>;

async fn blocking<T, F>(notes: &Notes, work: F) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce(Notes) -> Result<T> + Send + 'static,
{
    let notes = notes.clone();
    tauri::async_runtime::spawn_blocking(move || work(notes))
        .await
        .map_err(|_| Error::other("task_failed", "Readback's file task stopped unexpectedly."))?
}

#[tauri::command]
pub async fn get_vault(notes: Service<'_>) -> Result<VaultView> {
    blocking(&notes, |notes| Ok(notes.vault_view())).await
}

#[tauri::command]
pub async fn create_default_vault(notes: Service<'_>) -> Result<VaultView> {
    blocking(&notes, |notes| notes.create_default_vault()).await
}

/// Opens the system folder picker. Resolves to `None` if the user cancels.
#[tauri::command]
pub async fn choose_vault<R: Runtime>(
    app: AppHandle<R>,
    notes: Service<'_>,
) -> Result<Option<VaultView>> {
    let current = notes.vault_view();
    let start = current
        .path
        .as_deref()
        .map(std::path::PathBuf::from)
        .and_then(|path| {
            // Start next to the old location (also when it's missing: "Locate folder").
            path.ancestors()
                .skip(1)
                .find(|p| p.is_dir())
                .map(Into::into)
        })
        .or_else(|| app.path().document_dir().ok());
    let picked = tauri::async_runtime::spawn_blocking(move || {
        let mut dialog = app
            .dialog()
            .file()
            .set_title("Choose your notes folder")
            .set_can_create_directories(true);
        if let Some(start) = start {
            dialog = dialog.set_directory(start);
        }
        if let Some(window) = app.get_webview_window("main") {
            dialog = dialog.set_parent(&window);
        }
        dialog.blocking_pick_folder()
    })
    .await
    .map_err(|_| Error::other("dialog_failed", "The folder picker couldn't open."))?;
    let Some(picked) = picked else {
        return Ok(None);
    };
    let path = picked.into_path().map_err(|_| {
        Error::other(
            "dialog_failed",
            "That location can't be used as a notes folder.",
        )
    })?;
    blocking(&notes, move |notes| notes.set_vault(&path).map(Some)).await
}

#[tauri::command]
pub async fn list_tree(notes: Service<'_>) -> Result<Tree> {
    blocking(&notes, |notes| notes.list_tree()).await
}

#[tauri::command]
pub async fn read_note(notes: Service<'_>, rel_path: String) -> Result<NoteView> {
    let name = super::paths::file_name(&rel_path).to_owned();
    let read = blocking(&notes, move |notes| notes.read_note(&rel_path));
    tokio::time::timeout(READ_TIMEOUT, read)
        .await
        .map_err(|_| Error::timeout(&name))?
}

/// `base_revision` is the revision the edit started from, or `None` to create the note (for
/// example "Recreate from my copy" after it was deleted on disk).
#[tauri::command]
pub async fn write_note(
    notes: Service<'_>,
    rel_path: String,
    content: String,
    base_revision: Option<String>,
) -> Result<WriteView> {
    blocking(&notes, move |notes| {
        notes.write_note(&rel_path, &content, base_revision.as_deref())
    })
    .await
}

#[tauri::command]
pub async fn create_note(notes: Service<'_>, parent_rel: String, title: String) -> Result<Entry> {
    blocking(&notes, move |notes| notes.create_note(&parent_rel, &title)).await
}

#[tauri::command]
pub async fn create_folder(notes: Service<'_>, parent_rel: String, name: String) -> Result<Entry> {
    blocking(&notes, move |notes| notes.create_folder(&parent_rel, &name)).await
}

#[tauri::command]
pub async fn rename_entry(notes: Service<'_>, rel_path: String, new_name: String) -> Result<Entry> {
    blocking(&notes, move |notes| {
        notes.rename_entry(&rel_path, &new_name)
    })
    .await
}

#[tauri::command]
pub async fn move_entry(
    notes: Service<'_>,
    rel_path: String,
    new_parent_rel: String,
) -> Result<Entry> {
    blocking(&notes, move |notes| {
        notes.move_entry(&rel_path, &new_parent_rel)
    })
    .await
}

#[tauri::command]
pub async fn trash_entry(notes: Service<'_>, rel_path: String) -> Result<()> {
    blocking(&notes, move |notes| notes.trash_entry(&rel_path)).await
}

/// `""` reveals the vault folder itself.
#[tauri::command]
pub async fn reveal_in_finder(notes: Service<'_>, rel_path: String) -> Result<()> {
    blocking(&notes, move |notes| {
        let path = notes.locate(&rel_path)?;
        tauri_plugin_opener::reveal_item_in_dir(&path)
            .map_err(|_| Error::other("reveal_failed", "Finder couldn't show this item."))
    })
    .await
}

#[tauri::command]
pub async fn open_in_default_app(notes: Service<'_>, rel_path: String) -> Result<()> {
    blocking(&notes, move |notes| {
        if rel_path.is_empty() {
            return Err(Error::invalid_path());
        }
        let path = notes.locate(&rel_path)?;
        tauri_plugin_opener::open_path(&path, None::<&str>).map_err(|_| {
            Error::other(
                "open_failed",
                format!(
                    "macOS couldn't open “{}” in its default app.",
                    super::paths::file_name(&rel_path)
                ),
            )
        })
    })
    .await
}
