#[cfg(feature = "desktop")]
pub mod auto_next;
#[cfg(feature = "desktop")]
pub mod automation;
pub mod capture;
#[cfg(feature = "desktop")]
pub mod desktop;
pub mod imaging;
#[cfg(feature = "desktop")]
mod nickname;
pub mod nickname_debug;
pub mod nickname_ocr;
pub mod ocr;
pub mod ocr_settings;
#[cfg(feature = "desktop")]
pub mod overlay;
pub mod ppocr;
pub mod selection;
pub mod settlement;
pub mod storage;

use std::path::PathBuf;
pub fn resources() -> anyhow::Result<PathBuf> {
    let installed = std::env::current_exe()?.parent().unwrap().join("resources");
    if installed.is_dir() {
        return Ok(installed);
    }
    let dev = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources");
    anyhow::ensure!(dev.is_dir(), "未找到识别资源，请使用完整验证包。");
    Ok(dev)
}
