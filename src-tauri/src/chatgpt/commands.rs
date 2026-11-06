//! The Tauri command surface. Every value returned here is display data.

use std::sync::Arc;

use serde::Serialize;
use tauri::State;
use tauri::ipc::Channel;

use super::errors::Result;
use super::inference::Model;
use super::{ChatGpt, SessionView, SignOutView};

type Service<'a> = State<'a, Arc<ChatGpt>>;

#[derive(Clone, Serialize)]
#[serde(tag = "event", content = "data", rename_all = "camelCase")]
pub enum PromptEvent {
    Delta { text: String },
}

#[tauri::command]
pub fn get_session(service: Service<'_>) -> Result<SessionView> {
    service.session()
}

#[tauri::command]
pub async fn sign_in(service: Service<'_>, reconsent: Option<bool>) -> Result<SessionView> {
    service.sign_in(reconsent.unwrap_or(false)).await
}

#[tauri::command]
pub fn cancel_sign_in(service: Service<'_>) {
    service.cancel_sign_in();
}

#[tauri::command]
pub async fn sign_out(service: Service<'_>) -> Result<SignOutView> {
    service.sign_out().await
}

#[tauri::command]
pub async fn list_models(service: Service<'_>) -> Result<Vec<Model>> {
    service.list_models().await
}

/// Streams text deltas over `on_event`; resolves when the response completes, or rejects with
/// an error (including `cancelled`).
#[tauri::command]
pub async fn send_prompt(
    service: Service<'_>,
    model: String,
    prompt: String,
    on_event: Channel<PromptEvent>,
) -> Result<()> {
    service
        .send_prompt(&model, &prompt, |text| {
            let _ = on_event.send(PromptEvent::Delta {
                text: text.to_owned(),
            });
        })
        .await
}

#[tauri::command]
pub fn cancel_prompt(service: Service<'_>) {
    service.cancel_prompt();
}

#[tauri::command]
pub fn open_usage_settings(service: Service<'_>) -> Result<()> {
    service.open_usage_settings()
}

#[tauri::command]
pub fn dismiss_welcome(service: Service<'_>) -> Result<SessionView> {
    service.dismiss_welcome()
}
