# Readback Desktop

Open-source macOS app (Tauri 2 + React) that will quiz users on their own notes. The current
work is a **spike**: prove Sign in with ChatGPT (SIWC) end to end. A user signs in with their
ChatGPT plan (no API key), sends a prompt, and sees the streamed reply.

## Layout

```
src/                     React 19 + Mantine 9 frontend (webview). Display only.
  main.tsx               MantineProvider + root
  App.tsx                the single window: signed out / connected / reauth states
  api.ts                 typed wrappers for the Tauri commands (display data only)
  components/            ContinueWithChatGPT (official button), ManageUsageButton, PromptPanel,
                         ErrorNotice, WelcomeModal
  assets/                official ChatGPT logos (from developers.openai.com)
src-tauri/               Rust backend (Tauri 2)
  src/lib.rs             Tauri builder, plugins, command registration
  src/chatgpt/           SIWC runtime
    mod.rs               ChatGpt service: sign-in, sign-out, refresh orchestration, prompts
    commands.rs          Tauri command surface (display data only)
    oauth.rs             discovery, PKCE/state/nonce, authorize URL, loopback, code exchange,
                         token parsing, revocation
    id_token.rs          JWKS fetch + ID token validation
    refresh.rs           refresh decision + rotating refresh request
    inference.rs         model listing, SSE parser, streaming Responses call
    store.rs             profile JSON (0600, atomic) + Keychain vault
    host_id.rs           ext_agent_host_id (urn:uuid, per installation)
    secret.rs            `Secret` newtype (redacted, zeroized, not serializable)
    errors.rs            typed errors with user-facing messages
  capabilities/          webview permissions (core only, no opener)
  tauri.conf.json        app config, CSP, macOS bundle
```

## Commands

```sh
yarn install
yarn tauri dev                       # run the app
yarn lint && yarn typecheck          # frontend checks
yarn format:check                    # Prettier
cd src-tauri && cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test
```

## Rules (non-negotiable)

- **Tokens never reach the webview.** OAuth, token storage, refresh, model listing and the
  Responses call all live in Rust. Commands return display data only (email, status,
  `sharing` flag, model slugs and names, streamed text). The frontend source must never mention
  `access_token`, `refresh_token` or `id_token`.
- Never log, print or serialize a token. Tokens are wrapped in a newtype whose
  `Debug`/`Display` print `[redacted]`. URLs that carry tokens are never logged.
- Secrets go in the macOS Keychain (one item per registration). Non-secret data (host ID,
  issued client ID, email, subject, scopes, expiry) goes in JSON in the app data dir.
- Sign-in opens the **system browser** (opener plugin, from Rust). Never embed the auth page in
  a webview. The webview has no opener permission; Rust opens only fixed, validated URLs.
- Loopback redirect is `http://127.0.0.1:<port>/auth/callback`. Never `localhost`.
- Send `login_hint`, not `id_token_hint`: `/usr/bin/open` exposes the URL in process arguments.
- Only the documented SIWC flow. No ChatGPT `backend-api`, no cookies, no Codex auth file, no
  API-key path, no other provider, no telemetry or crash reporting.
- Never invent endpoints, parameters or model IDs. Models come from `GET /v1/models`.
- Usage-limit and other non-retryable plan errors are never retried in a loop.
- No notes, quizzes or spaced repetition yet.

## Sources of truth

- Docs: https://developers.openai.com/siwc/token-sharing-open-source (sign-in, accounts and
  sessions, models and inference, errors and recovery, token reference, preview limitations)
  and https://developers.openai.com/siwc/ui-ux-guidelines.
- Reference runtime: https://github.com/openai/sign-in-with-chatgpt-devkit (`packages/local`).
  It is under a **noncommercial** license: use it as a behavioural reference and write our
  Rust code from the public docs. Do not translate or copy its source.

## Conventions

- Prettier: 100 cols, single quotes, semicolons, trailing commas. ESLint flat config with
  `eslint-config-prettier` last. Rust: `rustfmt` (100 cols) and clippy with `-D warnings`.
- Dependencies are pinned to exact versions. Check current versions before adding one.
- Yarn 4 (via Corepack, `nodeLinker: node-modules`). TypeScript stays on 6.0.x until
  `typescript-eslint` supports 7.
- Conventional Commits, one commit per phase. Stop for confirmation at the end of each phase.
- macOS only (14+). No signing, notarization or CI yet.
