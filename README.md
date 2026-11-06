# Readback Desktop

An open-source macOS app that will quiz you on your own notes. This early version is a spike:
it proves that **Sign in with ChatGPT** works in a Tauri app, so the app runs on your ChatGPT
plan with no API key. Your credentials stay on your Mac.

## Requirements

- macOS 14 or later, with Xcode Command Line Tools (`xcode-select --install`)
- Rust stable (via [rustup](https://rustup.rs))
- Node.js 22 or later, with Corepack enabled (`corepack enable`), which provides Yarn 4

## Run it

```sh
yarn install
yarn tauri dev
```

## Signing in

1. Click **Continue with ChatGPT**. Your browser opens the ChatGPT sign-in page.
2. Approve Readback, allowing ChatGPT plan usage.
3. Come back to the app, pick a model, and send a prompt.

Readback stores non-secret settings in `~/Library/Application Support/app.readback.desktop/`
and your tokens in the macOS Keychain (item `app.readback.desktop.chatgpt`). Development builds
aren't signed, so after a rebuild macOS may ask whether Readback can use its Keychain item;
choose **Always Allow**.

The terminal running `yarn tauri dev` logs the issued client ID on each sign-in (never a token).

## Checks

```sh
yarn lint
yarn typecheck
yarn format:check
cd src-tauri
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

## Project layout

- `src/`: React + Mantine frontend. It only displays data and calls Tauri commands.
- `src-tauri/`: Rust backend. Sign-in, credential storage and ChatGPT requests live here.

See [CLAUDE.md](CLAUDE.md) for the architecture rules.

## License

[MIT](LICENSE)
