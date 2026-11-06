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
