[English](README.md) | **中文**

# MD Viewer

快速、美观的 Windows Markdown 查看器 **+ 编辑器**。多 Tab、文件侧栏、实时预览、语法高亮、粘贴即存图 —— Rust + WebView2 打造，安装包约 3 MB。

![MD Viewer 截图](docs/screenshot2.png)

**[⬇ 下载最新版本](../../releases/)**

## 功能特性

### 查看
- **精美渲染** — 支持 GitHub 风格 Markdown：表格、任务列表、脚注、删除线
- **语法高亮** — 50+ 种语言，带行号 + 一键复制
- **自动内嵌图片** — 本地图片自动转 data URI（支持 Markdown 和 HTML `<img>` 两种写法）
- **大纲侧栏** — 自动按 h1/h2/h3 生成目录，滚动时高亮当前章节
- **文件侧栏** — 浏览同目录下所有 `.md`，单击预览、双击固化为永久 Tab
- **深色/浅色主题** — 自动跟随 Windows 系统主题

### 编辑
- **三种模式** — `查看`（只读） / `编辑`（纯编辑器） / `双栏`（编辑 + 实时预览）
- **光标-预览联动** — 双栏模式下预览会跟随编辑器光标所在块滚动
- **Markdown 工具栏** — 标题、加粗、斜体、列表、表格、代码、链接、图片
- **Slash 命令** — 在编辑器中输入 `/` 弹出菜单，快速插入各种块
- **粘贴图片** — `Ctrl+V` 粘贴图片：自动保存到文档同级 `images/`，链接自动插入
- **保存** — `Ctrl+S` 写回磁盘；关闭时若有未保存修改会弹窗提示

### 多 Tab 与窗口
- **多 Tab** — 打开任意多个文档，可拖入新增、中键关闭
- **预览 Tab 与固定 Tab** — 文件树单击是预览（斜体显示），双击或编辑则固化
- **外部修改自动刷新** — 文件被其它编辑器改写后渲染自动更新（用户未保存的修改会被保留）
- **文档间链接跳转** — 渲染后的 Markdown 中点击 `[中文](README_CN.md)` 这类 `.md` 链接，会作为预览 Tab 在应用内打开
- **单实例** — 从资源管理器再次打开 `.md` 会路由到已有窗口
- **自定义标题栏** — 无边框、拖拽移动、双击最大化、边缘可调整大小
- **记忆窗口尺寸** — 关闭后自动保存窗口大小

## 安装

1. 从 [Releases](../../releases/) 下载 `md-viewer-setup-vX.Y.Z.exe`
2. 运行安装包，默认会关联 `.md` / `.markdown` 文件，并在右键菜单加入 **Open with MD Viewer**
3. 双击任意 `.md` 文件即可打开，或命令行运行 `md-viewer.exe 文件路径.md`

> 安装包默认装到用户目录，不需要管理员权限。仅依赖 Windows 10/11 系统自带的 WebView2。

## 快捷键

### 文件 & 窗口
| 快捷键 | 操作 |
|---|---|
| `Ctrl+O` | 打开文件对话框 |
| `Ctrl+S` | 保存当前 Tab |
| 拖放文件 | 打开一个或多个 `.md` 文件 |
| 中键点 Tab | 关闭该 Tab |
| 双击标题栏 | 最大化 / 还原 |

### 格式化（编辑 / 双栏模式）
| 快捷键 | 操作 |
|---|---|
| `Ctrl+1` / `Ctrl+2` / `Ctrl+3` | 标题 H1 / H2 / H3 |
| `Ctrl+B` | **加粗** |
| `Ctrl+I` | *斜体* |
| `Ctrl+Shift+X` | ~~删除线~~ |
| `Ctrl+E` | 行内 `代码` |
| `Ctrl+Shift+E` | 代码块（围栏式） |
| `Ctrl+Q` | 引用块 |
| `Ctrl+L` | 无序列表 |
| `Ctrl+Shift+L` | 有序列表 |
| `Ctrl+T` | 任务列表 |
| `Ctrl+K` | 链接 |
| `Ctrl+Shift+I` | 图片 |
| `Ctrl+Shift+M` | 表格 |
| `Ctrl+Shift+H` | 分隔线 |
| `Ctrl+Z` / `Ctrl+Y` | 撤销 / 重做 |
| `/`（编辑器内） | 弹出 Slash 命令菜单 |

## 支持的 Markdown 语法

标题 · 加粗 · 斜体 · 删除线 · 有序/无序/任务列表 · 表格 · 围栏代码块（语法高亮） · 引用 · 图片（本地/网络） · 链接 · 分隔线 · 内联 HTML（`<p>`、`<img>`、`<details>` 等）。

## 从源码构建

### 前置条件
- [Rust](https://rustup.rs/)（stable）
- Windows 10/11（自带 WebView2）
- （可选，打安装包用）[Inno Setup 6](https://jrsoftware.org/isdl.php)

### 编译
```bash
cargo build --release
```
产物：`target/release/md-viewer.exe`。

### 发布（自动递增版本号 + 打安装包）
```bash
python release.py
```
自动递增 `Cargo.toml` 的 patch 版本号，编译 release，然后调用 Inno Setup 生成 `dist/md-viewer-setup-vX.Y.Z.exe`。

## 技术栈

- **Rust** — 启动快，安装包约 3 MB
- **[wry](https://github.com/tauri-apps/wry)** — WebView2 封装
- **[tao](https://github.com/tauri-apps/tao)** — 窗口管理
- **[pulldown-cmark](https://github.com/raphlinus/pulldown-cmark)** — Markdown 解析
- **[syntect](https://github.com/trishume/syntect)** — 语法高亮（base16-ocean.dark 主题）

## 许可证

MIT
