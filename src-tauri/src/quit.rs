//! Quitting and closing the window without losing an unsaved edit.
//!
//! On macOS the standard Quit item calls `NSApp terminate:`, which exits without a chance to
//! cancel. So the app menu gets its own Quit item (⌘Q), and closing the window is intercepted
//! too. Both send `app://quit-requested { id }` to the webview, which acknowledges at once
//! (`quit_ack`), saves any pending edit, then answers `quit_response(id, proceed)`.
//!
//! If the webview doesn't acknowledge within [`ACK_TIMEOUT`] (frozen or crashed), the app exits
//! anyway, so ⌘Q always works. A slow save after the acknowledgement is waited for.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use tauri::menu::{Menu, MenuItem};
use tauri::{AppHandle, Emitter, Manager, Runtime, State};

pub const QUIT_MENU_ID: &str = "readback-quit";
pub const QUIT_EVENT: &str = "app://quit-requested";
const ACK_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Default)]
pub struct QuitGuard {
    next_id: AtomicU64,
    /// The request in progress: its id, and whether the webview acknowledged it.
    pending: Mutex<Option<(u64, bool)>>,
    exiting: AtomicBool,
}

impl QuitGuard {
    fn pending(&self) -> std::sync::MutexGuard<'_, Option<(u64, bool)>> {
        self.pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn is_exiting(&self) -> bool {
        self.exiting.load(Ordering::SeqCst)
    }
}

fn exit<R: Runtime>(app: &AppHandle<R>) {
    app.state::<QuitGuard>()
        .exiting
        .store(true, Ordering::SeqCst);
    app.exit(0);
}

/// Asks the webview to save, then quits (see the module docs).
pub fn request<R: Runtime>(app: &AppHandle<R>) {
    let guard = app.state::<QuitGuard>();
    let id = guard.next_id.fetch_add(1, Ordering::SeqCst) + 1;
    *guard.pending() = Some((id, false));
    if app.emit(QUIT_EVENT, id).is_err() {
        exit(app);
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(ACK_TIMEOUT).await;
        let unanswered = *app.state::<QuitGuard>().pending() == Some((id, false));
        if unanswered {
            tracing::warn!("window didn't answer the quit request; quitting");
            exit(&app);
        }
    });
}

/// The default menu, with Quit replaced by one that saves first.
pub fn menu<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<Menu<R>> {
    let menu = Menu::default(app)?;
    if let Some(app_menu) = menu
        .items()?
        .first()
        .and_then(|item| item.as_submenu().cloned())
    {
        // The standard Quit item is the last one in the app menu.
        if let Some(last) = app_menu.items()?.last() {
            app_menu.remove(last)?;
        }
        app_menu.append(&MenuItem::with_id(
            app,
            QUIT_MENU_ID,
            "Quit Readback",
            true,
            Some("CmdOrCtrl+Q"),
        )?)?;
    }
    Ok(menu)
}

#[tauri::command]
pub fn quit_ack(guard: State<'_, QuitGuard>, id: u64) {
    let mut pending = guard.pending();
    if let Some((pending_id, acked)) = pending.as_mut()
        && *pending_id == id
    {
        *acked = true;
    }
}

/// `proceed: false` cancels (an edit couldn't be saved; the window explains why).
#[tauri::command]
pub fn quit_response<R: Runtime>(
    app: AppHandle<R>,
    guard: State<'_, QuitGuard>,
    id: u64,
    proceed: bool,
) {
    let matches = {
        let mut pending = guard.pending();
        let matches = pending.is_some_and(|(pending_id, _)| pending_id == id);
        if matches {
            *pending = None;
        }
        matches
    };
    if matches && proceed {
        exit(&app);
    }
}

/// Quits without saving, after the user chose "Quit anyway".
#[tauri::command]
pub fn quit_now<R: Runtime>(app: AppHandle<R>) {
    exit(&app);
}
