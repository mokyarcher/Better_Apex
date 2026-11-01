# Better Apex Rust Lab

当前交付与验证约定（2026-11-01）：用户反馈此前版本长时间游戏测试暂未发现问题；新增自动下一局已完成45项Rust、3项前端回归及正式构建，实际游戏按键接收尚未验证。该功能默认开启，可在设置关闭；仅完整数字自动新保存成功后，确认前台新结算画面及重新匹配提示，每局最多按一次主键盘数字1，切出／停止／超时取消。

按用户要求，后续不操控桌面跑测试，不自动启动程序做UI验收；仅做必要的代码检查、有限命令行验证和编译，文档更新不编译或测试。以下桌面采集、UI脚本及旧阶段记录仅作参考，未经用户明确要求不执行。

当前PP-OCR模式已覆盖结算排名、击杀／助攻／击倒、伤害及卡片匹配线索，原引擎仍可选；自动复核只重读数字，确认后读取一次卡片线索。修复未完成结算进入下一局时仍停留03的问题。昵称最终来源仍只取同局02名单。详见[可选OCR说明](../docs/可选OCR引擎.md)。下方带日期旧引擎范围说明保留作历史记录。


1080P／2K识别自动适配已完成本机离线回归，无需在设置里选择分辨率。测试包含1080P缩图的页面、昵称、结算数值和证据保存；范围见[1080P适配](../docs/1080P识别适配.md)。

当前规则（2026-11-01）：连续两帧确认局内HUD后，只在确认帧读取一次左下三人昵称，各槽位直接采用这次结果，不计票、不重试。空结果或错误占用唯一一次尝试并保留未知；切出恢复不重新读取，新一局才重置。02保持常规2秒采集，只有结算确认临时改为1秒。最终自动昵称只来自同局02单次识别名单；03只作不可信的一对一匹配线索，不能补名。存储字段`voted_roster`保留为兼容标签，当前表示02自动名单，不代表执行了投票。下方带日期的旧采样说明为历史记录。

Rust＋Tauri 已完成阶段 A/B、C1/C1b、C2/C3，并接入阶段 D 自动记录。自动流按新原图验证 00 加载、01 选传奇、02 局内 HUD、03 结算；包含局内单次昵称识别、结算 1 秒确认、保存去重及下一局状态转换。用户已反馈此前版本长时间游戏测试暂未发现问题；新增自动下一局按键及实际性能仍以对应实测为准。新版不提供旧版战绩导入或读取；旧 WPF 程序继续保留。

当前识别说明见 [阶段 C1 说明](../docs/Rust阶段C1结算结构化识别.md)，新版存储与手动关联见 [阶段 C2 说明](../docs/Rust阶段C2新版数据库初始化.md) 和 [阶段 C3 说明](../docs/Rust阶段C3手动关联与保存.md)，自动记录见[阶段 D 说明](../docs/Rust阶段D自动监测.md)，界面说明见 [阶段 B 界面说明](../docs/Rust阶段B界面说明.md)，总体设计见 [改造计划](../docs/Rust-Tauri技术与UI改造计划.md)。

## 本机开发

昵称 OCR 支持设置页选择 PP-OCRv6 small／原 Windows OCR + Tesseract；无设置时默认新模型。切换前需停止自动记录，设置存入新版数据目录的 `ocr-settings.json`，不会修改历史战绩。详细适用范围、静态对比与性能边界见 [可选 OCR 引擎](../docs/可选OCR引擎.md)。

开发环境：Windows x64、Rust MSVC 工具链、Visual Studio C++ 编译工具及 Windows SDK、Node.js、WebView2、Windows 简体中文 OCR。Rust 与 npm 依赖均使用锁文件。

在项目根目录执行：

```powershell
cd rust-app
npm ci
cd ..
./scripts/Prepare-RustPrototype.ps1
./scripts/Prepare-PpOcr.ps1 -DownloadMissing
$env:CARGO_TARGET_DIR = Join-Path $PWD '.local/rust-target-gui'
cd rust-app
npm run tauri dev
```

资源准备脚本复制已有模型和 `.local/packages/tesseract/5.2.0/x64` 中的 DLL；源文件不修改。干净检出需先还原既有 Tesseract NuGet 包，或按相同版本准备这两份原生 DLL。生成的资源、编译缓存、前端依赖均不纳入版本管理。

## 验证

```powershell
# 在项目根目录执行；只读现有数据库并检查字节哈希。
# 这是阶段 A 的开发验证命令，不属于 Rust 正式界面功能；新版不导入或读取旧版战绩。
$env:CARGO_TARGET_DIR = Join-Path $PWD '.local/rust-target'
cargo run --manifest-path rust-app/src-tauri/Cargo.toml --no-default-features --bin compatibility-cli -- image_for_test data/matches.db
cargo test --manifest-path rust-app/src-tauri/Cargo.toml --lib
# 此脚本只创建自己的 WPF 测试窗口；采集与判定由 Rust 进程完成。
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/Test-RustPrototypeCapture.ps1
```

`scripts/Test-RustPrototypeUi.mjs` 使用独立本地 WebView2 调试端口，对自有应用做端到端测试。参数为原型 exe、隔离输出目录、项目根目录。它不连接用户浏览器，文件选择器返回由测试提供，业务命令仍真实执行。

默认从项目根目录构建并更新直接运行目录（自动构建前端，不生成安装包或压缩包）：

```powershell
./scripts/Publish-RustPortable.ps1
```

该脚本使用 `tauri build --no-bundle --features custom-protocol` 并复制 EXE 和 resources。仅使用普通 `cargo build --release` 会保留开发页面地址，产生 `127.0.0.1` 拒绝连接的错误交付；桌面主程序已加编译保护，Release 缺少 `custom-protocol` 时直接失败。按用户最新要求，默认不启动EXE或操作桌面做验收；交付说明区分编译通过与实际启动／游戏交互已验证，后者由用户实测，除非另有明确要求。

自动状态已改为 Rust 事件通知；自动保存与 pending 恢复会刷新历史、数量和最近一场，保留正在查看的旧记录。窗口恢复时补读完整状态，采集检查以外不再定时查询自动状态。前端同步测试在本目录运行 `npm.cmd test`（Node.js 24），覆盖保留详情、并发刷新和失败重试。

结算回退反馈已修复：无新帧超时只显示暂停并保留对局状态；观察结算后不接受无新局入场证据的 HUD 回退。UI 由后台阶段枚举驱动，长中断后无法证明连续性的旧名单不参与关联。该次修复通过的 15 项测试含四张基线原图、用户结算附件及合成中断／误判序列；详见阶段 D。

2026-11-01：昵称采样扩至选传奇 5 秒／最多 5 帧、确认 HUD 后 10 秒／最多 10 帧，每人最多 20 份读数；完整写法分开计票，结合有效票占比、领先幅度、独立帧和来源支持进行决策。一对一匹配后才合并跨阶段读数，未确认不强行补全。仅保留有界文字样本及每阶段一张图，复用自动线程的 Windows OCR 实例。当前共有 21 项 Rust 与 3 项前端测试通过；准确率和游戏性能增量仍需游戏电脑验证。

正式安装和运行不依赖上述开发环境。UI 为本地 Vue 页面，所有采集、识别和数据库操作在 Rust 完成。

临时调试日志：按用户指示，当前版本每次运行默认启用昵称日志，覆盖自动／手动实际 OCR、区域融合、规范化、投票及最终关联和保存 ID。数据目录下 `nickname-debug/` 每次运行新建同名前缀 TXT／JSONL，设置页可打开目录、查看写入错误或关闭本次记录；关闭不删旧文件，重启默认开启。日志 I/O 故障不影响识别和保存。当前 24 项 Rust、3 项前端测试通过；日志不新增截图或重复 OCR。具体字段与复盘步骤见阶段 D 和自动记录测试说明。

身份来源已严格分离：最终自动昵称只使用同局 01／02 投票结果，03 卡片文字字段 `nickname_hint` 仅作粗对齐证据，不参与投票，不兜底展示或保存。PlayerAnnotation 必须有 voted_roster／manual 来源，否则昵称和本人身份为空；手动导入不预填结算文字，旧 pending 缺来源也不复用昵称。现有历史不据猜测批量改写。当前 26 项 Rust 及 3 项前端测试通过，包含 vjkook／旗帜杂字及来源约束回归。
