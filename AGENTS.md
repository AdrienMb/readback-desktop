# Readback Desktop

Open-source macOS app (Tauri 2 + React) that will quiz users on their own notes. The current
work is a **spike**: prove Sign in with ChatGPT (SIWC) end to end. A user signs in with their
ChatGPT plan (no API key), sends a prompt, and sees the streamed reply. Alongside it, local notes:
plain folders and Markdown files in a user-visible folder (the vault), edited live in CodeMirror
or read rendered (react-markdown, GFM), toggled from the title bar (⌘E).

## Layout

```
src/                     React 19 + Mantine 9 frontend (webview). Display only.
  main.tsx               MantineProvider + root
  Shell.tsx              the window: notes sidebar + open note, or the ChatGPT panel (App)
  App.tsx                ChatGPT panel: signed out / connected / reauth states
  api.ts                 typed wrappers for the Tauri commands (display data only)
  components/            ContinueWithChatGPT (official button), ManageUsageButton, PromptPanel,
                         ErrorNotice, WelcomeModal
  notes/                 notes UI: api.ts (commands + vault:// events), tree.ts (pure tree
                         helpers), Sidebar (drag a row onto a folder or empty space to
                         move it, @dnd-kit/core), NoteEditor (autosave, on-disk change
                         banners, view/edit mode), MarkdownEditor (CodeMirror 6), MarkdownView
                         (react-markdown + GFM, links don't navigate), VaultSetup (onboarding /
                         missing), EntryDialogs, NotesErrorNotice
  assets/                official ChatGPT logos (from developers.openai.com)
src-tauri/               Rust backend (Tauri 2)
  src/lib.rs             Tauri builder, plugins, command registration
  src/quit.rs            ⌘Q / window close ask the webview to save first (app://quit-requested)
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
  src/notes/             local notes (the vault)
    mod.rs               Notes service: vault choice, read/write, create/rename/move/trash
    commands.rs          Tauri command surface (vault-relative paths in, display data out)
    config.rs            settings.json in the app data dir (vault path) + vault status
    paths.rs             resolve/validate relative paths, NFC + case folding, name sanitising
    scan.rs              walk the vault, describe entries (title, size, flags)
    note.rs              UTF-8/BOM/CRLF handling, title (frontmatter, heading, file name), revision
    write.rs             conflict-checked save, create note/folder
    macos.rs             Foundation calls: Trash, atomic replace, no-overwrite move
    index.rs             what the app last saw (paths, titles, hashes; no content)
    watcher.rs           debounced FSEvents -> reconcile against index -> vault://changed
  capabilities/          webview permissions (core only, no opener, dialog or fs)
  tauri.conf.json        app config, CSP, macOS bundle
  Info.plist             privacy usage strings (Documents, Desktop, Downloads)
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
  a webview. The webview has no opener permission; Rust opens only fixed, validated URLs and
  (reveal / open in default app) notes and folders resolved inside the vault.
- Loopback redirect is `http://127.0.0.1:<port>/auth/callback`. Never `localhost`.
- Send `login_hint`, not `id_token_hint`: `/usr/bin/open` exposes the URL in process arguments.
- Only the documented SIWC flow. No ChatGPT `backend-api`, no cookies, no Codex auth file, no
  API-key path, no other provider, no telemetry or crash reporting.
- Never invent endpoints, parameters or model IDs. Models come from `GET /v1/models`.
- Usage-limit and other non-retryable plan errors are never retried in a loop.
- **All note file access is in Rust.** The webview has no `fs` or `dialog` permission and sends
  only vault-relative paths, which `notes/paths.rs` resolves and confines to the vault (no `..`,
  no absolute paths, no hidden segments, no symbolic links).
- **The files are the source of truth.** No database, cache or copy of note content outlives the
  open editor. Never write frontmatter, IDs or metadata into notes; never create files in the
  vault other than the notes and folders the user asks for.
- **Never destroy user data.** No hard deletes (Move to Trash only). Never overwrite a file that
  changed on disk since it was read (`base_revision` check). Never rename or move onto an
  existing name. Never touch files that aren't `.md` notes (they move with their folder).
- View mode renders Markdown without raw HTML. Links never navigate the webview and images are
  not loaded (the CSP and the no-opener rule stand).
- No quizzes, spaced repetition, search, index, tags or wiki-links yet.

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
- macOS only (14+). No signing, notarization or CI yet.
