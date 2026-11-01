//! Lightweight, non-activating desktop display. No game hooks or extra webview.
use crate::{
    automation::{AutoManager, AutoStage},
    capture::Target,
};
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicIsize, Ordering},
        Arc, Mutex,
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};
use windows::{
    core::{w, HSTRING},
    Win32::{
        Foundation::{
            CloseHandle, COLORREF, FILETIME, HANDLE, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT,
            WPARAM,
        },
        Graphics::Gdi::*,
        System::{
            Diagnostics::ToolHelp::*, LibraryLoader::GetModuleHandleW, ProcessStatus::*,
            Threading::*,
        },
        UI::{HiDpi::GetDpiForWindow, WindowsAndMessaging::*},
    },
};

static OVERLAY_HWND: AtomicIsize = AtomicIsize::new(0);
const REFRESH: u32 = WM_APP + 1;

pub fn is_overlay_window(hwnd: HWND) -> bool {
    hwnd.0 as isize != 0 && OVERLAY_HWND.load(Ordering::Relaxed) == hwnd.0 as isize
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Position {
    #[default]
    TopCenter,
    BottomCenter,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub enabled: bool,
    pub position: Position,
    pub opacity: u8,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            enabled: false,
            position: Position::TopCenter,
            opacity: 85,
        }
    }
}
impl Settings {
    fn validate(&self) -> Result<()> {
        ensure!(
            (40..=100).contains(&self.opacity),
            "浮窗不透明度须在40至100之间"
        );
        Ok(())
    }
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Metrics {
    pub cpu_percent: Option<f64>,
    pub private_mib: Option<f64>,
    pub processes: usize,
    pub partial: bool,
}
#[derive(Clone, Serialize)]
pub struct Status {
    pub settings: Settings,
    pub ready: bool,
    pub error: String,
    pub metrics: Metrics,
}
struct Shared {
    status: Status,
    preview: Option<(isize, Instant)>,
    stopping: bool,
}

pub struct OverlayManager {
    path: PathBuf,
    shared: Arc<Mutex<Shared>>,
    worker: Mutex<Option<JoinHandle<()>>>,
}
impl OverlayManager {
    pub fn open(
        data_dir: &Path,
        automation: Arc<AutoManager>,
        target: Arc<Mutex<Option<Target>>>,
    ) -> Self {
        let path = data_dir.join("display-settings.json");
        let loaded = load_settings(&path);
        let (settings, error) = match loaded {
            Ok(s) => (s, String::new()),
            Err(e) => (Settings::default(), format!("浮窗设置读取失败：{e:#}")),
        };
        let shared = Arc::new(Mutex::new(Shared {
            status: Status {
                settings,
                ready: false,
                error,
                metrics: Metrics::default(),
            },
            preview: None,
            stopping: false,
        }));
        let state = shared.clone();
        let worker = std::thread::Builder::new()
            .name("status-display".into())
            .spawn(move || {
                if let Err(error) = run_window(state.clone(), automation, target) {
                    let mut state = state.lock().unwrap();
                    state.status.ready = false;
                    state.status.error = format!("浮窗不可用：{error:#}");
                }
            });
        let worker = match worker {
            Ok(w) => Some(w),
            Err(e) => {
                shared.lock().unwrap().status.error = e.to_string();
                None
            }
        };
        Self {
            path,
            shared,
            worker: Mutex::new(worker),
        }
    }
    pub fn status(&self) -> Status {
        self.shared.lock().unwrap().status.clone()
    }
    pub fn save(&self, settings: Settings) -> Result<Status> {
        settings.validate()?;
        let mut shared = self.shared.lock().unwrap();
        ensure!(shared.status.ready, "浮窗尚未就绪：{}", shared.status.error);
        write_settings(&self.path, &settings)?;
        shared.status.settings = settings;
        shared.status.error.clear();
        shared.preview = None;
        drop(shared);
        self.wake();
        Ok(self.status())
    }
    pub fn preview(&self, anchor: isize) -> Result<Status> {
        let mut shared = self.shared.lock().unwrap();
        ensure!(shared.status.ready, "浮窗尚未就绪：{}", shared.status.error);
        shared.preview = Some((anchor, Instant::now() + Duration::from_secs(15)));
        drop(shared);
        self.wake();
        Ok(self.status())
    }
    fn wake(&self) {
        unsafe {
            let _ = PostMessageW(
                Some(HWND(OVERLAY_HWND.load(Ordering::Relaxed) as *mut _)),
                REFRESH,
                WPARAM(0),
                LPARAM(0),
            );
        }
    }
    pub fn stop(&self) {
        // Mark shutdown before the worker creates its HWND as well.
        self.shared.lock().unwrap().stopping = true;
        if let Some(worker) = self.worker.lock().unwrap().take() {
            let hwnd = OVERLAY_HWND.load(Ordering::Relaxed);
            if hwnd != 0 {
                unsafe {
                    let _ =
                        PostMessageW(Some(HWND(hwnd as *mut _)), WM_CLOSE, WPARAM(0), LPARAM(0));
                }
            }
            // Setup normally finishes before exit; run_window also checks the
            // shutdown flag to handle an immediate exit during startup.
            let _ = worker.join();
        }
    }
}

fn load_settings(path: &Path) -> Result<Settings> {
    let settings: Settings = match std::fs::read(path) {
        Ok(b) => serde_json::from_slice(&b)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Settings::default(),
        Err(e) => return Err(e.into()),
    };
    settings.validate()?;
    Ok(settings)
}
fn write_settings(path: &Path, settings: &Settings) -> Result<()> {
    settings.validate()?;
    std::fs::create_dir_all(path.parent().context("浮窗设置目录无效")?)?;
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, serde_json::to_vec_pretty(settings)?)?;
    std::fs::rename(temporary, path).context("浮窗设置保存失败")
}

struct OwnedHandle(HANDLE);
impl Drop for OwnedHandle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}
fn filetime(time: FILETIME) -> u64 {
    ((time.dwHighDateTime as u64) << 32) | time.dwLowDateTime as u64
}
fn descendants(root: u32, entries: &[(u32, u32)]) -> HashSet<u32> {
    let mut included = HashSet::from([root]);
    loop {
        let before = included.len();
        for &(pid, parent) in entries {
            if included.contains(&parent) {
                included.insert(pid);
            }
        }
        if included.len() == before {
            return included;
        }
    }
}

#[derive(Default)]
struct Sampler {
    previous: HashMap<(u32, u64), u64>,
    at: Option<Instant>,
}
impl Sampler {
    fn sample(&mut self) -> Result<Metrics> {
        unsafe {
            let snapshot = OwnedHandle(CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0)?);
            let mut entry = PROCESSENTRY32W {
                dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
                ..Default::default()
            };
            Process32FirstW(snapshot.0, &mut entry)?;
            let mut entries = Vec::new();
            loop {
                entries.push((entry.th32ProcessID, entry.th32ParentProcessID));
                if Process32NextW(snapshot.0, &mut entry).is_err() {
                    break;
                }
            }
            let pids = descendants(std::process::id(), &entries);
            let mut counters = HashMap::new();
            let mut bytes = 0u64;
            let mut partial = false;
            let mut root_creation = FILETIME::default();
            let (mut exit, mut kernel, mut user) = (
                FILETIME::default(),
                FILETIME::default(),
                FILETIME::default(),
            );
            GetProcessTimes(
                GetCurrentProcess(),
                &mut root_creation,
                &mut exit,
                &mut kernel,
                &mut user,
            )?;
            for pid in pids {
                let queried = (|| -> Result<((u32, u64), u64, u64)> {
                    let handle = OwnedHandle(OpenProcess(
                        PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_VM_READ,
                        false,
                        pid,
                    )?);
                    let mut creation = FILETIME::default();
                    GetProcessTimes(handle.0, &mut creation, &mut exit, &mut kernel, &mut user)?;
                    // A stale parent PID from before this application started
                    // must not count as a WebView descendant after PID reuse.
                    ensure!(
                        filetime(creation) >= filetime(root_creation),
                        "stale process id"
                    );
                    let mut memory = PROCESS_MEMORY_COUNTERS_EX {
                        cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
                        ..Default::default()
                    };
                    GetProcessMemoryInfo(
                        handle.0,
                        &mut memory as *mut _ as *mut PROCESS_MEMORY_COUNTERS,
                        memory.cb,
                    )?;
                    Ok((
                        (pid, filetime(creation)),
                        filetime(kernel) + filetime(user),
                        memory.PrivateUsage as u64,
                    ))
                })();
                match queried {
                    Ok((key, cpu, memory)) => {
                        counters.insert(key, cpu);
                        bytes += memory;
                    }
                    Err(_) => partial = true,
                }
            }
            ensure!(!counters.is_empty(), "无法读取 Better Apex 资源占用");
            let now = Instant::now();
            let cpu_percent = self.at.map(|at| {
                cpu_load(
                    &self.previous,
                    &counters,
                    now.duration_since(at).as_secs_f64(),
                    std::thread::available_parallelism().map_or(1, |n| n.get()),
                )
            });
            self.at = Some(now);
            self.previous = counters;
            Ok(Metrics {
                cpu_percent,
                private_mib: Some(bytes as f64 / 1_048_576.0),
                processes: self.previous.len(),
                partial,
            })
        }
    }
}
fn cpu_load(
    previous: &HashMap<(u32, u64), u64>,
    current: &HashMap<(u32, u64), u64>,
    seconds: f64,
    processors: usize,
) -> f64 {
    let ticks: u64 = current
        .iter()
        .filter_map(|(key, value)| previous.get(key).map(|old| value.saturating_sub(*old)))
        .sum();
    if seconds <= 0.0 || processors == 0 {
        return 0.0;
    }
    (ticks as f64 / 10_000_000.0 / seconds / processors as f64 * 100.0).clamp(0.0, 100.0)
}

struct WindowContext {
    shared: Arc<Mutex<Shared>>,
    automation: Arc<AutoManager>,
    target: Arc<Mutex<Option<Target>>>,
    sampler: Sampler,
    sampled: Option<Instant>,
    lines: [String; 4],
    scale: f64,
    visible: bool,
}
impl WindowContext {
    fn update(&mut self, hwnd: HWND) -> Result<()> {
        let (settings, preview, stopping) = {
            let shared = self.shared.lock().unwrap();
            (
                shared.status.settings.clone(),
                shared.preview,
                shared.stopping,
            )
        };
        if stopping {
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
            return Ok(());
        }
        let preview = preview.filter(|(_, until)| *until > Instant::now());
        let target = self.target.lock().unwrap().clone();
        let anchor = if let Some((anchor, _)) = preview {
            Some(HWND(anchor as *mut _))
        } else if settings.enabled {
            target.as_ref().and_then(|t| unsafe {
                let game = HWND(t.handle as *mut _);
                let mut pid = 0;
                GetWindowThreadProcessId(game, Some(&mut pid));
                (pid == t.pid
                    && IsWindow(Some(game)).as_bool()
                    && !IsIconic(game).as_bool()
                    && GetAncestor(GetForegroundWindow(), GA_ROOT) == game)
                    .then_some(game)
            })
        } else {
            None
        };
        let Some(anchor) = anchor else {
            if self.visible {
                unsafe {
                    let _ = ShowWindow(hwnd, SW_HIDE);
                }
                self.visible = false;
            }
            self.sampler = Sampler::default();
            self.sampled = None;
            return Ok(());
        };
        let mut bounds = RECT::default();
        let mut origin = POINT::default();
        unsafe {
            GetClientRect(anchor, &mut bounds)?;
            ClientToScreen(anchor, &mut origin).ok()?;
        }
        if bounds.right <= 0 || bounds.bottom <= 0 {
            return Ok(());
        }
        self.scale = display_scale(
            bounds.right,
            unsafe { GetDpiForWindow(anchor).max(96) } as f64 / 96.0,
        );
        let rect = layout(
            origin.x,
            origin.y,
            bounds.right,
            bounds.bottom,
            self.scale,
            settings.position,
        );
        unsafe {
            SetLayeredWindowAttributes(
                hwnd,
                COLORREF(0),
                ((settings.opacity as u16 * 255) / 100) as u8,
                LWA_ALPHA,
            )?;
            SetWindowPos(
                hwnd,
                Some(HWND_TOPMOST),
                rect.left,
                rect.top,
                rect.right - rect.left,
                rect.bottom - rect.top,
                SWP_NOACTIVATE,
            )?;
        }
        if self
            .sampled
            .is_none_or(|at| at.elapsed() >= Duration::from_secs(2))
        {
            match self.sampler.sample() {
                Ok(metrics) => self.shared.lock().unwrap().status.metrics = metrics,
                Err(e) => {
                    let mut shared = self.shared.lock().unwrap();
                    shared.status.metrics = Metrics::default();
                    shared.status.error = format!("资源采样失败：{e:#}");
                }
            }
            self.sampled = Some(Instant::now());
        }
        let status = self.automation.status();
        let shared = self.shared.lock().unwrap();
        let cpu = shared
            .status
            .metrics
            .cpu_percent
            .map_or_else(|| "采样中".into(), |v| format!("{v:.1}%"));
        let memory = shared
            .status
            .metrics
            .private_mib
            .map_or_else(|| "—".into(), |v| format!("{v:.0} MB"));
        self.lines = [
            format!(
                "BETTER APEX  ·  {}",
                if preview.is_some() {
                    "浮窗预览"
                } else if !status.pause_reason.is_empty() {
                    "采集暂停"
                } else if status.running {
                    "记录中"
                } else {
                    "未开始记录"
                }
            ),
            format!("{}  {}", stage_label(status.stage), status.phase),
            format!(
                "CPU {cpu}    专用内存 {memory}    {} 场",
                status.saved_count
            ),
            if !shared.status.error.is_empty() {
                shared.status.error.clone()
            } else if shared.status.metrics.partial {
                "部分子进程资源暂不可读".into()
            } else if !status.error.is_empty() {
                status.error
            } else {
                status.pause_reason
            },
        ];
        drop(shared);
        unsafe {
            let _ = InvalidateRect(Some(hwnd), None, false);
            if !self.visible {
                let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
                self.visible = true;
            }
        }
        Ok(())
    }
    unsafe fn paint(&self, hwnd: HWND) {
        let mut paint = PAINTSTRUCT::default();
        let dc = BeginPaint(hwnd, &mut paint);
        let mut rect = RECT::default();
        let _ = GetClientRect(hwnd, &mut rect);
        let brush = CreateSolidBrush(COLORREF(0x1d1510));
        FillRect(dc, &rect, brush);
        let _ = DeleteObject(brush.into());
        let font = CreateFontW(
            -(15.0 * self.scale) as i32,
            0,
            0,
            0,
            500,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_DEFAULT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            CLEARTYPE_QUALITY,
            0,
            w!("Microsoft YaHei UI"),
        );
        let old = SelectObject(dc, font.into());
        SetBkMode(dc, TRANSPARENT);
        for (index, line) in self.lines.iter().enumerate() {
            if line.is_empty() {
                continue;
            }
            SetTextColor(
                dc,
                if index == 0 {
                    COLORREF(0x7597f2)
                } else if index == 3 {
                    COLORREF(0x80b8ff)
                } else {
                    COLORREF(0xf0ece8)
                },
            );
            let mut bounds = RECT {
                left: (12.0 * self.scale) as i32,
                top: ((8.0 + index as f64 * 23.0) * self.scale) as i32,
                right: rect.right - (10.0 * self.scale) as i32,
                bottom: rect.bottom,
            };
            // DrawText's native implementation may inspect the first code unit
            // even with count=0. Skip empty lines and keep a real terminator.
            let mut text: Vec<u16> = line.encode_utf16().chain(Some(0)).collect();
            DrawTextW(
                dc,
                &mut text,
                &mut bounds,
                DT_SINGLELINE | DT_END_ELLIPSIS | DT_NOPREFIX,
            );
        }
        SelectObject(dc, old);
        let _ = DeleteObject(font.into());
        let _ = EndPaint(hwnd, &paint);
    }
}
fn stage_label(stage: AutoStage) -> &'static str {
    match stage {
        AutoStage::Waiting => "等待",
        AutoStage::Loading => "00 加载",
        AutoStage::LegendSelect => "01 选传奇",
        AutoStage::InMatch => "02 局内",
        AutoStage::Settlement => "03 结算",
        AutoStage::Saving => "03 保存",
        AutoStage::Saved => "已保存",
    }
}
fn layout(x: i32, y: i32, width: i32, height: i32, scale: f64, position: Position) -> RECT {
    let scale = display_scale(width, scale);
    let w = ((440.0 * scale) as i32).min(width);
    let h = ((108.0 * scale) as i32).min(height);
    let margin = ((16.0 * scale) as i32).min((height - h).max(0));
    let top = y + match position {
        Position::TopCenter => margin,
        Position::BottomCenter => height - h - margin,
    };
    let left = x + (width - w) / 2;
    RECT {
        left,
        top,
        right: left + w,
        bottom: top + h,
    }
}
fn display_scale(width: i32, dpi_scale: f64) -> f64 {
    // Large desktop DPI must not expand the overlay into the left HUD or right
    // summary headers. Keep both presets inside the middle third of the game.
    dpi_scale.min(width as f64 * 0.33 / 440.0)
}
unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_NCCREATE {
        let create = &*(lparam.0 as *const CREATESTRUCTW);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
    }
    let context = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut WindowContext;
    match message {
        WM_NCHITTEST => return LRESULT(HTTRANSPARENT as isize),
        WM_MOUSEACTIVATE => return LRESULT(MA_NOACTIVATE as isize),
        WM_ERASEBKGND => return LRESULT(1),
        WM_PAINT if !context.is_null() => {
            (*context).paint(hwnd);
            return LRESULT(0);
        }
        WM_TIMER | REFRESH if !context.is_null() => {
            if let Err(e) = (*context).update(hwnd) {
                (*context).shared.lock().unwrap().status.error = format!("浮窗更新失败：{e:#}");
                let _ = ShowWindow(hwnd, SW_HIDE);
                (*context).visible = false;
            }
            return LRESULT(0);
        }
        WM_DESTROY => {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            PostQuitMessage(0);
            return LRESULT(0);
        }
        _ => {}
    }
    DefWindowProcW(hwnd, message, wparam, lparam)
}
fn run_window(
    shared: Arc<Mutex<Shared>>,
    automation: Arc<AutoManager>,
    target: Arc<Mutex<Option<Target>>>,
) -> Result<()> {
    unsafe {
        let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL);
        let instance = HINSTANCE(GetModuleHandleW(None)?.0);
        let class = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            lpszClassName: w!("BetterApex.StatusDisplay"),
            ..Default::default()
        };
        ensure!(RegisterClassW(&class) != 0, "无法注册浮窗类");
        let mut context = Box::new(WindowContext {
            shared: shared.clone(),
            automation,
            target,
            sampler: Sampler::default(),
            sampled: None,
            lines: Default::default(),
            scale: 1.0,
            visible: false,
        });
        let hwnd = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            w!("BetterApex.StatusDisplay"),
            &HSTRING::from("Better Apex Display"),
            WS_POPUP,
            0,
            0,
            440,
            108,
            None,
            None,
            Some(instance),
            Some((&mut *context as *mut WindowContext).cast()),
        )?;
        OVERLAY_HWND.store(hwnd.0 as isize, Ordering::Relaxed);
        shared.lock().unwrap().status.ready = true;
        if SetTimer(Some(hwnd), 1, 500, None) == 0 {
            let _ = DestroyWindow(hwnd);
            OVERLAY_HWND.store(0, Ordering::Relaxed);
            return Err(anyhow::anyhow!("无法创建浮窗更新定时器"));
        }
        let mut message = MSG::default();
        while GetMessageW(&mut message, None, 0, 0).0 > 0 {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
        let _ = KillTimer(Some(hwnd), 1);
        OVERLAY_HWND.store(0, Ordering::Relaxed);
        shared.lock().unwrap().status.ready = false;
        let _ = UnregisterClassW(w!("BetterApex.StatusDisplay"), Some(instance));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cpu_tree_and_counter_reuse_do_not_include_unrelated_or_new_processes() {
        assert_eq!(
            descendants(10, &[(30, 20), (99, 1), (20, 10)]),
            HashSet::from([10, 20, 30])
        );
        let old = HashMap::from([((10, 1), 10_000_000), ((20, 2), 20_000_000)]);
        let new = HashMap::from([((10, 1), 20_000_000), ((20, 3), 900_000_000)]);
        assert_eq!(cpu_load(&old, &new, 2.0, 4), 12.5);
        assert_eq!(cpu_load(&old, &new, 0.0, 4), 0.0);
    }
    #[test]
    fn settings_defaults_validation_and_persistence() -> Result<()> {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!(
            "../../.local/storage-tests/display-settings-{}.json",
            time::OffsetDateTime::now_utc().unix_timestamp_nanos()
        ));
        let defaults = load_settings(&path)?;
        assert!(!defaults.enabled);
        let settings = Settings {
            enabled: true,
            position: Position::BottomCenter,
            opacity: 70,
        };
        write_settings(&path, &settings)?;
        let restored = load_settings(&path)?;
        assert!(restored.enabled);
        assert_eq!(restored.position, Position::BottomCenter);
        assert_eq!(restored.opacity, 70);
        assert!(Settings {
            opacity: 0,
            ..settings
        }
        .validate()
        .is_err());
        std::fs::remove_file(path)?;
        Ok(())
    }
    #[test]
    fn display_layout_follows_negative_monitor_origins_and_dpi() {
        for position in [Position::TopCenter, Position::BottomCenter] {
            let rect = layout(-2560, -100, 2560, 1440, 1.5, position);
            assert_eq!((rect.right + rect.left) / 2, -1280);
            assert!(rect.top >= -100 && rect.bottom <= 1340);
            // Both presets avoid the HUD/summary fields and capture probe points.
            assert!(
                rect.left > -2560 + (2560.0 * 0.30) as i32
                    && rect.right < -2560 + (2560.0 * 0.70) as i32
            );
        }
        for width in [1920, 2560] {
            for scale in [1.0, 1.5, 2.0, 3.0] {
                for position in [Position::TopCenter, Position::BottomCenter] {
                    let rect = layout(0, 0, width, width * 9 / 16, scale, position);
                    assert!(rect.left as f64 > width as f64 * 0.31);
                    assert!((rect.right as f64) < width as f64 * 0.69);
                }
            }
        }
    }
    #[test]
    fn current_process_tree_can_be_sampled_without_game_access() -> Result<()> {
        let mut sampler = Sampler::default();
        let sample = sampler.sample()?;
        assert!(sample.processes >= 1);
        assert!(sample.private_mib.unwrap() > 0.0);
        assert!(sample.cpu_percent.is_none());
        let second = sampler.sample()?;
        assert!((0.0..=100.0).contains(&second.cpu_percent.unwrap()));
        Ok(())
    }

    #[test]
    fn native_preview_is_click_through_keeps_focus_and_shuts_down() -> Result<()> {
        let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!(
            "../../.local/storage-tests/display-{}",
            time::OffsetDateTime::now_utc().unix_timestamp_nanos()
        ));
        unsafe {
            // An owned, hidden fixture supplies a real client rectangle. The
            // test never activates a game or captures another application's UI.
            let anchor = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                w!("STATIC"),
                w!("Better Apex display fixture"),
                WS_OVERLAPPEDWINDOW,
                100,
                100,
                1180,
                820,
                None,
                None,
                None,
                None,
            )?;
            let foreground = GetForegroundWindow();
            let manager = OverlayManager::open(
                &directory,
                Arc::new(AutoManager::default()),
                Arc::new(Mutex::new(None)),
            );
            let outcome = (|| -> Result<()> {
                let deadline = Instant::now() + Duration::from_secs(5);
                while !manager.status().ready && Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(20));
                }
                ensure!(manager.status().ready, "{}", manager.status().error);
                manager.preview(anchor.0 as isize)?;
                let hwnd = HWND(OVERLAY_HWND.load(Ordering::Relaxed) as *mut _);
                let deadline = Instant::now() + Duration::from_secs(3);
                while !IsWindowVisible(hwnd).as_bool() && Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(20));
                }
                ensure!(
                    IsWindowVisible(hwnd).as_bool(),
                    "preview was not shown: {}",
                    manager.status().error
                );
                ensure!(GetForegroundWindow() == foreground, "display stole focus");
                let style = WINDOW_EX_STYLE(GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32);
                ensure!(
                    style.contains(
                        WS_EX_NOACTIVATE | WS_EX_TRANSPARENT | WS_EX_LAYERED | WS_EX_TOOLWINDOW
                    ),
                    "display styles missing"
                );
                let mut rect = RECT::default();
                GetWindowRect(hwnd, &mut rect)?;
                ensure!(
                    WindowFromPoint(POINT {
                        x: (rect.left + rect.right) / 2,
                        y: (rect.top + rect.bottom) / 2
                    }) != hwnd,
                    "display intercepts pointer hit testing"
                );
                ensure!(
                    !crate::capture::targets()?
                        .iter()
                        .any(|target| target.handle == hwnd.0 as isize),
                    "display listed as capture target"
                );
                ensure!(
                    manager.status().metrics.private_mib.is_some(),
                    "preview metrics missing"
                );
                manager.save(Settings::default())?; // also cancel an active preview
                let deadline = Instant::now() + Duration::from_secs(3);
                while IsWindowVisible(hwnd).as_bool() && Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(20));
                }
                ensure!(
                    !IsWindowVisible(hwnd).as_bool(),
                    "disabled preview remained visible"
                );
                Ok(())
            })();
            manager.stop();
            let _ = DestroyWindow(anchor);
            ensure!(
                OVERLAY_HWND.load(Ordering::Relaxed) == 0,
                "native display leaked"
            );
            if directory.exists() {
                std::fs::remove_dir_all(&directory)?;
            }
            outcome
        }
    }
}
