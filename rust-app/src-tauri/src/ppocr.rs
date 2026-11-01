//! Local, CPU-only PP-OCRv6 small. The game ROIs stay unchanged; detection runs
//! inside each ROI and recognition decodes the detected horizontal text lines.
use anyhow::{ensure, Context, Result};
use image::{
    imageops::{crop_imm, resize, FilterType},
    RgbaImage,
};
use ort::{session::Session, value::Tensor};
use serde::Serialize;
use std::{collections::VecDeque, path::Path, sync::OnceLock, time::Instant};

const DET_FILE: &str = "PP-OCRv6_det_small.onnx";
const REC_FILE: &str = "PP-OCRv6_rec_small.onnx";
const DET_HASH: &str = "090f04abcd9d9a7498bc4ebf677e4cb9bdce1fe4197ddb7e529f1ef44e1ff94f";
const REC_HASH: &str = "6f327246b50388f3c176ae304bd95767ea6dc0c9ae92153ef8cbe210b3c14884";

pub fn assets_present(root: &Path) -> bool {
    let root = root.join("ppocr-v6-small");
    [DET_FILE, REC_FILE, "onnxruntime.dll"]
        .iter()
        .all(|file| root.join(file).is_file())
}

#[derive(Clone, Debug, Serialize)]
pub struct Line {
    pub text: String,
    pub confidence: f32,
    pub bounds: [u32; 4],
}

#[derive(Serialize)]
pub struct TextResult {
    pub text: String,
    pub lines: Vec<Line>,
    pub elapsed_ms: u128,
}

pub struct PpOcr {
    detection: Session,
    recognition: Session,
    characters: Vec<String>,
}

impl PpOcr {
    pub fn new(root: &Path) -> Result<Self> {
        ensure!(assets_present(root), "PP-OCRv6 small 资源不完整，请保留 EXE 旁完整 resources 目录，或在设置中选择原 OCR 引擎。");
        let root = root.join("ppocr-v6-small");
        // Verify pinned model bytes once per process, not on every sample.
        static VERIFIED: OnceLock<std::result::Result<(), String>> = OnceLock::new();
        VERIFIED
            .get_or_init(|| {
                (|| -> Result<()> {
                    use sha2::{Digest, Sha256};
                    for (file, expected) in [(DET_FILE, DET_HASH), (REC_FILE, REC_HASH)] {
                        let hash = format!("{:x}", Sha256::digest(std::fs::read(root.join(file))?));
                        ensure!(hash == expected, "PP-OCRv6 small 模型校验失败：{file}");
                    }
                    Ok(())
                })()
                .map_err(|error| format!("{error:#}"))
            })
            .as_ref()
            .map_err(|error| anyhow::anyhow!(error.clone()))?;
        // Dynamic loading is explicit and local: no PATH lookup, runtime download or Python.
        ort::init_from(root.join("onnxruntime.dll"))?
            .with_name("BetterApex-OCR")
            .commit();
        let session = |file: &str| -> Result<Session> {
            Ok(Session::builder()?
                .with_intra_threads(1)
                .map_err(|e| anyhow::anyhow!(e.to_string()))?
                .with_inter_threads(1)
                .map_err(|e| anyhow::anyhow!(e.to_string()))?
                .with_parallel_execution(false)
                .map_err(|e| anyhow::anyhow!(e.to_string()))?
                .with_intra_op_spinning(false)
                .map_err(|e| anyhow::anyhow!(e.to_string()))?
                .with_inter_op_spinning(false)
                .map_err(|e| anyhow::anyhow!(e.to_string()))?
                .commit_from_file(root.join(file))?)
        };
        let detection = session(DET_FILE).context("加载 PP-OCRv6 small 检测模型失败")?;
        let recognition = session(REC_FILE).context("加载 PP-OCRv6 small 识别模型失败")?;
        let dictionary = recognition
            .metadata()?
            .custom("character")
            .context("模型缺少内嵌字符表")?;
        let mut characters = vec![String::new()]; // CTC blank = 0
        characters.extend(dictionary.lines().map(str::to_string));
        characters.push(" ".into());
        ensure!(characters.len() > 6000, "PP-OCRv6 字符表异常");
        Ok(Self {
            detection,
            recognition,
            characters,
        })
    }

    pub fn read(&mut self, image: &RgbaImage) -> Result<TextResult> {
        let started = Instant::now();
        ensure!(image.width() > 0 && image.height() > 0, "OCR 裁图为空");
        let max_side = image.width().max(image.height()) as f64;
        let ratio = (960.0 / max_side).min(1.0);
        let width = ((image.width() as f64 * ratio / 32.0).round() as u32 * 32).max(32);
        let height = ((image.height() as f64 * ratio / 32.0).round() as u32 * 32).max(32);
        let scaled = resize(image, width, height, FilterType::Triangle);
        let mut input = vec![0_f32; (width * height * 3) as usize];
        let mean = [0.485_f32, 0.456, 0.406];
        let std = [0.229_f32, 0.224, 0.225];
        // Paddle/RapidOCR's inputs originate from cv2 BGR, including recognition.
        for (x, y, pixel) in scaled.enumerate_pixels() {
            for channel in 0..3 {
                input[channel * (width * height) as usize + (y * width + x) as usize] =
                    (pixel[2 - channel] as f32 / 255.0 - mean[channel]) / std[channel];
            }
        }
        let tensor = Tensor::from_array(([1, 3, height as usize, width as usize], input))?;
        let outputs = self.detection.run(ort::inputs![tensor])?;
        let (shape, probability) = outputs[0].try_extract_tensor::<f32>()?;
        ensure!(
            shape.len() == 4 && shape[0] == 1 && shape[1] == 1,
            "文字检测输出维度异常：{shape:?}"
        );
        let map_h = shape[2] as usize;
        let map_w = shape[3] as usize;
        let boxes = horizontal_boxes(probability, map_w, map_h, image.width(), image.height());
        drop(outputs);
        let mut lines = Vec::new();
        for bounds in boxes.into_iter().take(12) {
            let crop = crop_imm(
                image,
                bounds[0],
                bounds[1],
                bounds[2] - bounds[0],
                bounds[3] - bounds[1],
            )
            .to_image();
            let (text, confidence) = self.read_line(&crop)?;
            // Retain every raw line, including empty/low-confidence results, in debug logs.
            lines.push(Line {
                text,
                confidence,
                bounds,
            });
        }
        let text = lines
            .iter()
            .map(|line| line.text.as_str())
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        Ok(TextResult {
            text,
            lines,
            elapsed_ms: started.elapsed().as_millis(),
        })
    }

    /// Direct recognition for a known, bounded single text line. Settlement
    /// numbers already have foreground bounds; they need no detection pass.
    pub(crate) fn read_line(&mut self, image: &RgbaImage) -> Result<(String, f32)> {
        ensure!(image.width() > 0 && image.height() > 0, "OCR 裁图为空");
        let height = 48_u32;
        let resized_width = ((image.width() as f64 * height as f64 / image.height() as f64).ceil()
            as u32)
            .clamp(1, 2048);
        let width = resized_width.max(320);
        let scaled = resize(image, resized_width, height, FilterType::Triangle);
        let mut input = vec![0_f32; (width * height * 3) as usize];
        for (x, y, pixel) in scaled.enumerate_pixels() {
            for channel in 0..3 {
                input[channel * (width * height) as usize + (y * width + x) as usize] =
                    pixel[2 - channel] as f32 / 127.5 - 1.0;
            }
        }
        let tensor = Tensor::from_array(([1, 3, height as usize, width as usize], input))?;
        let outputs = self.recognition.run(ort::inputs![tensor])?;
        let (shape, probabilities) = outputs[0].try_extract_tensor::<f32>()?;
        ensure!(
            shape.len() == 3 && shape[0] == 1 && shape[2] as usize == self.characters.len(),
            "文字识别输出与字符表不一致：{shape:?} / {}",
            self.characters.len()
        );
        decode_ctc(probabilities, shape[2] as usize, &self.characters)
    }
}

// The game ROIs contain horizontal text. A bounded, axis-aligned DB component
// extraction avoids adding a document layout/perspective correction pipeline.
fn horizontal_boxes(
    map: &[f32],
    width: usize,
    height: usize,
    original_w: u32,
    original_h: u32,
) -> Vec<[u32; 4]> {
    let mut seen = vec![false; width * height];
    let mut boxes = Vec::new();
    for index in 0..seen.len() {
        if seen[index] || map[index] < 0.3 {
            continue;
        }
        let mut queue = VecDeque::from([index]);
        seen[index] = true;
        let (mut left, mut top, mut right, mut bottom) = (width, height, 0, 0);
        let mut pixels = 0;
        let mut score = 0_f32;
        while let Some(index) = queue.pop_front() {
            let (x, y) = (index % width, index / width);
            left = left.min(x);
            right = right.max(x + 1);
            top = top.min(y);
            bottom = bottom.max(y + 1);
            pixels += 1;
            score += map[index];
            for (nx, ny) in [
                (x.wrapping_sub(1), y),
                (x + 1, y),
                (x, y.wrapping_sub(1)),
                (x, y + 1),
            ] {
                if nx >= width || ny >= height {
                    continue;
                }
                let next = ny * width + nx;
                if !seen[next] && map[next] >= 0.3 {
                    seen[next] = true;
                    queue.push_back(next);
                }
            }
        }
        if pixels < 6 || bottom - top < 3 || score / (pixels as f32) < 0.5 {
            continue;
        }
        let expansion = ((right - left) * (bottom - top)) as f64 * 1.5
            / (2 * ((right - left) + (bottom - top))) as f64;
        let scale_x = original_w as f64 / width as f64;
        let scale_y = original_h as f64 / height as f64;
        let bounds = [
            ((left as f64 - expansion).max(0.0) * scale_x).floor() as u32,
            ((top as f64 - expansion).max(0.0) * scale_y).floor() as u32,
            (((right as f64 + expansion) * scale_x).ceil() as u32).min(original_w),
            (((bottom as f64 + expansion) * scale_y).ceil() as u32).min(original_h),
        ];
        if bounds[2] > bounds[0] && bounds[3] > bounds[1] {
            boxes.push(bounds);
        }
    }
    boxes.sort_by_key(|b| (b[1] / 12, b[0]));
    boxes
}

fn decode_ctc(
    probabilities: &[f32],
    classes: usize,
    characters: &[String],
) -> Result<(String, f32)> {
    ensure!(
        classes == characters.len() && classes > 1 && probabilities.len() % classes == 0,
        "CTC 输出维度无效"
    );
    let mut text = String::new();
    let mut previous = 0;
    let mut sum = 0_f32;
    let mut count = 0;
    for timestep in probabilities.chunks_exact(classes) {
        let (index, confidence) = timestep
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .unwrap();
        if index != 0 && index != previous {
            text.push_str(&characters[index]);
            sum += confidence;
            count += 1;
        }
        previous = index;
    }
    Ok((text, if count == 0 { 0.0 } else { sum / count as f32 }))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ctc_keeps_separated_repeats_and_exact_mixed_script() -> Result<()> {
        let chars = ["", "小", "p", "0", "o"].map(str::to_string);
        let mut probabilities = Vec::new();
        for label in [1, 1, 0, 1, 2, 0, 3, 4] {
            let mut step = vec![0.01; chars.len()];
            step[label] = 0.96;
            probabilities.extend(step);
        }
        let (text, confidence) = decode_ctc(&probabilities, chars.len(), &chars)?;
        assert_eq!(text, "小小p0o");
        assert!((confidence - 0.96).abs() < 0.001);
        assert!(decode_ctc(&[0.1], 5, &chars).is_err());
        Ok(())
    }
    #[test]
    fn detection_boxes_are_clamped_and_blank_maps_stay_empty() {
        assert!(horizontal_boxes(&vec![0.0; 32 * 32], 32, 32, 100, 40).is_empty());
        let mut map = vec![0.0; 32 * 32];
        for y in 0..8 {
            for x in 0..20 {
                map[y * 32 + x] = 0.9;
            }
        }
        let boxes = horizontal_boxes(&map, 32, 32, 100, 40);
        assert_eq!(boxes.len(), 1);
        assert_eq!(boxes[0][0], 0);
        assert_eq!(boxes[0][1], 0);
        assert!(boxes[0][2] <= 100 && boxes[0][3] <= 40);
    }
}
