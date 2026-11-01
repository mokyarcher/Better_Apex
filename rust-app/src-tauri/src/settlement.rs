use anyhow::{ensure, Result};
use image::{
    imageops::{crop_imm, resize, FilterType},
    RgbaImage,
};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Clone, Deserialize, Serialize)]
pub struct NumberField {
    pub raw_text: String,
    pub value: Option<i64>,
    pub confidence: f32,
}

#[derive(Clone, Deserialize, Serialize)]
pub struct SettlementPlayer {
    pub position: String,
    /// Untrusted text for aligning summary cards only. Never a display/save name.
    #[serde(default, alias = "nickname")]
    pub nickname_hint: Option<String>,
    pub name_confirmed: bool,
    pub kills: NumberField,
    pub assists: NumberField,
    pub knocks: NumberField,
    pub damage: NumberField,
}

#[derive(Clone, Deserialize, Serialize)]
pub struct SettlementResult {
    pub rank: NumberField,
    pub squad_kills: NumberField,
    pub players: Vec<SettlementPlayer>,
    pub warnings: Vec<String>,
}

#[derive(Clone, Copy)]
struct Glyph {
    left: u32,
    top: u32,
    right: u32,
    bottom: u32,
}

impl Glyph {
    fn width(self) -> u32 {
        self.right - self.left
    }
    fn height(self) -> u32 {
        self.bottom - self.top
    }
}

pub fn recognize(path: &Path, root: &Path) -> Result<SettlementResult> {
    recognize_traced(path, root, None)
}

pub fn recognize_traced(
    path: &Path,
    root: &Path,
    trace: Option<&crate::nickname_debug::TraceContext>,
) -> Result<SettlementResult> {
    let image = crate::imaging::load(path)?;
    ensure!(
        (image.width() as f64 / image.height() as f64 - 16.0 / 9.0).abs() < 0.025,
        "请选择完整 16:9 原图"
    );
    recognize_image_traced(&image, root, trace)
}

pub fn recognize_image(image: &RgbaImage, root: &Path) -> Result<SettlementResult> {
    recognize_image_traced(image, root, None)
}

pub fn recognize_image_traced(
    image: &RgbaImage,
    root: &Path,
    trace: Option<&crate::nickname_debug::TraceContext>,
) -> Result<SettlementResult> {
    let mut reader = crate::nickname_ocr::NicknameRecognizer::new(
        root,
        crate::ocr_settings::OcrBackend::WindowsTesseract,
    )?;
    recognize_image_selected(image, root, &mut reader, trace)
}

pub fn recognize_image_selected(
    image: &RgbaImage,
    root: &Path,
    reader: &mut crate::nickname_ocr::NicknameRecognizer,
    trace: Option<&crate::nickname_debug::TraceContext>,
) -> Result<SettlementResult> {
    let mut result = recognize_image_numbers_selected(image, root, reader, trace)?;
    fill_nickname_hints(image, reader, &mut result, trace);
    Ok(result)
}

/// Automatic verification needs numbers from both frames, but card text only
/// once after corroboration. Do not spend detection/recognition on hints here.
pub fn recognize_image_numbers_selected(
    image: &RgbaImage,
    root: &Path,
    reader: &mut crate::nickname_ocr::NicknameRecognizer,
    trace: Option<&crate::nickname_debug::TraceContext>,
) -> Result<SettlementResult> {
    ensure!(
        (image.width() as f64 / image.height() as f64 - 16.0 / 9.0).abs() < 0.025,
        "请选择完整 16:9 原图"
    );
    // Keep a single explicitly selected backend for this whole settlement.
    // Reuse the worker's PP-OCR instance, never load another model per frame.
    let mut engine = if matches!(reader, crate::nickname_ocr::NicknameRecognizer::Paddle(_)) {
        None
    } else {
        Some(crate::ocr::Tesseract::new(root, "eng")?)
    };
    let rank = if let crate::nickname_ocr::NicknameRecognizer::Paddle(pp) = reader {
        read_paddle_header(pp, image, true, trace)?
    } else {
        read_summary_header(engine.as_mut().unwrap(), image, true)?
    };
    let squad_kills = if let crate::nickname_ocr::NicknameRecognizer::Paddle(pp) = reader {
        read_paddle_header(pp, image, false, trace)?
    } else {
        read_summary_header(engine.as_mut().unwrap(), image, false)?
    };

    let mut players = Vec::with_capacity(3);
    let mut damage_reader = None;
    for (slot, (position, x)) in [
        ("左侧玩家", 0.118),
        ("中间玩家", 0.394),
        ("右侧玩家", 0.670),
    ]
    .into_iter()
    .enumerate()
    {
        let kda_row = crate::imaging::region(image, x, 0.375, 0.066, 0.038, 1.0);
        let damage_crop = crate::imaging::region(image, x, 0.440, 0.050, 0.038, 1.0);
        let (kda_fields, damage) =
            if let crate::nickname_ocr::NicknameRecognizer::Paddle(pp) = reader {
                let kda_fields = read_paddle_kda(pp, &kda_row, slot, trace)?;
                let glyphs = foreground_glyphs(&number_foreground(&damage_crop, false));
                (
                    kda_fields,
                    read_paddle_number(
                        pp,
                        &damage_crop,
                        &glyphs,
                        i64::MAX,
                        slot,
                        "settlement_damage",
                        trace,
                    )?,
                )
            } else {
                let engine = engine.as_mut().unwrap();
                let kda_line = engine.read(&kda_row, 7, Some("0123456789/,"))?;
                let (_, fields) = read_kda(engine, &kda_row, &kda_line)?;
                (
                    fields,
                    read_damage(engine, &damage_crop, &mut damage_reader)?,
                )
            };
        if let Some(trace) = trace {
            trace.event(
                "damage_result",
                serde_json::json!({"slot":slot,"result":damage}),
            );
        }

        players.push(SettlementPlayer {
            position: position.to_string(),
            nickname_hint: None,
            name_confirmed: false,
            kills: kda_fields[0].clone(),
            assists: kda_fields[1].clone(),
            knocks: kda_fields[2].clone(),
            damage,
        });
    }

    let mut warnings = Vec::new();
    warnings.push(
        "结算文字仅作卡片匹配线索；最终昵称来自局内单次识别名单或手动核对，无法关联时保留归属待核对。"
            .to_string(),
    );
    let fields = players
        .iter()
        .flat_map(|p| [&p.kills, &p.assists, &p.knocks, &p.damage])
        .chain([&rank, &squad_kills])
        .collect::<Vec<_>>();
    if fields.iter().any(|f| f.value.is_none()) {
        warnings.push("部分数字未能完整识别，已标记为待核对，没有补成 0。".to_string());
    }
    if fields
        .iter()
        .any(|f| f.value.is_some() && f.confidence < 0.70)
    {
        warnings.push("部分数字的识别可信度较低，请对照原图检查。".to_string());
    }
    if let Some(total) = squad_kills.value {
        if players.iter().all(|p| p.kills.value.is_some())
            && players.iter().map(|p| p.kills.value.unwrap()).sum::<i64>() != total
        {
            warnings.push("三人击杀之和与小队总击杀不一致，请核对。".to_string());
        }
    }
    ensure!(
        fields.iter().any(|f| f.value.is_some()),
        "未识别到此模板的有效战绩数字，请检查是否为完整的三人结算页"
    );
    Ok(SettlementResult {
        rank,
        squad_kills,
        players,
        warnings,
    })
}

pub fn fill_nickname_hints(
    image: &RgbaImage,
    reader: &mut crate::nickname_ocr::NicknameRecognizer,
    result: &mut SettlementResult,
    trace: Option<&crate::nickname_debug::TraceContext>,
) {
    for (slot, (player, x)) in result
        .players
        .iter_mut()
        .zip([0.118, 0.394, 0.670])
        .enumerate()
    {
        match reader.read(
            &crate::imaging::region(image, x - 0.012, 0.255, 0.160, 0.070, 3.0),
            slot,
            "settlement_card",
            trace,
        ) {
            Ok(text) => {
                player.nickname_hint = (!text.trim().is_empty()).then(|| text.trim().to_owned())
            }
            Err(error) => {
                player.nickname_hint = None;
                result.warnings.push(format!(
                    "{}匹配线索读取失败，保留数字与未知归属：{error:#}",
                    player.position
                ));
            }
        }
    }
}

fn parse_number(raw: &str, confidence: f32, maximum: i64) -> NumberField {
    let compact = raw
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .replace('，', ",");
    let groups = compact.split(',').collect::<Vec<_>>();
    let valid = if groups.len() == 1 {
        !groups[0].is_empty() && groups[0].chars().all(|c| c.is_ascii_digit())
    } else {
        (1..=3).contains(&groups[0].len())
            && groups[0].chars().all(|c| c.is_ascii_digit())
            && groups[1..]
                .iter()
                .all(|g| g.len() == 3 && g.chars().all(|c| c.is_ascii_digit()))
    };
    let value = if valid {
        compact
            .replace(',', "")
            .parse::<i64>()
            .ok()
            .filter(|n| *n <= maximum)
    } else {
        None
    };
    NumberField {
        raw_text: raw.trim().to_string(),
        value,
        confidence,
    }
}

fn parse_kda(raw: &str, confidence: f32) -> [NumberField; 3] {
    let normalized = raw
        .replace('／', "/")
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>();
    let values = normalized.split('/').collect::<Vec<_>>();
    if values.len() != 3 {
        return std::array::from_fn(|_| NumberField {
            raw_text: raw.to_string(),
            value: None,
            confidence,
        });
    }
    std::array::from_fn(|i| parse_number(values[i], confidence, 999))
}

fn read_kda(
    engine: &mut crate::ocr::Tesseract,
    row: &RgbaImage,
    raw: &(String, f32),
) -> Result<(String, [NumberField; 3])> {
    if let Some(groups) = split_kda(row) {
        let mut readings = Vec::with_capacity(3);
        let mut fields = Vec::with_capacity(3);
        for group in groups {
            let full = prepare_number(&crop_glyphs(row, &group));
            let first = engine.read(&full, 8, Some("0123456789"))?;
            let mut reading = first.clone();
            let mut field = parse_number(&first.0, first.1, 999);
            if first.0.chars().filter(|c| c.is_ascii_digit()).count() != group.len()
                || field.value.is_none()
            {
                let digit_reads = group
                    .iter()
                    .map(|glyph| {
                        let crop = prepare_number(&crop_glyphs(row, &[*glyph]));
                        engine.read(&crop, 10, Some("0123456789"))
                    })
                    .collect::<Result<Vec<_>>>()?;
                if digit_reads
                    .iter()
                    .all(|r| r.0.chars().filter(|c| c.is_ascii_digit()).count() == 1)
                {
                    let text = digit_reads.iter().map(|r| r.0.trim()).collect::<String>();
                    let confidence = digit_reads.iter().map(|r| r.1).fold(1.0, f32::min);
                    field = parse_number(&text, confidence, 999);
                    reading = (text, confidence);
                } else {
                    field.value = None;
                }
            }
            readings.push(reading.0);
            fields.push(field);
        }
        let parsed: [NumberField; 3] = fields.try_into().ok().expect("three KDA groups");
        Ok((readings.join("/"), parsed))
    } else {
        Ok((raw.0.clone(), parse_kda(&raw.0, raw.1)))
    }
}

// Isolate the actual ink before choosing OCR bounds. Yellow headers must reject
// the red banner; damage uses neutral white, excluding portrait/background art.
fn number_foreground(crop: &RgbaImage, yellow: bool) -> RgbaImage {
    let mut mask = crop.clone();
    for p in mask.pixels_mut() {
        let ink = if yellow {
            p[0] >= 130 && p[1] as u16 * 100 >= p[0] as u16 * 55 && p[1].saturating_sub(p[2]) >= 50
        } else {
            let low = p[0].min(p[1]).min(p[2]);
            let high = p[0].max(p[1]).max(p[2]);
            low >= 140 && high - low <= 60
        };
        let v = if ink { 255 } else { 0 };
        *p = image::Rgba([v, v, v, 255]);
    }
    mask
}

fn foreground_glyphs(mask: &RgbaImage) -> Vec<Glyph> {
    let mut glyphs = Vec::new();
    let mut x = 0;
    let has_ink = |x| (0..mask.height()).any(|y| mask.get_pixel(x, y)[0] > 0);
    while x < mask.width() {
        if !has_ink(x) {
            x += 1;
            continue;
        }
        let left = x;
        while x + 1 < mask.width() && has_ink(x + 1) {
            x += 1;
        }
        let right = x + 1;
        let mut top = mask.height();
        let mut bottom = 0;
        for xx in left..right {
            for y in 0..mask.height() {
                if mask.get_pixel(xx, y)[0] > 0 {
                    top = top.min(y);
                    bottom = bottom.max(y + 1);
                }
            }
        }
        // Ignore commas and speckles, but keep narrow full-height digits (1).
        if bottom - top >= 4 && (bottom - top) as f64 >= mask.height() as f64 * 0.4 {
            glyphs.push(Glyph {
                left,
                top,
                right,
                bottom,
            });
        }
        x += 1;
    }
    glyphs
}

fn summary_number_region(image: &RgbaImage, rank: bool) -> (RgbaImage, RgbaImage, Vec<Glyph>) {
    let row = if rank {
        crate::imaging::region(image, 0.740, 0.140, 0.092, 0.065, 1.0)
    } else {
        crate::imaging::region(image, 0.890, 0.145, 0.048, 0.060, 1.0)
    };
    let foreground = number_foreground(&row, true);
    let mut glyphs = foreground_glyphs(&foreground);
    if rank {
        // Locate the entire 第 prefix, then take the following complete digits.
        // A fixed fallback rectangle could clip a shifted single-digit rank.
        let prefix = glyphs.first().copied();
        if glyphs.len() >= 2
            && glyphs.len() <= 3
            && prefix.is_some_and(|p| {
                p.width() as f64 >= p.height() as f64 * 0.7
                    && p.height() as f64
                        > glyphs[1..].iter().map(|g| g.height()).max().unwrap_or(0) as f64 * 1.05
            })
        {
            glyphs.remove(0);
        } else {
            glyphs.clear();
        }
    }
    (row, foreground, glyphs)
}

fn read_paddle_number(
    engine: &mut crate::ppocr::PpOcr,
    row: &RgbaImage,
    glyphs: &[Glyph],
    maximum: i64,
    slot: usize,
    source: &str,
    trace: Option<&crate::nickname_debug::TraceContext>,
) -> Result<NumberField> {
    if glyphs.is_empty() || glyphs.len() > 6 {
        return Ok(parse_number("", 0.0, maximum));
    }
    let started = std::time::Instant::now();
    let (text, confidence) = crate::nickname_debug::record_ocr(
        trace,
        slot,
        source,
        "ppocr_v6_small_number",
        engine.read_line(&crop_glyphs(row, glyphs)),
    )?;
    let mut result = parse_number(&text, confidence, maximum);
    let digits = text
        .chars()
        .filter(|c| c.is_ascii_digit())
        .collect::<String>();
    if confidence < 0.85
        || digits.len() != glyphs.len()
        || (digits.len() > 1 && digits.starts_with('0'))
    {
        result.value = None;
    }
    if let Some(trace) = trace {
        trace.event("settlement_number",serde_json::json!({"slot":slot,"source":source,"engine":"ppocr_v6_small","result":result,"elapsed_ms":started.elapsed().as_millis()}));
    }
    Ok(result)
}

fn read_paddle_header(
    engine: &mut crate::ppocr::PpOcr,
    image: &RgbaImage,
    rank: bool,
    trace: Option<&crate::nickname_debug::TraceContext>,
) -> Result<NumberField> {
    let (row, _, glyphs) = summary_number_region(image, rank);
    let mut result = read_paddle_number(
        engine,
        &row,
        &glyphs,
        if rank { 60 } else { 999 },
        0,
        if rank {
            "settlement_rank"
        } else {
            "settlement_squad_kills"
        },
        trace,
    )?;
    if rank && result.value == Some(0) {
        result.value = None;
    }
    Ok(result)
}

fn read_paddle_kda(
    engine: &mut crate::ppocr::PpOcr,
    row: &RgbaImage,
    slot: usize,
    trace: Option<&crate::nickname_debug::TraceContext>,
) -> Result<[NumberField; 3]> {
    let groups = split_kda(row);
    let (text, confidence) = crate::nickname_debug::record_ocr(
        trace,
        slot,
        "settlement_kda",
        "ppocr_v6_small_number",
        engine.read_line(row),
    )?;
    let mut fields = parse_kda(&text, confidence);
    if let Some(groups) = groups {
        for (field, glyphs) in fields.iter_mut().zip(&groups) {
            let digits = field
                .raw_text
                .chars()
                .filter(|c| c.is_ascii_digit())
                .collect::<String>();
            if confidence < 0.85
                || digits.len() != glyphs.len()
                || (digits.len() > 1 && digits.starts_with('0'))
            {
                field.value = None;
            }
            if field.value.is_none() {
                *field = read_paddle_number(
                    engine,
                    row,
                    glyphs,
                    999,
                    slot,
                    "settlement_kda_retry",
                    trace,
                )?;
            }
        }
    } else {
        for field in &mut fields {
            field.value = None;
        }
    }
    Ok(fields)
}

fn read_summary_header(
    engine: &mut crate::ocr::Tesseract,
    image: &RgbaImage,
    rank: bool,
) -> Result<NumberField> {
    let (row, foreground, glyphs) = summary_number_region(image, rank);
    let mut result =
        read_bounded_number(engine, &foreground, &glyphs, if rank { 60 } else { 999 })?;
    // Keep grayscale antialiasing when a hard foreground mask loses a thin
    // stroke. The bounds/count still come from the isolated foreground.
    if result.value.is_none() && result.confidence < 0.70 && !glyphs.is_empty() {
        result = read_bounded_number(engine, &row, &glyphs, if rank { 60 } else { 999 })?;
    }
    if result.value.is_none() && result.confidence < 0.70 && !glyphs.is_empty() {
        let mut text = String::new();
        let mut confidence = 1.0f32;
        for glyph in &glyphs {
            // Single-character OCR needs its actual ink box, without extra
            // source background margin (prepare_number adds white padding).
            let crop = crop_imm(
                &foreground,
                glyph.left,
                glyph.top,
                glyph.width(),
                glyph.height(),
            )
            .to_image();
            let reading = engine.read(&prepare_number(&crop), 10, Some("0123456789"))?;
            if reading.0.len() != 1
                || !reading.0.chars().all(|c| c.is_ascii_digit())
                || reading.1 < 0.70
            {
                return Ok(result);
            }
            text.push_str(&reading.0);
            confidence = confidence.min(reading.1);
        }
        result = parse_number(&text, confidence, if rank { 60 } else { 999 });
        if text.len() > 1 && text.starts_with('0') {
            result.value = None;
        }
    }
    if rank && result.value == Some(0) {
        result.value = None;
    }
    Ok(result)
}

fn read_damage(
    engine: &mut crate::ocr::Tesseract,
    row: &RgbaImage,
    windows_reader: &mut Option<crate::ocr::WindowsRecognizer>,
) -> Result<NumberField> {
    let foreground = number_foreground(row, false);
    let glyphs = foreground_glyphs(&foreground);
    let first = read_bounded_number(engine, row, &glyphs, i64::MAX)?;
    if first.value.is_some() || glyphs.is_empty() || glyphs.len() > 6 {
        return Ok(first);
    }
    // Retry only failed damage fields, using the same complete digit bounds.
    // Removing the background gives OCR a different input, rather than simply
    // repeating the identical recognition and treating it as new evidence.
    let masked = read_bounded_number(engine, &foreground, &glyphs, i64::MAX)?;
    if masked.value.is_some() {
        return Ok(masked);
    }
    // Some digital-font numbers are read correctly with low Tesseract
    // confidence. Require independent Windows OCR agreement with BOTH crops,
    // complete digit count and no leading zero. Never override a deliberate
    // high-confidence conflict. This does not change the nickname backend.
    if first.confidence < 0.70 && masked.confidence < 0.70 {
        if windows_reader.is_none() {
            *windows_reader = crate::ocr::WindowsRecognizer::new().ok();
        }
        if let Some(reader) = windows_reader.as_ref() {
            if let Ok(raw) = reader.read(&prepare_number(&crop_glyphs(row, &glyphs))) {
                let verified =
                    parse_number(&raw, first.confidence.min(masked.confidence), i64::MAX);
                let digits = raw
                    .chars()
                    .filter(|c| !c.is_whitespace())
                    .collect::<String>();
                if verified.value.is_some()
                    && digits.len() == glyphs.len()
                    && !(digits.len() > 1 && digits.starts_with('0'))
                    && verified.value == parse_number(&first.raw_text, 0.0, i64::MAX).value
                    && verified.value == parse_number(&masked.raw_text, 0.0, i64::MAX).value
                {
                    // Keep real confidence instead of inventing a higher score.
                    return Ok(verified);
                }
            }
        }
    }
    Ok(first)
}

fn read_bounded_number(
    engine: &mut crate::ocr::Tesseract,
    row: &RgbaImage,
    glyphs: &[Glyph],
    maximum: i64,
) -> Result<NumberField> {
    if glyphs.is_empty() || glyphs.len() > 6 {
        return Ok(parse_number("", 0.0, maximum));
    }
    let prepared = prepare_number(&crop_glyphs(row, glyphs));
    let validate = |reading: (String, f32)| {
        let mut result = parse_number(&reading.0, reading.1, maximum);
        let digits = reading
            .0
            .chars()
            .filter(|c| c.is_ascii_digit())
            .collect::<String>();
        if digits.len() != glyphs.len() || (digits.len() > 1 && digits.starts_with('0')) {
            result.value = None;
        }
        result
    };
    let line = validate(engine.read(&prepared, 7, Some("0123456789,"))?);
    if line.value.is_some() && line.confidence >= 0.90 {
        return Ok(line);
    }
    // Word (8) and raw-line (13) agreed on wrong readings of the old loose
    // crops. Use tight bounds, actual digit count, and line vs word instead.
    let word = validate(engine.read(&prepared, 8, Some("0123456789,"))?);
    if line.value.is_some()
        && word.value.is_some()
        && line.value != word.value
        && line.confidence >= 0.70
        && word.confidence >= 0.70
    {
        return Ok(NumberField {
            value: None,
            ..line
        });
    }
    let mut best =
        if word.value.is_some() && (line.value.is_none() || word.confidence > line.confidence) {
            word
        } else {
            line
        };
    if best.confidence < 0.70 {
        best.value = None;
    }
    Ok(best)
}

fn orange_glyphs(row: &RgbaImage) -> Vec<Glyph> {
    let width = row.width();
    let height = row.height();
    let mask = row
        .pixels()
        .map(|p| p[0] >= 130 && p[0].saturating_sub(p[1]) >= 25 && p[1].saturating_sub(p[2]) >= 20)
        .collect::<Vec<_>>();
    let has_ink = |x: u32| (0..height).any(|y| mask[(y * width + x) as usize]);
    let mut glyphs = Vec::new();
    let mut x = 0;
    while x < width {
        if !has_ink(x) {
            x += 1;
            continue;
        }
        let left = x;
        while x + 1 < width && has_ink(x + 1) {
            x += 1;
        }
        let right = x + 1;
        let mut top = height;
        let mut bottom = 0;
        for xx in left..right {
            for yy in 0..height {
                if mask[(yy * width + xx) as usize] {
                    top = top.min(yy);
                    bottom = bottom.max(yy + 1);
                }
            }
        }
        if bottom - top >= 4 && (bottom - top) as f64 >= height as f64 * 0.4 {
            glyphs.push(Glyph {
                left,
                top,
                right,
                bottom,
            });
        }
        x += 1;
    }
    glyphs
}

fn is_slash(row: &RgbaImage, glyph: Glyph) -> bool {
    let width = glyph.width() as f64;
    let height = glyph.height() as f64;
    if width < height * 0.2 || width > height * 0.65 {
        return false;
    }
    let band = (glyph.height() / 4).max(1);
    let mut upper = Vec::new();
    let mut lower = Vec::new();
    for x in glyph.left..glyph.right {
        for y in glyph.top..glyph.bottom {
            let p = row.get_pixel(x, y);
            if p[0] < 130 || p[0].saturating_sub(p[1]) < 25 || p[1].saturating_sub(p[2]) < 20 {
                continue;
            }
            if y < glyph.top + band {
                upper.push(x);
            }
            if y >= glyph.bottom - band {
                lower.push(x);
            }
        }
    }
    if upper.is_empty() || lower.is_empty() {
        return false;
    }
    let mean = |xs: &[u32]| xs.iter().map(|x| *x as f64).sum::<f64>() / xs.len() as f64;
    let range = upper
        .iter()
        .min()
        .zip(upper.iter().max())
        .map(|(a, b)| b - a + 1)
        .unwrap_or(0);
    mean(&upper) - mean(&lower) >= width * 0.35 && range as f64 <= width * 0.7
}

fn split_kda(row: &RgbaImage) -> Option<[Vec<Glyph>; 3]> {
    let glyphs = orange_glyphs(row);
    let slashes = glyphs
        .iter()
        .copied()
        .filter(|g| is_slash(row, *g))
        .collect::<Vec<_>>();
    if slashes.len() != 2 {
        return None;
    }
    let mut groups = [Vec::new(), Vec::new(), Vec::new()];
    for glyph in glyphs {
        if glyph.right <= slashes[0].left {
            groups[0].push(glyph);
        } else if glyph.left >= slashes[0].right && glyph.right <= slashes[1].left {
            groups[1].push(glyph);
        } else if glyph.left >= slashes[1].right {
            groups[2].push(glyph);
        }
    }
    if groups.iter().any(|g| !(1..=3).contains(&g.len())) {
        return None;
    }
    Some(groups)
}

fn crop_glyphs(row: &RgbaImage, glyphs: &[Glyph]) -> RgbaImage {
    let left = glyphs.iter().map(|g| g.left).min().unwrap();
    let top = glyphs.iter().map(|g| g.top).min().unwrap();
    let right = glyphs.iter().map(|g| g.right).max().unwrap();
    let bottom = glyphs.iter().map(|g| g.bottom).max().unwrap();
    let x = left.saturating_sub(2);
    let y = top.saturating_sub(2);
    let r = (right + 2).min(row.width());
    let b = (bottom + 2).min(row.height());
    crop_imm(row, x, y, r - x, b - y).to_image()
}

fn prepare_number(crop: &RgbaImage) -> RgbaImage {
    let mut gray = crop.clone();
    for p in gray.pixels_mut() {
        let value = 255 - p[0].max(p[1]).max(p[2]);
        *p = image::Rgba([value, value, value, 255]);
    }
    let enlarged = resize(
        &gray,
        gray.width() * 3,
        gray.height() * 3,
        FilterType::Triangle,
    );
    let mut padded = RgbaImage::from_pixel(
        enlarged.width() + 32,
        enlarged.height() + 32,
        image::Rgba([255, 255, 255, 255]),
    );
    image::imageops::replace(&mut padded, &enlarged, 16, 16);
    padded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paddle_settlement_reads_reported_numbers_without_tesseract_retries() -> Result<()> {
        let project = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let image = crate::imaging::load(&project.join("image_for_test/异常识别问题1.png"))?;
        let resources = crate::resources()?;
        let mut reader = crate::nickname_ocr::NicknameRecognizer::new(
            &resources,
            crate::ocr_settings::OcrBackend::PpOcrV6Small,
        )?;
        let directory = project.join(format!(
            ".local/storage-tests/pp-summary-{}",
            time::OffsetDateTime::now_utc().unix_timestamp_nanos()
        ));
        let debug = std::sync::Arc::new(crate::nickname_debug::NicknameDebug::new(&directory));
        let round = debug.round("pp_summary_numbers");
        let trace = debug.context(&round, "03_settlement_current", 1, 1);
        let started = std::time::Instant::now();
        let mut result =
            recognize_image_numbers_selected(&image, &resources, &mut reader, Some(&trace))?;
        let numbers_ms = started.elapsed().as_millis();
        assert!(result.players.iter().all(|p| p.nickname_hint.is_none()));
        let logfile = debug.status().current_file.unwrap();
        let before = std::fs::read_to_string(&logfile)?;
        assert!(!before.contains("settlement_card"));
        let started = std::time::Instant::now();
        fill_nickname_hints(&image, &mut reader, &mut result, Some(&trace));
        let hints_ms = started.elapsed().as_millis();
        let events: Vec<serde_json::Value> = std::fs::read_to_string(logfile)?
            .lines()
            .map(serde_json::from_str)
            .collect::<std::result::Result<_, _>>()?;
        let ocr: Vec<_> = events.iter().filter(|e| e["event"] == "ocr").collect();
        assert_eq!(
            ocr.iter()
                .filter(|e| e["data"]["detail"]["source"] == "settlement_card")
                .count(),
            3
        );
        assert!(ocr.iter().all(|e| e["data"]["detail"]["engine"]
            .as_str()
            .unwrap()
            .starts_with("ppocr_v6_small")));
        let report_directory = project.join(".local/ppocr-settlement-review");
        std::fs::create_dir_all(&report_directory)?;
        std::fs::write(
            report_directory.join("feedback.json"),
            serde_json::to_vec_pretty(
                &serde_json::json!({"numbers_ms":numbers_ms,"hints_ms":hints_ms,"result":result}),
            )?,
        )?;
        assert_eq!(result.rank.value, Some(12));
        assert_eq!(result.squad_kills.value, Some(1));
        for (player, expected) in
            result
                .players
                .iter()
                .zip([[0, 1, 0, 74], [1, 0, 2, 665], [0, 0, 0, 117]])
        {
            assert_eq!(
                [
                    player.kills.value,
                    player.assists.value,
                    player.knocks.value,
                    player.damage.value
                ],
                expected.map(Some)
            );
        }
        let crate::nickname_ocr::NicknameRecognizer::Paddle(pp) = &mut reader else {
            unreachable!()
        };
        let blank = RgbaImage::from_pixel(100, 50, image::Rgba([0, 0, 0, 255]));
        assert_eq!(
            read_paddle_number(pp, &blank, &[], 999, 0, "blank", None)?.value,
            None
        );
        drop(trace);
        drop(debug);
        std::fs::remove_dir_all(directory)?;
        Ok(())
    }

    #[test]
    fn reported_damage_failure_retries_without_filling_unknowns_with_zero() -> Result<()> {
        let project = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        // This is a desktop feedback screenshot, not a clean game frame. Only
        // the three unobstructed damage regions are used as OCR evidence.
        let image = crate::imaging::load(&project.join("image_for_test/异常识别问题1.png"))?;
        let mut engine = crate::ocr::Tesseract::new(&crate::resources()?, "eng")?;
        for (x, expected) in [(0.118, 74), (0.394, 665), (0.670, 117)] {
            let row = crate::imaging::region(&image, x, 0.440, 0.050, 0.038, 1.0);
            let result = read_damage(&mut engine, &row, &mut None)?;
            assert_eq!(
                result.value,
                Some(expected),
                "raw={}, confidence={}",
                result.raw_text,
                result.confidence
            );
        }
        let blank = RgbaImage::from_pixel(128, 55, image::Rgba([0, 0, 0, 255]));
        assert_eq!(read_damage(&mut engine, &blank, &mut None)?.value, None);
        assert_eq!(parse_number("-", 1.0, i64::MAX).value, None);
        assert_eq!(parse_number("0", 1.0, i64::MAX).value, Some(0));
        Ok(())
    }

    #[test]
    fn incomplete_header_does_not_guess_rank_from_a_fixed_rectangle() -> Result<()> {
        let project = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut image = crate::imaging::load(&project.join("image_for_test/1080p/结算图-02.png"))?;
        // Remove the 第 prefix, leaving the shifted 8 visible. Without the
        // expected structure the header must stay unknown, not read a fragment.
        for x in 1420..1536 {
            for y in 151..224 {
                image.put_pixel(x, y, image::Rgba([0, 0, 0, 255]));
            }
        }
        let mut engine = crate::ocr::Tesseract::new(&crate::resources()?, "eng")?;
        assert_eq!(read_summary_header(&mut engine, &image, true)?.value, None);
        assert_eq!(
            read_summary_header(&mut engine, &image, false)?.value,
            Some(4)
        );
        Ok(())
    }

    #[test]
    fn supplied_seven_summaries_keep_all_numbers_at_both_resolutions() -> Result<()> {
        let project = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let resources = crate::resources()?;
        // Manually transcribed from the screenshots, never inferred from OCR.
        let cases = [
            (5, 11, [[4, 2, 4, 1323], [7, 3, 10, 1971], [0, 2, 1, 371]]),
            (8, 4, [[1, 0, 1, 380], [3, 0, 3, 480], [0, 0, 0, 60]]),
            (7, 11, [[1, 6, 1, 554], [5, 3, 5, 1150], [5, 3, 5, 976]]),
            (2, 19, [[5, 3, 7, 1520], [10, 5, 10, 1938], [4, 1, 4, 1351]]),
            (12, 2, [[0, 0, 0, 174], [1, 0, 1, 158], [1, 0, 1, 321]]),
            (9, 5, [[1, 0, 1, 340], [3, 1, 4, 664], [1, 3, 1, 411]]),
            (14, 0, [[0, 0, 0, 77], [0, 0, 0, 221], [0, 0, 0, 83]]),
        ];
        let mut failures = Vec::new();
        let mut reports = Vec::new();
        for backend in [
            crate::ocr_settings::OcrBackend::PpOcrV6Small,
            crate::ocr_settings::OcrBackend::WindowsTesseract,
        ] {
            let mut reader = crate::nickname_ocr::NicknameRecognizer::new(&resources, backend)?;
            for resolution in ["1080p", "2K"] {
                for (index, (rank, kills, players)) in cases.iter().enumerate() {
                    let extension = if resolution == "2K" && index < 5 {
                        "jpg"
                    } else {
                        "png"
                    };
                    let name = format!("结算图-{:02}.{extension}", index + 1);
                    let path = project.join("image_for_test").join(resolution).join(&name);
                    let image = crate::imaging::load(&path)?;
                    let started = std::time::Instant::now();
                    let parsed = recognize_image_selected(&image, &resources, &mut reader, None)?;
                    let elapsed_ms = started.elapsed().as_millis();
                    let mut actual = vec![parsed.rank.value, parsed.squad_kills.value];
                    actual.extend(parsed.players.iter().flat_map(|p| {
                        [
                            p.kills.value,
                            p.assists.value,
                            p.knocks.value,
                            p.damage.value,
                        ]
                    }));
                    let expected = [*rank, *kills]
                        .into_iter()
                        .chain(players.iter().flatten().copied())
                        .map(Some)
                        .collect::<Vec<_>>();
                    let passed = actual == expected;
                    if !passed {
                        failures.push(format!(
                        "{backend:?}/{resolution}/{name}: expected {expected:?}, actual {actual:?}"
                    ));
                    }
                    reports.push(serde_json::json!({"backend":backend,"path":path,"passed":passed,"elapsed_ms":elapsed_ms,"expected":expected,"result":parsed}));
                }
            }
        }
        let directory = project.join(".local/ppocr-settlement-review");
        std::fs::create_dir_all(&directory)?;
        std::fs::write(
            directory.join("comparison.json"),
            serde_json::to_vec_pretty(&reports)?,
        )?;
        assert!(failures.is_empty(), "{}", failures.join("\n"));
        Ok(())
    }
}
