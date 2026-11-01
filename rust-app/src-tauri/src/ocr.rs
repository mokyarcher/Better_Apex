use anyhow::{ensure, Result};
use image::RgbaImage;
use libloading::Library;
use serde::Serialize;
use std::{
    ffi::{c_char, c_int, c_void, CStr, CString},
    path::Path,
};
use windows::{
    core::HSTRING,
    Globalization::Language,
    Graphics::Imaging::{BitmapAlphaMode, BitmapPixelFormat, SoftwareBitmap},
    Media::Ocr::OcrEngine,
    Storage::Streams::DataWriter,
    Win32::System::WinRT::{RoInitialize, RoUninitialize, RO_INIT_MULTITHREADED},
};

struct Apartment;
impl Apartment {
    fn new() -> Result<Self> {
        unsafe {
            RoInitialize(RO_INIT_MULTITHREADED)?;
        }
        Ok(Self)
    }
}
impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe {
            RoUninitialize();
        }
    }
}

pub struct WindowsRecognizer {
    engine: OcrEngine,
    _apartment: Apartment,
}

impl WindowsRecognizer {
    pub fn new() -> Result<Self> {
        let apartment = Apartment::new()?;
        let language = Language::CreateLanguage(&HSTRING::from("zh-Hans"))?;
        ensure!(
            OcrEngine::IsLanguageSupported(&language)?,
            "缺少 Windows 简体中文 OCR 组件"
        );
        let engine = OcrEngine::TryCreateFromLanguage(&language)?;
        Ok(Self {
            engine,
            _apartment: apartment,
        })
    }

    pub fn read(&self, image: &RgbaImage) -> Result<String> {
        ensure!(
            image.width().max(image.height()) <= OcrEngine::MaxImageDimension()?,
            "OCR 区域过大"
        );
        let mut bgra = image.as_raw().clone();
        for pixel in bgra.chunks_exact_mut(4) {
            pixel.swap(0, 2);
        }
        let writer = DataWriter::new()?;
        writer.WriteBytes(&bgra)?;
        let bitmap = SoftwareBitmap::CreateCopyWithAlphaFromBuffer(
            &writer.DetachBuffer()?,
            BitmapPixelFormat::Bgra8,
            image.width() as i32,
            image.height() as i32,
            BitmapAlphaMode::Ignore,
        )?;
        let result = self.engine.RecognizeAsync(&bitmap)?.get()?;
        let text = result.Text()?.to_string();
        bitmap.Close()?;
        writer.Close()?;
        Ok(text)
    }
}

pub fn windows_text(image: &RgbaImage) -> Result<String> {
    WindowsRecognizer::new()?.read(image)
}

// Stage A calls the same DLL C ABI directly. Ownership is explicit; libraries
// outlive all API calls, the API handle and returned UTF-8 strings are freed.
pub struct Tesseract {
    tess: Library,
    _lept: Library,
    handle: *mut c_void,
}
impl Tesseract {
    pub fn new(root: &Path, languages: &str) -> Result<Self> {
        unsafe {
            let lept = Library::new(root.join("x64/leptonica-1.82.0.dll"))?;
            let tess = Library::new(root.join("x64/tesseract50.dll"))?;
            let create =
                tess.get::<unsafe extern "C" fn() -> *mut c_void>(b"TessBaseAPICreate\0")?;
            let handle = create();
            ensure!(!handle.is_null(), "无法创建 Tesseract");
            let api = Self {
                tess,
                _lept: lept,
                handle,
            };
            let init = api.tess.get::<unsafe extern "C" fn(
                *mut c_void,
                *const c_char,
                *const c_char,
                c_int,
            ) -> c_int>(b"TessBaseAPIInit2\0")?;
            let data = CString::new(root.join("tessdata").to_string_lossy().as_bytes())?;
            let language = CString::new(languages)?;
            ensure!(
                init(api.handle, data.as_ptr(), language.as_ptr(), 1) == 0,
                "Tesseract 模型初始化失败"
            );
            Ok(api)
        }
    }
    pub fn read(
        &mut self,
        image: &RgbaImage,
        mode: i32,
        whitelist: Option<&str>,
    ) -> Result<(String, f32)> {
        unsafe {
            let variable = self.tess.get::<unsafe extern "C" fn(
                *mut c_void,
                *const c_char,
                *const c_char,
            ) -> c_int>(b"TessBaseAPISetVariable\0")?;
            let value = CString::new(whitelist.unwrap_or(""))?;
            ensure!(
                variable(
                    self.handle,
                    c"tessedit_char_whitelist".as_ptr(),
                    value.as_ptr()
                ) != 0,
                "设置 OCR 字符集失败"
            );
            let set_mode = self
                .tess
                .get::<unsafe extern "C" fn(*mut c_void, c_int)>(b"TessBaseAPISetPageSegMode\0")?;
            let set_image = self.tess.get::<unsafe extern "C" fn(
                *mut c_void,
                *const u8,
                c_int,
                c_int,
                c_int,
                c_int,
            )>(b"TessBaseAPISetImage\0")?;
            set_mode(self.handle, mode);
            set_image(
                self.handle,
                image.as_raw().as_ptr(),
                image.width() as i32,
                image.height() as i32,
                4,
                (image.width() * 4) as i32,
            );
            let get = self
                .tess
                .get::<unsafe extern "C" fn(*mut c_void) -> *mut c_char>(
                    b"TessBaseAPIGetUTF8Text\0",
                )?;
            let delete = self
                .tess
                .get::<unsafe extern "C" fn(*mut c_char)>(b"TessDeleteText\0")?;
            let text = get(self.handle);
            ensure!(!text.is_null(), "Tesseract 未返回文字");
            let value = CStr::from_ptr(text).to_string_lossy().trim().to_string();
            let confidence = self.confidence();
            delete(text);
            Ok((value, confidence?))
        }
    }
    pub fn confidence(&self) -> Result<f32> {
        unsafe {
            let get = self
                .tess
                .get::<unsafe extern "C" fn(*mut c_void) -> c_int>(b"TessBaseAPIMeanTextConf\0")?;
            Ok(get(self.handle) as f32 / 100.0)
        }
    }
}
impl Drop for Tesseract {
    fn drop(&mut self) {
        unsafe {
            if let Ok(delete) = self
                .tess
                .get::<unsafe extern "C" fn(*mut c_void)>(b"TessBaseAPIDelete\0")
            {
                delete(self.handle);
            }
        }
    }
}

#[derive(Serialize)]
pub struct OcrProbe {
    pub nickname_backend: crate::ocr_settings::OcrBackend,
    pub width: u32,
    pub height: u32,
    pub windows_text: Vec<String>,
    pub tesseract_text: Vec<String>,
    pub scope: String,
    pub settlement: Option<crate::settlement::SettlementResult>,
    pub selection: Option<Vec<crate::selection::SelectionNameEvidence>>,
}
pub fn roster_names(image: &RgbaImage) -> Result<Vec<String>> {
    roster_names_with(image, &WindowsRecognizer::new()?)
}

pub fn roster_names_with(image: &RgbaImage, recognizer: &WindowsRecognizer) -> Result<Vec<String>> {
    roster_names_traced(image, recognizer, None)
}

pub fn roster_names_traced(
    image: &RgbaImage,
    recognizer: &WindowsRecognizer,
    trace: Option<&crate::nickname_debug::TraceContext>,
) -> Result<Vec<String>> {
    roster_names_read(image, |crop, slot| {
        crate::nickname_debug::record_ocr(
            trace,
            slot,
            "hud_lower_left",
            "windows",
            recognizer.read(crop),
        )
    })
}

pub fn roster_names_selected(
    image: &RgbaImage,
    recognizer: &mut crate::nickname_ocr::NicknameRecognizer,
    trace: Option<&crate::nickname_debug::TraceContext>,
) -> Result<Vec<String>> {
    roster_names_read(image, |crop, slot| {
        recognizer.read(crop, slot, "hud_lower_left", trace)
    })
}

fn roster_names_read(
    image: &RgbaImage,
    mut read: impl FnMut(&RgbaImage, usize) -> Result<String>,
) -> Result<Vec<String>> {
    ensure!(
        (image.width() as f64 / image.height() as f64 - 16.0 / 9.0).abs() < 0.025,
        "请使用未裁剪的 16:9 对局截图，完整保留左下角队伍栏。"
    );
    let names = [
        (0.070, 0.715, 0.090, 0.045),
        (0.070, 0.775, 0.090, 0.045),
        (0.085, 0.880, 0.130, 0.050),
    ]
    .into_iter()
    .enumerate()
    .map(|(slot, (x, y, w, h))| {
        read(&crate::imaging::region(image, x, y, w, h, 3.0), slot)
            .map(|text| text.trim().to_string())
    })
    .collect::<Result<Vec<_>>>()?;
    ensure!(
        names.iter().any(|name| !name.is_empty()),
        "未读取到队伍昵称，请检查是否完整显示了对局左下角队伍栏。"
    );
    Ok(names)
}
pub fn probe(path: &Path, root: &Path, kind: &str) -> Result<OcrProbe> {
    probe_traced(path, root, kind, None)
}

pub fn probe_traced(
    path: &Path,
    root: &Path,
    kind: &str,
    trace: Option<&crate::nickname_debug::TraceContext>,
) -> Result<OcrProbe> {
    probe_selected(
        path,
        root,
        kind,
        crate::ocr_settings::OcrBackend::WindowsTesseract,
        trace,
    )
}

pub fn probe_selected(
    path: &Path,
    root: &Path,
    kind: &str,
    backend: crate::ocr_settings::OcrBackend,
    trace: Option<&crate::nickname_debug::TraceContext>,
) -> Result<OcrProbe> {
    let image = crate::imaging::load(path)?;
    ensure!(
        (image.width() as f64 / image.height() as f64 - 16.0 / 9.0).abs() < 0.025,
        "请选择完整 16:9 原图"
    );
    let windows;
    let mut tess = Vec::new();
    let mut settlement = None;
    let mut selection = None;
    let mut reader = crate::nickname_ocr::NicknameRecognizer::new(root, backend)?;
    if kind == "roster" {
        windows = roster_names_selected(&image, &mut reader, trace)?;
    } else if kind == "selection" {
        let evidence =
            crate::selection::SelectionRecognizer { reader }.recognize_traced(&image, trace)?;
        windows = evidence
            .iter()
            .flat_map(|entry| [entry.upper_left.clone(), entry.lower_bar.clone()])
            .collect();
        selection = Some(evidence);
    } else {
        let parsed = crate::settlement::recognize_image_selected(&image, root, &mut reader, trace)?;
        windows = parsed
            .players
            .iter()
            .map(|p| p.nickname_hint.clone().unwrap_or_default())
            .collect();
        tess = parsed
            .players
            .iter()
            .map(|p| p.damage.raw_text.clone())
            .collect();
        settlement = Some(parsed);
    }
    Ok(OcrProbe {
        nickname_backend: backend,
        width: image.width(),
        height: image.height(),
        windows_text: windows,
        tesseract_text: tess,
        scope: match kind {
            "roster" => "局内队伍昵称 OCR 候选，后续关联仍需人工核对",
            "selection" => {
                "传奇选择界面按位置合并两处区域的昵称候选；使用设置中所选引擎，不判断本人或跨画面匹配"
            }
            _ => "三人结算字段解析结果；昵称与本人归属仍需人工核对",
        }
        .into(),
        settlement,
        selection,
    })
}
