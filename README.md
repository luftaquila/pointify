# pointify

Retro analog gauge meters for hardware resource and Claude usage limit monitoring.

## Development

## GUI app

### Prerequisites

- [Node.js](https://nodejs.org/)
- [Rust](https://rustup.rs/)

### Build

```bash
cd gui
npm install
npm run tauri dev     # dev
npm run tauri build   # production
```

## Firmware

### Prerequisites

```bash
cargo install probe-rs-tools
```

### Build

```bash
cd device/firmware
cargo run --release     # build + flash
cargo build --release   # build only
```
