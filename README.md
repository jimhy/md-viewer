[English](README_EN.md) | **中文**

# MD Viewer

轻量美观的 Windows Markdown 查看器。单文件便携，无需安装。

**[下载最新版本](https://gitee.com/jimyliu/md-viewer/releases/latest)**

## 功能特性

- **即时预览** — 双击 `.md` 文件即可查看渲染效果
- **语法高亮** — 代码块深色主题 + 行号 + 一键复制
- **图片支持** — 自动内嵌本地图片（markdown 和 HTML `<img>` 两种写法）
- **自定义标题栏** — 简洁现代，支持拖拽移动和双击最大化
- **深色/浅色模式** — 自动跟随 Windows 系统主题
- **窗口调整** — 拖拽边缘调整大小，关闭后自动记住窗口尺寸
- **拖放打开** — 拖拽 `.md` 文件到窗口或 exe 上即可打开
- **便携部署** — 单个 `.exe`（约 2MB），无运行时依赖（使用系统自带 WebView2）

## 使用方法

### 快速开始

1. 下载 `md-viewer.exe`
2. 将 `.md` 文件拖到 exe 上，或命令行运行：
   ```
   md-viewer.exe 文件路径.md
   ```

### 文件关联

注册 `.md` 文件默认用 MD Viewer 打开：

```
install.bat
```

取消关联：

```
uninstall.bat
```

### 操作方式

| 操作 | 方式 |
|------|------|
| 关闭 | 点击 X 按钮 |
| 最大化/还原 | 双击标题栏 |
| 复制代码块 | 悬停时点击复制按钮 |
| 调整窗口大小 | 拖拽窗口边缘 |

## 支持的 Markdown 语法

- 标题（h1-h6）
- 粗体、斜体、删除线
- 有序/无序列表
- 任务列表（复选框）
- 表格
- 代码块语法高亮（支持 50+ 语言）
- 引用块
- 图片（本地和网络）
- 链接（在默认浏览器打开）
- 分隔线
- HTML 内联元素（`<p>`、`<img>`、`<details>` 等）

## 从源码构建

### 前置条件

- [Rust](https://rustup.rs/)（stable）
- Windows 10/11（需要系统自带的 WebView2 运行时）

### 编译

```bash
cargo build --release
```

生成的可执行文件位于 `target/release/md-viewer.exe`。

### 发布构建（自动递增版本号）

```bash
python release.py
```

自动递增 `Cargo.toml` 中的 patch 版本号，编译 release，并将 exe 复制到项目根目录。

## 技术栈

- **Rust** — 启动快，体积小
- **wry** — WebView2 封装，用于渲染 HTML
- **tao** — 窗口管理
- **pulldown-cmark** — Markdown 解析
- **syntect** — 语法高亮（base16-ocean.dark 主题）

## 许可证

MIT
