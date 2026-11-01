use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    sync::Mutex,
};

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OcrBackend {
    WindowsTesseract,
    #[default]
    PpOcrV6Small,
}

impl OcrBackend {
    pub fn label(self) -> &'static str {
        match self {
            Self::WindowsTesseract => "Windows OCR + Tesseract",
            Self::PpOcrV6Small => "PP-OCRv6 small",
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Settings {
    nickname_backend: OcrBackend,
}

#[derive(Serialize)]
pub struct OcrSettingsStatus {
    pub backend: OcrBackend,
    pub ppocr_ready: bool,
    pub error: String,
}

pub struct OcrSettings {
    path: PathBuf,
    value: Mutex<(Settings, String)>,
}

impl OcrSettings {
    pub fn open(data_dir: &Path) -> Self {
        let path = data_dir.join("ocr-settings.json");
        let value = match std::fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice(&bytes) {
                Ok(settings) => (settings, String::new()),
                Err(error) => (
                    Settings {
                        nickname_backend: OcrBackend::WindowsTesseract,
                    },
                    format!("OCR 设置读取失败，暂用原引擎；请选择并保存设置：{error}"),
                ),
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                (Settings::default(), String::new())
            }
            Err(error) => (
                Settings {
                    nickname_backend: OcrBackend::WindowsTesseract,
                },
                format!("OCR 设置读取失败：{error}"),
            ),
        };
        Self {
            path,
            value: Mutex::new(value),
        }
    }

    pub fn backend(&self) -> OcrBackend {
        self.value.lock().unwrap().0.nickname_backend
    }

    pub fn status(&self, root: &Path) -> OcrSettingsStatus {
        let value = self.value.lock().unwrap();
        OcrSettingsStatus {
            backend: value.0.nickname_backend,
            ppocr_ready: crate::ppocr::assets_present(root),
            error: value.1.clone(),
        }
    }

    // Called while AutoManager's worker lock is held, so a start cannot race a switch.
    pub fn save(&self, backend: OcrBackend) -> Result<()> {
        let mut value = self.value.lock().unwrap();
        let settings = Settings {
            nickname_backend: backend,
        };
        let parent = self.path.parent().context("OCR 设置目录无效")?;
        std::fs::create_dir_all(parent)?;
        let temporary = self.path.with_extension("json.tmp");
        std::fs::write(&temporary, serde_json::to_vec_pretty(&settings)?)?;
        std::fs::rename(&temporary, &self.path).context("无法保存 OCR 设置")?;
        *value = (settings, String::new());
        Ok(())
    }
}

pub fn require_idle(running: bool) -> Result<()> {
    ensure!(
        !running,
        "请先停止自动记录，再切换 OCR 引擎；同一局不能中途混用引擎。"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn backend_settings_persist_and_invalid_settings_are_reported() -> Result<()> {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!(
            "../../.local/storage-tests/ocr-settings-{}",
            time::OffsetDateTime::now_utc().unix_timestamp_nanos()
        ));
        let settings = OcrSettings::open(&root);
        assert_eq!(settings.backend(), OcrBackend::PpOcrV6Small);
        settings.save(OcrBackend::WindowsTesseract)?;
        assert_eq!(
            OcrSettings::open(&root).backend(),
            OcrBackend::WindowsTesseract
        );
        settings.save(OcrBackend::PpOcrV6Small)?;
        assert_eq!(OcrSettings::open(&root).backend(), OcrBackend::PpOcrV6Small);
        std::fs::write(root.join("ocr-settings.json"), b"invalid settings")?;
        let invalid = OcrSettings::open(&root);
        assert_eq!(invalid.backend(), OcrBackend::WindowsTesseract);
        assert!(!invalid.status(&root).error.is_empty());
        assert!(require_idle(true).is_err());
        std::fs::remove_dir_all(root)?;
        Ok(())
    }
}
