use crate::{
    capture::{CaptureManager, Snapshot, Target},
    ocr::OcrProbe,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::Serialize;
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};
use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager, State, Wry,
};
use windows::{
    core::HSTRING,
    Win32::{
        Foundation::{
            CloseHandle, GetLastError, SetLastError, ERROR_ALREADY_EXISTS, HANDLE, WAIT_OBJECT_0,
            WIN32_ERROR,
        },
        System::Threading::{
            CreateEventW, CreateMutexW, OpenEventW, SetEvent, WaitForSingleObject,
            EVENT_MODIFY_STATE,
        },
    },
};

static INSTANCE_MUTEX: std::sync::OnceLock<isize> = std::sync::OnceLock::new();
static INSTANCE_EVENT: std::sync::OnceLock<isize> = std::sync::OnceLock::new();
static SHUTTING_DOWN: AtomicBool = AtomicBool::new(false);

fn forward_to_existing_instance() -> bool {
    let mutex_name = HSTRING::from("Local\\BetterApex.RustLab.SingleInstance");
    let event_name = HSTRING::from("Local\\BetterApex.RustLab.ShowMainWindow");
    unsafe { SetLastError(WIN32_ERROR(0)) };
    let mutex = unsafe { CreateMutexW(None, false, &mutex_name) };
    let Ok(mutex) = mutex else {
        return false;
    };
    if unsafe { GetLastError() } != ERROR_ALREADY_EXISTS {
        let _ = INSTANCE_MUTEX.set(mutex.0 as isize);
        if let Ok(event) = unsafe { CreateEventW(None, false, false, &event_name) } {
            let _ = INSTANCE_EVENT.set(event.0 as isize);
        }
        return false;
    }

    let mut forwarded = false;
    for _ in 0..20 {
        if let Ok(event) = unsafe { OpenEventW(EVENT_MODIFY_STATE, false, &event_name) } {
            forwarded = unsafe { SetEvent(event) }.is_ok();
            let _ = unsafe { CloseHandle(event) };
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let _ = unsafe { CloseHandle(mutex) };
    forwarded
}

fn listen_for_activation(app: &AppHandle<Wry>) {
    let Some(event) = INSTANCE_EVENT.get().copied() else {
        return;
    };
    let app = app.clone();
    let _ = std::thread::Builder::new()
        .name("single-instance-activation".into())
        .spawn(move || loop {
            if SHUTTING_DOWN.load(Ordering::Relaxed) {
                break;
            }
            let wait = unsafe { WaitForSingleObject(HANDLE(event as *mut _), 1000) };
            if wait == WAIT_OBJECT_0 {
                let activation_app = app.clone();
                let _ = app.run_on_main_thread(move || show_main_window(&activation_app));
            }
        });
}

struct AppState {
    ocr_settings: Arc<crate::ocr_settings::OcrSettings>,
    auto_next: Arc<crate::auto_next::Settings>,
    nickname_debug: Arc<crate::nickname_debug::NicknameDebug>,
    capture: Arc<CaptureManager>,
    storage: crate::storage::LocalStore,
    automation: Arc<crate::automation::AutoManager>,
    selected_target: Arc<Mutex<Option<Target>>>,
    display: crate::overlay::OverlayManager,
    auto_menu_item: Mutex<Option<MenuItem<Wry>>>,
    tray_available: AtomicBool,
}

fn update_tray_auto_item(state: &AppState) {
    if let Some(item) = state.auto_menu_item.lock().unwrap().as_ref() {
        let label = if state.automation.status().running {
            "停止自动记录"
        } else {
            "开始自动记录"
        };
        let _ = item.set_text(label);
    }
}

fn show_main_window(app: &AppHandle<Wry>) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

fn toggle_auto_from_tray(app: &AppHandle<Wry>) {
    let state = app.state::<AppState>();
    if state.automation.status().running {
        let automation = state.automation.clone();
        let capture = state.capture.clone();
        let app = app.clone();
        tauri::async_runtime::spawn_blocking(move || {
            automation.stop(&capture);
            update_tray_auto_item(&app.state::<AppState>());
        });
        return;
    }

    let target = state.selected_target.lock().unwrap().clone();
    let Some(target) = target else {
        show_main_window(app);
        let _ = app.emit("tray-action-error", "请先在主界面选择 Apex 窗口。");
        return;
    };
    let resources = match crate::resources() {
        Ok(resources) => resources,
        Err(error) => {
            show_main_window(app);
            let _ = app.emit("tray-action-error", format!("自动记录无法启动：{error:#}"));
            return;
        }
    };
    let automation = state.automation.clone();
    let capture = state.capture.clone();
    let storage = state.storage.clone();
    let debug = state.nickname_debug.clone();
    let settings = state.ocr_settings.clone();
    let auto_next = state.auto_next.clone();
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        if let Err(error) = automation.start(
            target, capture, storage, resources, debug, settings, auto_next,
        ) {
            show_main_window(&app);
            let _ = app.emit("tray-action-error", format!("自动记录无法启动：{error:#}"));
        }
        update_tray_auto_item(&app.state::<AppState>());
    });
}

fn build_tray(app: &tauri::App<Wry>) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show-main", "打开 Better Apex", true, None::<&str>)?;
    let auto = MenuItem::with_id(app, "auto-toggle", "开始自动记录", true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, "quit", "退出程序", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &auto, &separator, &quit])?;
    if let Some(state) = app.try_state::<AppState>() {
        *state.auto_menu_item.lock().unwrap() = Some(auto);
    }
    let Some(icon) = app.default_window_icon().cloned() else {
        return Err(tauri::Error::InvalidIcon(std::io::Error::other(
            "default window icon is missing",
        )));
    };
    TrayIconBuilder::new()
        .menu(&menu)
        .show_menu_on_left_click(false)
        .icon(icon)
        .tooltip("Better Apex")
        .on_menu_event(|app, event| match event.id().as_ref() {
            "show-main" => show_main_window(app),
            "auto-toggle" => toggle_auto_from_tray(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if matches!(
                event,
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                }
            ) {
                show_main_window(&tray.app_handle());
            }
        })
        .build(app)?;
    if let Some(state) = app.try_state::<AppState>() {
        state.tray_available.store(true, Ordering::Relaxed);
    }
    Ok(())
}

fn handle_tray_menu(app: &tauri::App<Wry>) {
    if let Err(error) = build_tray(app) {
        eprintln!("Could not create tray icon: {error}");
    }
}
#[tauri::command]
fn auto_next_status(state: State<'_, AppState>) -> crate::auto_next::Status {
    state.auto_next.status()
}
#[tauri::command]
fn set_auto_next(
    enabled: bool,
    state: State<'_, AppState>,
) -> Result<crate::auto_next::Status, String> {
    state.auto_next.save(enabled).map_err(|e| e.to_string())
}
#[tauri::command]
fn display_status(state: State<'_, AppState>) -> crate::overlay::Status {
    state.display.status()
}
#[tauri::command]
fn set_display_settings(
    settings: crate::overlay::Settings,
    state: State<'_, AppState>,
) -> Result<crate::overlay::Status, String> {
    state
        .display
        .save(settings)
        .map_err(|error| error.to_string())
}
#[tauri::command]
fn preview_display(
    app: AppHandle<Wry>,
    state: State<'_, AppState>,
) -> Result<crate::overlay::Status, String> {
    let window = app.get_webview_window("main").ok_or("主窗口不可用")?;
    let hwnd = window.hwnd().map_err(|error| error.to_string())?;
    state
        .display
        .preview(hwnd.0 as isize)
        .map_err(|error| error.to_string())
}
#[tauri::command]
fn list_targets() -> Result<Vec<Target>, String> {
    crate::capture::targets().map_err(|e| e.to_string())
}
#[tauri::command]
fn nickname_debug_status(state: State<'_, AppState>) -> crate::nickname_debug::DebugStatus {
    state.nickname_debug.status()
}
#[tauri::command]
fn ocr_settings(
    state: State<'_, AppState>,
) -> Result<crate::ocr_settings::OcrSettingsStatus, String> {
    let root = crate::resources().map_err(|e| format!("{e:#}"))?;
    Ok(state.ocr_settings.status(&root))
}
#[tauri::command]
async fn set_ocr_backend(
    backend: crate::ocr_settings::OcrBackend,
    state: State<'_, AppState>,
) -> Result<crate::ocr_settings::OcrSettingsStatus, String> {
    let settings = state.ocr_settings.clone();
    let automation = state.automation.clone();
    let debug = state.nickname_debug.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let root = crate::resources().map_err(|e| format!("{e:#}"))?;
        automation
            .set_ocr_backend(&settings, backend, &root)
            .map_err(|e| format!("{e:#}"))?;
        let round = debug.round("settings");
        debug.context(&round, "settings", 0, 0).event(
            "ocr_backend_changed",
            serde_json::json!({"backend":backend,"label":backend.label()}),
        );
        Ok(settings.status(&root))
    })
    .await
    .map_err(|e| e.to_string())?
}
#[tauri::command]
async fn set_nickname_debug(
    enabled: bool,
    state: State<'_, AppState>,
) -> Result<crate::nickname_debug::DebugStatus, String> {
    let debug = state.nickname_debug.clone();
    tauri::async_runtime::spawn_blocking(move || {
        debug.set_enabled(enabled);
        debug.status()
    })
    .await
    .map_err(|e| e.to_string())
}
#[tauri::command]
async fn open_nickname_debug_folder(state: State<'_, AppState>) -> Result<(), String> {
    let directory = state.nickname_debug.status().directory;
    tauri::async_runtime::spawn_blocking(move || {
        std::fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
        std::process::Command::new("explorer.exe")
            .arg(directory)
            .spawn()
            .map_err(|e| e.to_string())?;
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
}
#[tauri::command]
async fn start_capture(target: Target, state: State<'_, AppState>) -> Result<(), String> {
    *state.selected_target.lock().unwrap() = Some(target.clone());
    let capture = state.capture.clone();
    tauri::async_runtime::spawn_blocking(move || capture.start(target).map_err(|e| e.to_string()))
        .await
        .map_err(|e| e.to_string())?
}
#[tauri::command]
fn remember_target(target: Option<Target>, state: State<'_, AppState>) {
    *state.selected_target.lock().unwrap() = target;
}
#[tauri::command]
async fn stop_capture(state: State<'_, AppState>) -> Result<(), String> {
    if state.automation.status().running {
        return Err("自动记录正在运行，请使用“停止自动记录”。".into());
    }
    let capture = state.capture.clone();
    tauri::async_runtime::spawn_blocking(move || capture.stop())
        .await
        .map_err(|e| e.to_string())
}
#[tauri::command]
async fn auto_record_status(
    state: State<'_, AppState>,
) -> Result<crate::automation::AutoRecordStatus, String> {
    update_tray_auto_item(&state);
    Ok(state.automation.status())
}
#[tauri::command]
async fn start_auto_recording(target: Target, state: State<'_, AppState>) -> Result<(), String> {
    *state.selected_target.lock().unwrap() = Some(target.clone());
    let automation = state.automation.clone();
    let capture = state.capture.clone();
    let storage = state.storage.clone();
    let debug = state.nickname_debug.clone();
    let settings = state.ocr_settings.clone();
    let auto_next = state.auto_next.clone();
    let resources = crate::resources().map_err(|error| format!("{error:#}"))?;
    let result = tauri::async_runtime::spawn_blocking(move || {
        automation
            .start(
                target, capture, storage, resources, debug, settings, auto_next,
            )
            .map_err(|error| format!("{error:#}"))
    })
    .await
    .map_err(|error| error.to_string())?;
    update_tray_auto_item(&state);
    result
}
#[tauri::command]
async fn stop_auto_recording(state: State<'_, AppState>) -> Result<(), String> {
    let automation = state.automation.clone();
    let capture = state.capture.clone();
    tauri::async_runtime::spawn_blocking(move || automation.stop(&capture))
        .await
        .map_err(|error| error.to_string())?;
    update_tray_auto_item(&state);
    Ok(())
}
#[derive(Serialize)]
struct CaptureView {
    #[serde(flatten)]
    status: Snapshot,
    preview: Option<String>,
}
#[tauri::command]
async fn capture_snapshot(
    preview: bool,
    state: State<'_, AppState>,
) -> Result<CaptureView, String> {
    let snapshot = state.capture.snapshot();
    tauri::async_runtime::spawn_blocking(move || {
        let preview = if preview {
            snapshot
                .last_image
                .as_ref()
                .map(|image| {
                    let thumb = image::imageops::thumbnail(image.as_ref(), 1000, 600);
                    crate::imaging::png(&thumb)
                        .map(|data| format!("data:image/png;base64,{}", STANDARD.encode(data)))
                })
                .transpose()
                .map_err(|e| e.to_string())?
        } else {
            None
        };
        Ok(CaptureView {
            status: snapshot,
            preview,
        })
    })
    .await
    .map_err(|e| e.to_string())?
}
#[tauri::command]
async fn save_capture(path: String, state: State<'_, AppState>) -> Result<String, String> {
    let image = state
        .capture
        .snapshot()
        .last_image
        .ok_or("尚无可保存的前台画面")?;
    tauri::async_runtime::spawn_blocking(move || {
        let path = PathBuf::from(path);
        if !path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("png"))
        {
            return Err("请保存为 PNG 文件".into());
        }
        let data = crate::imaging::png(&image).map_err(|e| e.to_string())?;
        std::fs::write(&path, data).map_err(|e| e.to_string())?;
        Ok(path.display().to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}
#[tauri::command]
async fn probe_image(
    path: String,
    kind: String,
    state: State<'_, AppState>,
) -> Result<OcrProbe, String> {
    if kind != "roster" && kind != "settlement" {
        return Err("不支持的验证类型".into());
    }
    let debug = state.nickname_debug.clone();
    let backend = state.ocr_settings.backend();
    tauri::async_runtime::spawn_blocking(move || {
        let round = debug.round("manual_probe");
        let trace = debug.context(&round, &kind, 1, 1);
        trace.event(
            "manual_started",
            serde_json::json!({"path":path,"kind":kind,"backend":backend}),
        );
        let result = crate::resources().and_then(|root| {
            crate::ocr::probe_selected(&PathBuf::from(path), &root, &kind, backend, Some(&trace))
        });
        match &result {
            Ok(result) => trace.event("manual_result", serde_json::json!(result)),
            Err(error) => trace.event(
                "manual_error",
                serde_json::json!({"error":format!("{error:#}")}),
            ),
        }
        result.map_err(|e| format!("{e:#}"))
    })
    .await
    .map_err(|e| e.to_string())?
}
#[derive(Serialize)]
struct ImagePreview {
    image: String,
    width: u32,
    height: u32,
    name: String,
}
fn preview_file(path: std::path::PathBuf) -> Result<ImagePreview, String> {
    let image = crate::imaging::load(&path).map_err(|e| e.to_string())?;
    let (width, height) = image.dimensions();
    let thumb = image::imageops::thumbnail(&image, 1600, 1000);
    let png = crate::imaging::png(&thumb).map_err(|e| e.to_string())?;
    Ok(ImagePreview {
        image: format!("data:image/png;base64,{}", STANDARD.encode(png)),
        width,
        height,
        name: path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
    })
}
#[tauri::command]
async fn image_preview(path: String) -> Result<ImagePreview, String> {
    tauri::async_runtime::spawn_blocking(move || preview_file(path.into()))
        .await
        .map_err(|e| e.to_string())?
}
#[tauri::command]
async fn local_storage_status(
    state: State<'_, AppState>,
) -> Result<crate::storage::LocalStoreStatus, String> {
    let storage = state.storage.clone();
    tauri::async_runtime::spawn_blocking(move || storage.status().map_err(|e| e.to_string()))
        .await
        .map_err(|e| e.to_string())?
}
#[tauri::command]
async fn list_matches(
    state: State<'_, AppState>,
) -> Result<Vec<crate::storage::SavedMatch>, String> {
    let storage = state.storage.clone();
    tauri::async_runtime::spawn_blocking(move || storage.list_matches().map_err(|e| e.to_string()))
        .await
        .map_err(|e| e.to_string())?
}
#[tauri::command]
async fn save_settlement(
    path: String,
    annotations: Vec<crate::storage::PlayerAnnotation>,
    selection_path: Option<String>,
    state: State<'_, AppState>,
) -> Result<crate::storage::SaveMatchResult, String> {
    let storage = state.storage.clone();
    // This command only accepts explicit manual edits/selections. Summary OCR
    // is never a fallback for an omitted name.
    let annotations: Vec<_> = annotations
        .into_iter()
        .map(|mut annotation| {
            annotation.nickname_source = crate::storage::NicknameSource::Manual;
            annotation
        })
        .collect();
    let debug = state.nickname_debug.clone();
    let backend = state.ocr_settings.backend();
    tauri::async_runtime::spawn_blocking(move || {
        let round = debug.round("manual_save");
        let trace = debug.context(&round,"03_settlement",1,1);
        trace.event("manual_save_started",serde_json::json!({"path":path,"annotations":annotations,"selection_path":selection_path}));
        let outcome = (|| -> Result<crate::storage::SaveMatchResult,String> {
        let path = PathBuf::from(path);
        let resources = crate::resources().map_err(|e| format!("{e:#}"))?;
        let image = crate::imaging::load(&path).map_err(|e| format!("{e:#}"))?;
        let mut reader = crate::nickname_ocr::NicknameRecognizer::new(&resources,backend).map_err(|e| format!("{e:#}"))?;
        let result = crate::settlement::recognize_image_selected(&image, &resources, &mut reader, Some(&trace)).map_err(|e| format!("{e:#}"))?;
        let selection_path = selection_path.map(PathBuf::from);
        storage
            .save_settlement(&result, &path, &annotations, selection_path.as_deref())
            .map_err(|e| format!("{e:#}"))
        })();
        match &outcome { Ok(saved)=>trace.event("match_saved",serde_json::json!({"saved":saved,"annotations":annotations})),Err(error)=>trace.event("manual_save_error",serde_json::json!({"error":error})) }
        outcome
    })
    .await
    .map_err(|e| e.to_string())?
}
#[tauri::command]
async fn match_image(
    match_id: String,
    kind: String,
    state: State<'_, AppState>,
) -> Result<ImagePreview, String> {
    let storage = state.storage.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let path = storage
            .evidence_path(&match_id, &kind)
            .map_err(|e| format!("{e:#}"))?;
        preview_file(path)
    })
    .await
    .map_err(|e| e.to_string())?
}
pub fn run() {
    if forward_to_existing_instance() {
        return;
    }
    unsafe {
        let _ = windows::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(
            windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        );
    }
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            listen_for_activation(app.handle());
            let data_dir = app
                .path()
                .app_local_data_dir()
                .map_err(|e| std::io::Error::other(e.to_string()))?;
            let nickname_debug = Arc::new(crate::nickname_debug::NicknameDebug::new(&data_dir));
            let ocr_settings = Arc::new(crate::ocr_settings::OcrSettings::open(&data_dir));
            let auto_next = Arc::new(crate::auto_next::Settings::open(&data_dir));
            let storage = crate::storage::LocalStore::open(data_dir.clone())
                .map_err(|e| std::io::Error::other(e.to_string()))?;
            let status_app = app.handle().clone();
            let automation = crate::automation::AutoManager::with_status_listener(move |status| {
                let _ = status_app.emit("auto-record-status", status);
                update_tray_auto_item(&status_app.state::<AppState>());
            });
            let automation = Arc::new(automation);
            let selected_target = Arc::new(Mutex::new(None));
            let display = crate::overlay::OverlayManager::open(
                &data_dir,
                automation.clone(),
                selected_target.clone(),
            );
            app.manage(AppState {
                ocr_settings,
                auto_next,
                nickname_debug,
                capture: Arc::new(CaptureManager::default()),
                storage,
                automation,
                selected_target,
                display,
                auto_menu_item: Mutex::new(None),
                tray_available: AtomicBool::new(false),
            });
            handle_tray_menu(app);
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let state = window.state::<AppState>();
                if state.automation.status().running {
                    api.prevent_close();
                    if state.tray_available.load(Ordering::Relaxed) {
                        let _ = window.hide();
                    } else {
                        let _ = window.emit(
                            "tray-action-error",
                            "托盘未能启动，自动记录仍在运行；请从主界面停止后再关闭程序。",
                        );
                    }
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            list_targets,
            remember_target,
            start_capture,
            stop_capture,
            auto_record_status,
            start_auto_recording,
            stop_auto_recording,
            capture_snapshot,
            save_capture,
            probe_image,
            image_preview,
            local_storage_status,
            list_matches,
            save_settlement,
            match_image,
            nickname_debug_status,
            set_nickname_debug,
            open_nickname_debug_folder,
            ocr_settings,
            set_ocr_backend,
            display_status,
            set_display_settings,
            preview_display,
            auto_next_status,
            set_auto_next
        ])
        .build(tauri::generate_context!())
        .expect("Cannot start compatibility lab");
    app.run(|app, event| {
        if matches!(event, tauri::RunEvent::ExitRequested { .. }) {
            SHUTTING_DOWN.store(true, Ordering::Relaxed);
            let state = app.state::<AppState>();
            state.display.stop();
            state.automation.stop(&state.capture);
            app.state::<AppState>().capture.stop();
        }
    });
}
