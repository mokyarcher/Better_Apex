use anyhow::{bail, ensure, Result};
use image::RgbaImage;
use serde::{Deserialize, Serialize};
use std::{
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use windows::{
    core::{Interface, BOOL},
    Win32::{
        Foundation::{HMODULE, HWND, LPARAM, POINT, RECT},
        Graphics::{
            Direct3D::{
                D3D_DRIVER_TYPE_UNKNOWN, D3D_FEATURE_LEVEL_10_0, D3D_FEATURE_LEVEL_10_1,
                D3D_FEATURE_LEVEL_11_0,
            },
            Direct3D11::*,
            Dxgi::{Common::*, *},
            Gdi::*,
        },
        System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_BELOW_NORMAL},
        UI::WindowsAndMessaging::*,
    },
};

#[derive(Clone, Serialize, Deserialize)]
pub struct Target {
    pub handle: isize,
    pub pid: u32,
    pub title: String,
}
impl Target {
    fn hwnd(&self) -> HWND {
        HWND(self.handle as *mut _)
    }
}
pub fn describe(hwnd: HWND) -> Option<Target> {
    unsafe {
        let mut title = [0u16; 512];
        let count = GetWindowTextW(hwnd, &mut title);
        if count == 0 {
            return None;
        }
        let mut pid = 0;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        Some(Target {
            handle: hwnd.0 as isize,
            pid,
            title: String::from_utf16_lossy(&title[..count as usize]),
        })
    }
}
pub fn targets() -> Result<Vec<Target>> {
    unsafe extern "system" fn visit(hwnd: HWND, param: LPARAM) -> BOOL {
        #[cfg(feature = "desktop")]
        if crate::overlay::is_overlay_window(hwnd) {
            return BOOL(1);
        }
        if IsWindowVisible(hwnd).as_bool() && !IsIconic(hwnd).as_bool() {
            if let Some(target) = describe(hwnd) {
                (*(param.0 as *mut Vec<Target>)).push(target);
            }
        }
        BOOL(1)
    }
    let mut result: Vec<Target> = Vec::new();
    unsafe {
        EnumWindows(Some(visit), LPARAM(&mut result as *mut _ as isize))?;
    }
    result.sort_by_key(|t| {
        (
            !t.title.to_lowercase().contains("apex legends"),
            t.title.clone(),
        )
    });
    Ok(result)
}
#[derive(Clone, Copy, PartialEq, Eq)]
struct Bounds {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
}
fn eligible(target: &Target) -> Result<Bounds> {
    unsafe {
        let hwnd = target.hwnd();
        ensure!(IsWindow(Some(hwnd)).as_bool(), "目标窗口已关闭");
        let mut pid = 0;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        ensure!(pid == target.pid, "目标窗口已变化，请重新选择");
        ensure!(!IsIconic(hwnd).as_bool(), "游戏已最小化，暂停截屏");
        ensure!(
            GetAncestor(GetForegroundWindow(), GA_ROOT) == hwnd,
            "请切回目标窗口前台，当前暂停截屏"
        );
        let mut rect = RECT::default();
        GetClientRect(hwnd, &mut rect)?;
        let mut origin = POINT::default();
        ClientToScreen(hwnd, &mut origin).ok()?;
        ensure!(rect.right > 0 && rect.bottom > 0, "等待有效客户区");
        for (x, y) in [
            (0.02, 0.02),
            (0.5, 0.5),
            (0.98, 0.98),
            (0.87, 0.06),
            (0.82, 0.18),
            (0.12, 0.738),
            (0.12, 0.798),
            (0.15, 0.904),
            (0.15, 0.43),
            (0.43, 0.43),
            (0.71, 0.43),
        ] {
            let point = POINT {
                x: origin.x + (rect.right as f64 * x) as i32,
                y: origin.y + (rect.bottom as f64 * y) as i32,
            };
            ensure!(
                GetAncestor(WindowFromPoint(point), GA_ROOT) == hwnd,
                "目标画面被其他窗口遮挡，暂停截屏"
            );
        }
        Ok(Bounds {
            x: origin.x,
            y: origin.y,
            width: rect.right as u32,
            height: rect.bottom as u32,
        })
    }
}
#[derive(Clone, Serialize)]
pub struct Snapshot {
    pub running: bool,
    pub status: String,
    pub frames: u64,
    pub attempts: u64,
    pub width: u32,
    pub height: u32,
    pub captured_at_ms: u128,
    pub interval_ms: u64,
    #[serde(skip)]
    pub image: Option<Arc<RgbaImage>>,
    #[serde(skip)]
    pub last_image: Option<Arc<RgbaImage>>,
}
impl Default for Snapshot {
    fn default() -> Self {
        Self {
            running: false,
            status: "尚未开始采集验证".into(),
            frames: 0,
            attempts: 0,
            width: 0,
            height: 0,
            captured_at_ms: 0,
            interval_ms: 2000,
            image: None,
            last_image: None,
        }
    }
}
struct Worker {
    stop: Arc<AtomicBool>,
    join: JoinHandle<()>,
}
pub struct CaptureManager {
    state: Arc<Mutex<Snapshot>>,
    interval_ms: Arc<AtomicU64>,
    worker: Mutex<Option<Worker>>,
}
impl Default for CaptureManager {
    fn default() -> Self {
        Self {
            state: Arc::new(Mutex::new(Snapshot::default())),
            interval_ms: Arc::new(AtomicU64::new(2000)),
            worker: Mutex::new(None),
        }
    }
}
impl CaptureManager {
    pub fn snapshot(&self) -> Snapshot {
        self.state.lock().unwrap().clone()
    }
    pub fn set_interval_ms(&self, interval_ms: u64) {
        let interval_ms = interval_ms.clamp(1000, 10_000);
        self.interval_ms.store(interval_ms, Ordering::Relaxed);
        self.state.lock().unwrap().interval_ms = interval_ms;
    }
    pub fn start(&self, target: Target) -> Result<()> {
        let mut worker = self.worker.lock().unwrap();
        ensure!(worker.is_none(), "采集已运行，请先停止");
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let state = self.state.clone();
        let interval_ms = self.interval_ms.clone();
        *state.lock().unwrap() = Snapshot {
            running: true,
            status: "等待目标窗口前台…".into(),
            interval_ms: interval_ms.load(Ordering::Relaxed),
            ..Default::default()
        };
        let join = thread::Builder::new()
            .name("low-rate-dxgi".into())
            .spawn(move || {
                unsafe {
                    let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL);
                }
                let mut reader: Option<Reader> = None;
                let mut due = Instant::now();
                while !stopping.load(Ordering::Relaxed) {
                    let bounds = match eligible(&target) {
                        Ok(v) => v,
                        Err(e) => {
                            reader = None;
                            let mut s = state.lock().unwrap();
                            s.status = e.to_string();
                            s.image = None;
                            drop(s);
                            thread::sleep(Duration::from_millis(100));
                            continue;
                        }
                    };
                    if Instant::now() < due {
                        thread::sleep(Duration::from_millis(50));
                        continue;
                    }
                    state.lock().unwrap().attempts += 1;
                    let result = (|| -> Result<Option<RgbaImage>> {
                        let monitor =
                            unsafe { MonitorFromWindow(target.hwnd(), MONITOR_DEFAULTTONEAREST) };
                        if reader.as_ref().is_none_or(|r| r.monitor != monitor) {
                            reader = Some(Reader::new(monitor)?);
                        }
                        reader.as_mut().unwrap().take(bounds, || {
                            stopping.load(Ordering::Relaxed) || eligible(&target).is_err()
                        })
                    })();
                    let interval = interval_ms.load(Ordering::Relaxed);
                    due = Instant::now() + Duration::from_millis(interval);
                    match result {
                        Ok(Some(image))
                            if !stopping.load(Ordering::Relaxed)
                                && eligible(&target).ok() == Some(bounds) =>
                        {
                            let mut s = state.lock().unwrap();
                            s.interval_ms = interval;
                            s.frames += 1;
                            s.width = image.width();
                            s.height = image.height();
                            s.captured_at_ms = SystemTime::now()
                                .duration_since(UNIX_EPOCH)
                                .unwrap()
                                .as_millis();
                            let image = Arc::new(image);
                            s.last_image = Some(image.clone());
                            s.image = Some(image);
                            s.status = "正在低频采集验证 · 尚未接入自动记录".into();
                        }
                        Ok(_) => {}
                        Err(e) => {
                            reader = None;
                            let mut s = state.lock().unwrap();
                            s.status = format!("采集等待重试：{e}");
                            s.image = None;
                        }
                    }
                }
                drop(reader);
                let mut s = state.lock().unwrap();
                s.running = false;
                s.image = None;
                s.status = "采集验证已停止".into();
            })?;
        *worker = Some(Worker { stop, join });
        Ok(())
    }
    pub fn stop(&self) {
        if let Some(w) = self.worker.lock().unwrap().take() {
            w.stop.store(true, Ordering::Relaxed);
            let _ = w.join.join();
        }
    }
}
impl Drop for CaptureManager {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod interval_tests {
    use super::CaptureManager;

    #[test]
    fn capture_interval_can_be_adjusted_without_starting_game_capture() {
        let capture = CaptureManager::default();
        assert_eq!(capture.snapshot().interval_ms, 2000);
        capture.set_interval_ms(1000);
        assert_eq!(capture.snapshot().interval_ms, 1000);
        capture.set_interval_ms(100);
        assert_eq!(capture.snapshot().interval_ms, 1000);
    }
}

struct Reader {
    monitor: HMONITOR,
    screen: RECT,
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    dup: IDXGIOutputDuplication,
    staging: Option<ID3D11Texture2D>,
    size: (u32, u32),
    owns: bool,
}
impl Reader {
    fn new(monitor: HMONITOR) -> Result<Self> {
        unsafe {
            let factory: IDXGIFactory1 = CreateDXGIFactory1()?;
            let mut a = 0;
            while let Ok(adapter) = factory.EnumAdapters1(a) {
                a += 1;
                let mut o = 0;
                while let Ok(output) = adapter.EnumOutputs(o) {
                    o += 1;
                    let d = output.GetDesc()?;
                    if d.Monitor != monitor || !d.AttachedToDesktop.as_bool() {
                        continue;
                    }
                    ensure!(
                        d.Rotation == DXGI_MODE_ROTATION_IDENTITY
                            || d.Rotation == DXGI_MODE_ROTATION_UNSPECIFIED,
                        "阶段 A 仅验证横向显示器"
                    );
                    let mut device = None;
                    let mut context = None;
                    D3D11CreateDevice(
                        &adapter.cast::<IDXGIAdapter>()?,
                        D3D_DRIVER_TYPE_UNKNOWN,
                        HMODULE::default(),
                        D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                        Some(&[
                            D3D_FEATURE_LEVEL_11_0,
                            D3D_FEATURE_LEVEL_10_1,
                            D3D_FEATURE_LEVEL_10_0,
                        ]),
                        D3D11_SDK_VERSION,
                        Some(&mut device),
                        None,
                        Some(&mut context),
                    )?;
                    let device = device.unwrap();
                    let context = context.unwrap();
                    let dup = output.cast::<IDXGIOutput1>()?.DuplicateOutput(&device)?;
                    return Ok(Self {
                        monitor,
                        screen: d.DesktopCoordinates,
                        device,
                        context,
                        dup,
                        staging: None,
                        size: (0, 0),
                        owns: false,
                    });
                }
            }
            bail!("未找到目标显示器的 DXGI 输出")
        }
    }
    fn take(&mut self, b: Bounds, cancel: impl Fn() -> bool) -> Result<Option<RgbaImage>> {
        ensure!(
            b.x >= self.screen.left
                && b.y >= self.screen.top
                && b.x + b.width as i32 <= self.screen.right
                && b.y + b.height as i32 <= self.screen.bottom,
            "请将完整窗口放在同一块显示器内"
        );
        ensure!(
            b.width as u64 * b.height as u64 <= 50_000_000,
            "客户区尺寸过大"
        );
        unsafe {
            // Keep ownership between samples, release immediately before next acquire.
            if self.owns {
                self.dup.ReleaseFrame()?;
                self.owns = false;
            }
            let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
            let mut resource = None;
            match self.dup.AcquireNextFrame(50, &mut info, &mut resource) {
                Err(e) if e.code() == DXGI_ERROR_WAIT_TIMEOUT => return Ok(None),
                r => r?,
            }
            self.owns = true;
            let texture: ID3D11Texture2D = resource.unwrap().cast()?;
            if self.size != (b.width, b.height) {
                self.staging = None;
                let desc = D3D11_TEXTURE2D_DESC {
                    Width: b.width,
                    Height: b.height,
                    MipLevels: 1,
                    ArraySize: 1,
                    Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    SampleDesc: DXGI_SAMPLE_DESC {
                        Count: 1,
                        Quality: 0,
                    },
                    Usage: D3D11_USAGE_STAGING,
                    BindFlags: 0,
                    CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                    MiscFlags: 0,
                };
                self.device
                    .CreateTexture2D(&desc, None, Some(&mut self.staging))?;
                self.size = (b.width, b.height);
            }
            let staging = self.staging.as_ref().unwrap();
            let x = (b.x - self.screen.left) as u32;
            let y = (b.y - self.screen.top) as u32;
            let region = D3D11_BOX {
                left: x,
                top: y,
                front: 0,
                right: x + b.width,
                bottom: y + b.height,
                back: 1,
            };
            self.context
                .CopySubresourceRegion(staging, 0, 0, 0, 0, &texture, 0, Some(&region));
            self.context.Flush();
            let started = Instant::now();
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            loop {
                if cancel() {
                    return Ok(None);
                }
                match self.context.Map(
                    staging,
                    0,
                    D3D11_MAP_READ,
                    D3D11_MAP_FLAG_DO_NOT_WAIT.0 as u32,
                    Some(&mut mapped),
                ) {
                    Err(e) if e.code() == DXGI_ERROR_WAS_STILL_DRAWING => {
                        ensure!(
                            started.elapsed() < Duration::from_millis(500),
                            "GPU 回读超时"
                        );
                        thread::sleep(Duration::from_millis(5));
                    }
                    r => {
                        r?;
                        break;
                    }
                }
            }
            let mut data = vec![0u8; (b.width * b.height * 4) as usize];
            for row in 0..b.height as usize {
                let source = std::slice::from_raw_parts(
                    (mapped.pData as *const u8).add(row * mapped.RowPitch as usize),
                    (b.width * 4) as usize,
                );
                data[row * b.width as usize * 4..(row + 1) * b.width as usize * 4]
                    .copy_from_slice(source);
            }
            self.context.Unmap(staging, 0);
            for p in data.chunks_exact_mut(4) {
                p.swap(0, 2);
                p[3] = 255;
            }
            Ok(Some(RgbaImage::from_raw(b.width, b.height, data).unwrap()))
        }
    }
}
impl Drop for Reader {
    fn drop(&mut self) {
        if self.owns {
            unsafe {
                let _ = self.dup.ReleaseFrame();
            }
        }
    }
}
