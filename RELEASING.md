# 发布与自动更新

## 一、发布新版本（GitHub Actions 自动编译+发版）

流水线：`.github/workflows/release.yml`，在 **Windows runner** 上编译 release exe、用 Inno Setup 打安装包，并创建 GitHub Release、上传 `md-viewer-setup-v{版本}.exe`。

### 标准发版步骤

1. **升级版本号**：改 `Cargo.toml` 里的 `version`（可用现成的 `python release.py` 帮你 bump，或手动改）。
2. **提交**：`git commit -am "release: vX.Y.Z"`。
3. **打 tag 并推到 GitHub**（tag 用**纯版本号、无 `v` 前缀**，沿用历史约定）：
   ```bash
   git tag 1.0.17
   git push github 1.0.17     # 注意是 github remote，不是 gitee 的 origin
   ```
4. 流水线自动触发 → 编译 → 打包 → 创建 Release `1.0.17` 并上传安装包。

> tag 必须与 `Cargo.toml` 的版本**完全一致**，否则流水线会在版本校验步骤失败（防止忘记 bump）。

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
- Release 的 tag 命名务必保持 `X.Y.Z`（无 `v`）以与历史一致；客户端版本比对对带不带 `v` 都兼容，但 tag 与 Cargo.toml 校验要求二者字面一致。
