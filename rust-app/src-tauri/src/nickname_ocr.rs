use crate::{
    nickname_debug::{record_ocr, TraceContext},
    ocr_settings::OcrBackend,
};
use anyhow::Result;
use image::RgbaImage;
use std::path::Path;

/// One selected backend per recording worker; instances live across samples.
pub enum NicknameRecognizer {
    Original {
        windows: crate::ocr::WindowsRecognizer,
        tesseract: crate::ocr::Tesseract,
    },
    Paddle(crate::ppocr::PpOcr),
}

impl NicknameRecognizer {
    pub fn new(root: &Path, backend: OcrBackend) -> Result<Self> {
        match backend {
            OcrBackend::WindowsTesseract => Ok(Self::Original {
                windows: crate::ocr::WindowsRecognizer::new()?,
                tesseract: crate::ocr::Tesseract::new(root, "chi_sim+eng")?,
            }),
            OcrBackend::PpOcrV6Small => Ok(Self::Paddle(crate::ppocr::PpOcr::new(root)?)),
        }
    }

    // Original selection keeps both engines and Windows tie preference.
    // Paddle contributes one vote per region, never one per detected line.
    pub fn candidates(
        &mut self,
        image: &RgbaImage,
        slot: usize,
        source: &str,
        dual: bool,
        trace: Option<&TraceContext>,
    ) -> Result<Vec<(String, bool)>> {
        match self {
            Self::Original { windows, tesseract } => {
                let tess = if dual {
                    Some(
                        record_ocr(
                            trace,
                            slot,
                            source,
                            "tesseract",
                            tesseract.read(image, 7, None),
                        )?
                        .0,
                    )
                } else {
                    None
                };
                let windows = record_ocr(trace, slot, source, "windows", windows.read(image))?;
                let mut results = vec![(windows, true)];
                if let Some(tess) = tess {
                    results.push((tess, false));
                }
                Ok(results)
            }
            Self::Paddle(engine) => {
                let result = record_ocr(trace, slot, source, "ppocr_v6_small", engine.read(image))?;
                Ok(vec![(result.text, true)])
            }
        }
    }

    pub fn read(
        &mut self,
        image: &RgbaImage,
        slot: usize,
        source: &str,
        trace: Option<&TraceContext>,
    ) -> Result<String> {
        Ok(self
            .candidates(image, slot, source, false, trace)?
            .remove(0)
            .0)
    }
}
