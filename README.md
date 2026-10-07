# API Workbench (Tauri)

Desktop version of **CipherDeck API Workbench** for **Windows, macOS, and Linux**.

Same UI as the VS Code extension: collections, environments, pre-request / post-response scripts (CryptoJS), history, saved responses. HTTP runs in the **Rust host** (no CORS). Data is stored in a local JSON file.

## Features

- Full HTTP client (GET/POST/PUT/PATCH/DELETE/HEAD/OPTIONS)
- Query params, headers, body: raw, form-data, urlencoded, binary, GraphQL
- Pre-request & post-response JavaScript with `pm` API and **CryptoJS**
- Collections + environments (Postman-compatible import/export)
- Request history and saved responses
- Local data file (configurable folder)
- Optional insecure TLS for local HTTPS (`allowInsecureTls` from the UI can be wired; default off)

## Prerequisites

- [Rust](https://www.rust-lang.org/tools/install) 1.70+ (`rustup`)
- [Node.js](https://nodejs.org/) 18+
- Platform build tools:
  - **Windows**: Visual Studio Build Tools (C++), WebView2
  - **macOS**: Xcode Command Line Tools
  - **Linux**: `webkit2gtk`, `libssl`, etc.  
    Ubuntu/Debian example:
    ```bash
    sudo apt update
    sudo apt install libwebkit2gtk-4.0-dev build-essential curl wget libssl-dev \
      libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev
    ```

## Quick start

```bash
cd api-workbench-tauri
npm install
npm run dev          # development window
npm run build        # production installers under src-tauri/target/release/bundle/
```

First `npm run dev` / `build` will compile the Rust backend (may take a few minutes).

## Project layout

```
api-workbench-tauri/
├── dist/
│   └── index.html          # UI (ported from VS Code webview)
├── src-tauri/
│   ├── src/main.rs         # HTTP + state file + folder picker
│   ├── Cargo.toml
│   ├── tauri.conf.json
│   └── icons/              # App icons (replace placeholders before shipping)
├── package.json
└── README.md
```

## Data location

Default data file:

| OS      | Path |
|---------|------|
| Windows | `%APPDATA%\API Workbench\workbench-data.json` |
| macOS   | `~/Library/Application Support/API Workbench/workbench-data.json` |
| Linux   | `~/.local/share/API Workbench/workbench-data.json` |

Use the badge in the top bar → **Change folder…** to pick another directory.

## Tauri commands (Rust ↔ UI)

| Command            | Purpose |
|--------------------|---------|
| `http_request`     | Send HTTP (multipart / base64 body supported) |
| `state_load`       | Read workbench JSON |
| `state_save`       | Atomic write of workbench JSON |
| `reveal_data_file` | Open containing folder in the OS file manager |
| `storage_set`      | Point storage at another folder (or reset to default) |
| `pick_folder`      | Native folder dialog |
| `get_data_file`    | Current data file path |

## Icons

Replace files under `src-tauri/icons/` before releasing:

- `32x32.png`, `128x128.png`, `128x128@2x.png`
- `icon.icns` (macOS), `icon.ico` (Windows)

You can generate a full set from a single PNG with:

```bash
npm install -g @tauri-apps/cli
# or use any icon generator that outputs Tauri’s expected names
```

## Differences from the VS Code extension

| VS Code extension              | Tauri app                          |
|-------------------------------|-------------------------------------|
| Webview + Node `http`/`https` | WebView + `reqwest` in Rust         |
| Extension globalStorage path  | OS app data dir (or custom folder)  |
| `acquireVsCodeApi()` bridge   | `window.__TAURI__.tauri.invoke`     |
| Marketplace install           | Native installer / binary           |

The HTML UI is shared conceptually; only the host bridge and packaging differ.

## License

MIT (same spirit as the original workbench).
