# Better Apex

当前主程序使用 Rust + Tauri 2 + Vue 3。Rust 负责窗口采集、OCR、自动记录、本地 SQLite 和系统集成；Vue 负责单窗口界面。自动记录流程及其边界见[自动采集流程计划](../docs/自动采集流程计划.md)和[阶段 D](../docs/Rust阶段D自动监测.md)。OCR 引擎见[可选 OCR 说明](../docs/可选OCR引擎.md)。

旧版 WPF 源码不参与当前程序构建。仅保留迁移过程中形成的设计决策、旧战绩边界，以及 Tesseract 引擎所需的语言模型与许可，见[旧版经验与迁移边界](../docs/旧版经验与迁移边界.md)。

## 开发环境

Windows x64、Rust MSVC 工具链、Visual Studio C++ 编译工具及 Windows SDK、Node.js 和 WebView2。Rust 与 npm 依赖通过锁文件管理。

在项目根目录执行：

```powershell
cd rust-app
npm ci
cd ..
./scripts/Prepare-RustResources.ps1
./scripts/Prepare-PpOcr.ps1 -DownloadMissing
$env:CARGO_TARGET_DIR = Join-Path $PWD '.local/rust-target-gui'
cd rust-app
npm run tauri dev
```

`Prepare-RustResources.ps1` 从仓库内的 `assets/tesseract/tessdata/` 复制语言模型，并从本机 NuGet 缓存复制 Tesseract 原生 DLL。干净开发机需先准备对应的 Tesseract 5.2.0 x64 DLL。`Prepare-PpOcr.ps1 -DownloadMissing` 仅在明确需要下载模型时使用。生成资源、构建缓存和前端依赖不纳入 Git。

## 验证

按改动范围选择命令，不需要每次全跑：

```powershell
$env:CARGO_TARGET_DIR = Join-Path $PWD '.local/rust-target-gui'
cargo test --manifest-path rust-app/src-tauri/Cargo.toml --lib --offline
cd rust-app
npm.cmd test
npm.cmd run build
```

默认不启动桌面程序或操作游戏做验收。发布便携版使用项目根目录的 `scripts/Publish-RustPortable.ps1`；它执行 Tauri 正式构建并准备随包资源，不生成安装包或 ZIP。普通 `cargo --release` 不能代替正式 Tauri 构建。

旧版战绩不导入新版。根目录 `data/` 仅为本机旧数据；`compatibility-cli` 可按需对其做只读校验，不属于主程序启动依赖。
