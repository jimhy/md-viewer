**English** | [中文](README_CN.md)

# MD Viewer

A lightweight, beautiful Markdown viewer for Windows. Single portable executable, no installation required.

**[Download Latest Release](/releases/latest)**

## Features

- **Instant Preview** — Double-click any `.md` file to view it beautifully rendered
- **Syntax Highlighting** — Code blocks with dark theme, line numbers, and one-click copy
- **Image Support** — Automatically embeds local images (both markdown and HTML `<img>` syntax)
- **Custom Title Bar** — Clean, modern UI with drag-to-move and double-click to maximize
- **Dark/Light Mode** — Automatically follows your Windows system theme
- **Resizable Window** — Drag edges to resize, window size remembered across sessions
- **Drag & Drop** — Drop `.md` files onto the window or the exe to open
- **Portable** — Single `.exe` (~2MB), no runtime dependencies (uses system WebView2)

## Usage

### Quick Start

1. Download `md-viewer.exe`
2. Drag a `.md` file onto it, or run from command line:
   ```
   md-viewer.exe path/to/file.md
   ```

### File Association

Register `.md` files to always open with MD Viewer:

```
install.bat
```

To remove the association:

```
uninstall.bat
```

### Controls

| Action | Method |
|--------|--------|
| Close | Click X button |
| Maximize/Restore | Double-click title bar |
| Copy code block | Click copy button on hover |
| Resize window | Drag window edges |

## Supported Markdown Syntax

- Headings (h1-h6)
- Bold, italic, strikethrough
- Ordered and unordered lists
- Task lists (checkboxes)
- Tables
- Code blocks with syntax highlighting (50+ languages)
- Blockquotes
- Images (local and remote)
- Links (open in default browser)
- Horizontal rules
- HTML inline elements (`<p>`, `<img>`, `<details>`, etc.)

## Build from Source

### Prerequisites

- [Rust](https://rustup.rs/) (stable)
- Windows 10/11 with WebView2 runtime (pre-installed on modern Windows)

### Build

```bash
cargo build --release
```

The executable will be at `target/release/md-viewer.exe`.

### Release Build (with version bump)

```bash
python release.py
```

Automatically increments the patch version in `Cargo.toml`, builds the release, and copies the exe to the project root.

## Tech Stack

- **Rust** — Fast startup, small binary
- **wry** — WebView2 wrapper for rendering HTML
- **tao** — Window management
- **pulldown-cmark** — Markdown parsing
- **syntect** — Syntax highlighting (base16-ocean.dark theme)

## License

MIT
