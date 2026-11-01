use anyhow::Result;
use image::RgbaImage;
use serde::Serialize;
use std::path::Path;

#[derive(Serialize)]
pub struct SelectionNameEvidence {
    pub position: String,
    pub nickname: String,
    pub upper_left: String,
    pub lower_bar: String,
}

pub struct SelectionRecognizer {
    pub reader: crate::nickname_ocr::NicknameRecognizer,
}

impl SelectionRecognizer {
    pub fn new(root: &Path) -> Result<Self> {
        Self::with_backend(root, crate::ocr_settings::OcrBackend::WindowsTesseract)
    }

    pub fn with_backend(root: &Path, backend: crate::ocr_settings::OcrBackend) -> Result<Self> {
        Ok(Self {
            reader: crate::nickname_ocr::NicknameRecognizer::new(root, backend)?,
        })
    }

    pub fn recognize(&mut self, image: &RgbaImage) -> Result<Vec<SelectionNameEvidence>> {
        self.recognize_traced(image, None)
    }

    pub fn recognize_traced(
        &mut self,
        image: &RgbaImage,
        trace: Option<&crate::nickname_debug::TraceContext>,
    ) -> Result<Vec<SelectionNameEvidence>> {
        // Apex's Legend Selection screen repeats the same three slots at the upper
        // left and in the colored player banners at the bottom. Combine both OCR
        // engines' readings by slot, while retaining the two regional readings.
        let mut members = Vec::with_capacity(3);
        for (slot, (position, y, upper_w, bottom_x, bottom_w)) in [
            // The top-left list starts near the left edge; use a per-row width so
            // the OCR area ends shortly after the name instead of including UI.
            ("队伍 1", 0.112, 0.075, 0.150, 0.075),
            ("队伍 2", 0.184, 0.100, 0.470, 0.075),
            ("队伍 3", 0.256, 0.160, 0.775, 0.105),
        ]
        .into_iter()
        .enumerate()
        {
            let upper_crop = crate::imaging::region(image, 0.073, y, upper_w, 0.040, 3.0);
            let upper = self
                .reader
                .candidates(&upper_crop, slot, "upper_left", true, trace)?;
            let upper_left = choose_nickname(
                upper
                    .iter()
                    .map(|(text, preferred)| (text.as_str(), *preferred)),
            );
            let lower_crop = crate::imaging::region(image, bottom_x, 0.963, bottom_w, 0.025, 3.0);
            let lower = self
                .reader
                .candidates(&lower_crop, slot, "lower_bar", true, trace)?;
            let lower_bar = choose_nickname(
                lower
                    .iter()
                    .map(|(text, preferred)| (text.as_str(), *preferred)),
            );
            let nickname = choose_nickname(
                upper
                    .iter()
                    .chain(&lower)
                    .map(|(text, preferred)| (text.as_str(), *preferred)),
            );
            members.push(SelectionNameEvidence {
                position: position.to_string(),
                nickname,
                upper_left: upper_left.trim().to_string(),
                lower_bar: lower_bar.trim().to_string(),
            });
            if let Some(trace) = trace {
                trace.event(
                    "region_choice",
                    serde_json::json!({"slot":slot,"result":members.last()}),
                );
            }
        }
        Ok(members)
    }
}

pub fn recognize(image: &RgbaImage, root: &Path) -> Result<Vec<SelectionNameEvidence>> {
    SelectionRecognizer::new(root)?.recognize(image)
}

fn choose_nickname<'a>(candidates: impl IntoIterator<Item = (&'a str, bool)>) -> String {
    let mut groups: Vec<(String, String, usize, usize)> = Vec::new();
    for (candidate, is_windows_ocr) in candidates {
        let display = normalize_name(candidate);
        if display.is_empty() {
            continue;
        }
        let key = display.to_lowercase();
        if let Some((_, _, count, windows_count)) = groups.iter_mut().find(|g| g.0 == key) {
            *count += 1;
            *windows_count += usize::from(is_windows_ocr);
        } else {
            groups.push((key, display, 1, usize::from(is_windows_ocr)));
        }
    }
    groups
        .into_iter()
        .max_by_key(|(_, _, count, windows_count)| (*count, *windows_count))
        .map(|(_, display, _, _)| display)
        .unwrap_or_default()
}

fn normalize_name(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed
        .chars()
        .any(|c| ('\u{3400}'..='\u{9fff}').contains(&c))
    {
        trimmed.chars().filter(|c| !c.is_whitespace()).collect()
    } else {
        trimmed.split_whitespace().collect::<Vec<_>>().join(" ")
    }
}
