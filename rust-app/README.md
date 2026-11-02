# Rust + Tauri 主程序

当前主程序由 Rust + Tauri 2 + Vue 3 构成。功能规则见[自动记录流程](../docs/04-specs/自动记录流程.md)、[OCR识别](../docs/04-specs/OCR识别.md)和[游戏状态浮窗](../docs/04-specs/游戏状态浮窗.md)；模块边界见[系统架构](../docs/02-architecture/系统架构与决策.md)。

## 入口

- Rust 后端：`src-tauri/src/`
- Vue 界面：`src/Workspace.vue` 与 `src/`
- 前端测试：`tests/`
- Rust 测试：`src-tauri/`

开发依赖、资源准备、开发模式、正式构建、产物路径和游戏电脑检查集中写在[开发、构建与游戏电脑验收](../docs/06-operations/开发构建与游戏电脑验收.md)。按变更选择的验证命令和证据边界见[验证策略与结果](../docs/05-quality/验证策略与结果.md)。
