# 发布与自动更新

## 一、发布新版本（GitHub 与 Gitee 双平台）

流水线：`.github/workflows/release.yml`，在 **Windows runner** 上编译 release exe、用 Inno Setup 打安装包，并创建 GitHub Release、上传 `md-viewer-setup-v{版本}.exe`。Gitee Release 使用同一安装包手动发布，保证两个平台的文件完全一致。

### 标准发版步骤

1. **升级版本号**：同步修改 `Cargo.toml` 和 `Cargo.lock` 中本项目的 `version`（也可用 `python release.py` bump 后再检查）。
2. **完整验证**：至少运行 `cargo test --locked` 和 `cargo build --release --locked`；涉及渲染或界面时还要做真实 WebView2 冒烟测试。
3. **显式暂存并提交**：检查 `git status --short`，把新增资源文件一并 `git add`，不要只用 `git commit -am`，否则未跟踪文件不会进入提交。
4. **先把 `main` 推到两个远端**：
   ```bash
   git push github main
   git push origin main
   ```
5. **分别创建并推送标签**（GitHub 无 `v`，Gitee 有 `v`）：
   ```bash
   git tag X.Y.Z
   git tag vX.Y.Z
   git push github X.Y.Z
   git push origin vX.Y.Z
   ```
6. GitHub 流水线自动编译、打包并创建 Release `X.Y.Z`。等待成功后下载 `md-viewer-setup-vX.Y.Z.exe`，记录 SHA-256。
7. 在 Gitee 为标签 `vX.Y.Z` 创建 Release，标题使用 `vX.Y.Z`，上传刚从 GitHub Release 下载的同一个安装包。
8. 最终检查两个 Release 均可访问、安装包名称正确且 SHA-256 一致。

> GitHub tag 必须与 `Cargo.toml` 的版本**完全一致**且不带 `v`，否则流水线会在版本校验步骤失败。Gitee 继续沿用带 `v` 的历史命名。

### 手动触发（备用）

GitHub 网页 Actions → Release → Run workflow，可填版本号（留空则用 `Cargo.toml` 的版本），会据此建 tag 与 Release。

### 前置条件

- 代码需推到 GitHub remote（`git@github.com:jimhy/md-viewer.git`）。当前本地 `main` 跟踪的是 gitee 的 `origin`，发版相关的 push（分支与 tag）要指到 `github` remote。
- 仓库需为 **public**（自动更新的终端用户要能匿名下载 Release 资产）——现已是 public。

## 二、自动更新（客户端行为）

- App 每次启动约 4 秒后，后台静默请求 `https://api.github.com/repos/jimhy/md-viewer/releases/latest`，比对 `tag_name` 与当前内置版本（`Cargo.toml` 的 version）。
- **仅当** GitHub 上的版本**严格更新**时，顶部弹出蓝色横幅「发现新版本 vX.Y.Z，是否更新？」，带「稍后」「立即更新」。
- 点「立即更新」：
  - 若有**未保存**的文档，会先提示「更新前请先保存」，不继续（避免丢数据）；
  - 否则后台用系统 `curl` 下载安装包到临时目录 → 运行安装包（其 `CloseApplications=force` 会关掉当前旧版）→ 当前进程退出。用户在安装向导里完成安装，结束可勾选「启动 MD Viewer」。
- 查更新/下载**失败一律静默或可重试**，绝不阻塞正常使用。
- 技术实现：HTTP 走 **Windows 自带的 `curl.exe`**（Win10 1803+），无第三方 HTTP/TLS 依赖，二进制体积基本不变。

## 三、注意事项

- 发布后，旧版本用户下次启动会在 ~4 秒后看到更新提示。
- 若要「静默无感更新 + 自动重启」，可把 `installer.iss` 的 `RestartApplications` 改为 `yes` 并在 `run_update` 里给安装包加 `/SILENT` 参数（当前为带向导的可见安装）。
- GitHub Release 的 tag 务必保持 `X.Y.Z`（无 `v`）；Gitee Release 使用 `vX.Y.Z`。客户端版本比对兼容两种形式。
