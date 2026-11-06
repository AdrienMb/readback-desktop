// Typed wrappers around the Rust command surface. The backend only ever returns display data;
// credentials stay in Rust.
import { Channel, invoke } from '@tauri-apps/api/core';

export type SessionStatus = 'signed_out' | 'connected' | 'reauth_required';

export interface Session {
  status: SessionStatus;
  email: string | null;
  /** True only when ChatGPT plan usage was granted. */
  sharing: boolean;
  showWelcome: boolean;
}

export interface SignOutResult {
  session: Session;
  revocationConfirmed: boolean;
}

export interface Model {
  slug: string;
  displayName: string;
}

export type ErrorKind =
  | 'usage_limit'
  | 'sharing_disabled'
  | 'not_eligible'
  | 'reauth'
  | 'network'
  | 'temporary'
  | 'cancelled'
  | 'sign_in'
  | 'request'
  | 'storage'
  | 'other';

export interface AppError {
  kind: ErrorKind;
  code: string;
  message: string;
  retryable: boolean;
  status?: number;
  requestId?: string;
}

type PromptEvent = { event: 'delta'; data: { text: string } };

function isAppError(value: unknown): value is AppError {
  return (
    typeof value === 'object' &&
    value !== null &&
    typeof (value as AppError).kind === 'string' &&
    typeof (value as AppError).message === 'string'
  );
}

/** Normalizes a rejected command into an AppError. */
export function toAppError(error: unknown): AppError {
  if (isAppError(error)) return error;
  return {
    kind: 'other',
    code: 'ipc_error',
    message:
      typeof error === 'string' && error.length > 0
        ? `Readback's backend reported: ${error}`
        : "Readback's window couldn't reach its backend. Restart the app.",
    retryable: false,
  };
}

export const getSession = () => invoke<Session>('get_session');

export const signIn = (reconsent = false) => invoke<Session>('sign_in', { reconsent });

export const cancelSignIn = () => invoke<void>('cancel_sign_in');

export const signOut = () => invoke<SignOutResult>('sign_out');

export const listModels = () => invoke<Model[]>('list_models');

export function sendPrompt(model: string, prompt: string, onDelta: (text: string) => void) {
  const onEvent = new Channel<PromptEvent>();
  onEvent.onmessage = (message) => {
    if (message.event === 'delta') onDelta(message.data.text);
  };
  return invoke<void>('send_prompt', { model, prompt, onEvent });
}

export const cancelPrompt = () => invoke<void>('cancel_prompt');

export const openUsageSettings = () => invoke<void>('open_usage_settings');

export const dismissWelcome = () => invoke<Session>('dismiss_welcome');
