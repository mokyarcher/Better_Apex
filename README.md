# Better Apex

Apex 玩家本地战绩工具。当前主程序为 Rust + Tauri 2 + Vue 3，通过低频窗口采集和本地 OCR 记录结算，战绩与证据保存在本机；不读取游戏内存、不上传数据，也不导入旧版战绩。

## 当前能力

00→01→02→03 自动记录、局内名单与结算数据关联、本地历史和手动补录、PP-OCRv6 small / Windows OCR + Tesseract 选择、托盘与单窗口、可选游戏状态浮窗，以及满足严格条件后的自动下一局。

未识别的数据保留未知；静态回归、编译结果和游戏电脑实测分别报告，不互相替代。具体现行规则以功能规范为准。

## 项目文档

从[文档目录与维护规范](docs/README.md)开始。当前状态见[开发计划与进度](docs/03-planning/开发计划与进度.md)；开发和构建见[运维说明](docs/06-operations/开发构建与游戏电脑验收.md)。

## 目录

- `rust-app/`：当前主程序；快速入口见[开发说明](rust-app/README.md)。
- `docs/01-product`—`06-operations/`：产品、架构、计划、功能规范、验证和运行文档。
- `docs/archive/`：旧阶段记录，仅作历史背景。
- `assets/tesseract/tessdata/`：可选 Tesseract 引擎的模型和许可。
- `image_for_test/`：用于有限页面/OCR回归的原始和派生样本。
- 根目录 `data/`、`.local/`、`artifacts/`：本机旧数据、缓存/诊断资料和产物；按[目录维护规范](docs/README.md)处理，不要整目录盲删。

产品范围见[产品需求](docs/01-product/产品范围与需求.md)。
