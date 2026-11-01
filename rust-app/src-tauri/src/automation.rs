use crate::nickname::{self, HUD_SAMPLE_LIMIT};
use crate::{
    capture::{CaptureManager, Target},
    ocr::Tesseract,
    settlement::{NumberField, SettlementResult},
    storage::{LocalStore, NicknameSource, PlayerAnnotation},
};
use anyhow::{ensure, Context, Result};
use image::RgbaImage;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use windows::Win32::System::Threading::{
    GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_BELOW_NORMAL,
};

const DAMAGE_RETRY_LIMIT: u8 = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Page {
    Unknown,
    Loading,
    LegendSelect,
    InMatch,
    Settlement,
}

struct Observation {
    page: Page,
    token: Option<String>,
    squads: Option<u8>,
    new_round_intro: bool,
}

struct PageDetector {
    marker: Tesseract,
    windows: crate::ocr::WindowsRecognizer,
    names: crate::nickname_ocr::NicknameRecognizer,
    root: std::path::PathBuf,
}

impl PageDetector {
    #[cfg(test)]
    fn new(root: &Path) -> Result<Self> {
        Self::with_backend(root, crate::ocr_settings::OcrBackend::WindowsTesseract)
    }

    fn with_backend(root: &Path, backend: crate::ocr_settings::OcrBackend) -> Result<Self> {
        Ok(Self {
            marker: Tesseract::new(root, "eng")?,
            windows: crate::ocr::WindowsRecognizer::new()?,
            names: crate::nickname_ocr::NicknameRecognizer::new(root, backend)?,
            root: root.to_path_buf(),
        })
    }

    fn observe(&mut self, image: &RgbaImage, look_for_intro: bool) -> Result<Observation> {
        ensure!(
            (image.width() as f64 / image.height() as f64 - 16.0 / 9.0).abs() < 0.025,
            "自动记录目前需要完整 16:9 游戏画面"
        );
        let header = self.read_text(image, (0.690, 0.135, 0.255, 0.085))?;
        if header.contains("小队排名") && header.contains("小队总击杀") {
            return Ok(Observation {
                page: Page::Settlement,
                token: self.read_token(image)?,
                squads: None,
                new_round_intro: false,
            });
        }
        let hud = self.read_text(image, (0.800, 0.035, 0.145, 0.060))?;
        if let Some(squads) = remaining_squads(&hud) {
            return Ok(Observation {
                page: Page::InMatch,
                token: self.read_token(image)?,
                squads: Some(squads),
                new_round_intro: false,
            });
        }
        if look_for_intro {
            let legend_prompt = self.read_text(image, (0.03, 0.34, 0.28, 0.18))?;
            if legend_prompt.contains("小队静音") && legend_prompt.contains("选择成为跳伞指挥官")
            {
                return Ok(Observation {
                    page: Page::LegendSelect,
                    token: None,
                    squads: None,
                    new_round_intro: true,
                });
            }
            let loading_marker = self.read_text(image, (0.035, 0.04, 0.17, 0.12))?;
            if loading_marker.contains("三人赛") {
                return Ok(Observation {
                    page: Page::Loading,
                    token: None,
                    squads: None,
                    new_round_intro: true,
                });
            }
        }
        Ok(Observation {
            page: Page::Unknown,
            token: None,
            squads: None,
            new_round_intro: false,
        })
    }

    fn read_text(&self, image: &RgbaImage, rect: (f64, f64, f64, f64)) -> Result<String> {
        let crop = crate::imaging::region(image, rect.0, rect.1, rect.2, rect.3, 2.0);
        Ok(self
            .windows
            .read(&crop)?
            .chars()
            .filter(|character| !character.is_whitespace())
            .collect())
    }

    fn read_token(&mut self, image: &RgbaImage) -> Result<Option<String>> {
        let crop = crate::imaging::region(image, 0.0, 0.925, 0.15, 0.028, 3.0);
        let native = extract_token(&self.windows.read(&crop)?);
        let (marker, confidence) = self
            .marker
            .read(&crop, 7, Some("0123456789abcdefABCDEF:"))?;
        let recognized = extract_token(&marker);
        if confidence >= 0.65 {
            return Ok(recognized);
        }
        if confidence >= 0.35 && recognized.is_some() && recognized == native {
            return Ok(recognized);
        }
        Ok(None)
    }
}

fn extract_token(text: &str) -> Option<String> {
    let (_, tail) = text.rsplit_once(':')?;
    let tail = tail.trim();
    let token: String = tail
        .chars()
        .filter(|character| character.is_ascii_hexdigit())
        .take(8)
        .map(|character| character.to_ascii_lowercase())
        .collect();
    (token.len() == 8).then_some(token)
}

fn remaining_squads(text: &str) -> Option<u8> {
    let marker = text.find("剩余小队")?;
    let digits: String = text[..marker]
        .chars()
        .rev()
        .take_while(|character| character.is_ascii_digit())
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    let count = digits.parse::<u8>().ok()?;
    (count > 0).then_some(count)
}

pub(crate) fn clean_roster_name(value: &str) -> String {
    let characters: Vec<char> = value.chars().collect();
    let mut cleaned = String::new();
    for (index, character) in characters.iter().copied().enumerate() {
        if character.is_whitespace() {
            let previous_is_ascii = index
                .checked_sub(1)
                .and_then(|previous| characters.get(previous))
                .is_some_and(char::is_ascii_alphanumeric);
            let next_is_ascii = characters
                .get(index + 1)
                .is_some_and(char::is_ascii_alphanumeric);
            if previous_is_ascii && next_is_ascii && !cleaned.ends_with(' ') {
                cleaned.push(' ');
            }
            continue;
        }
        cleaned.push(character);
    }
    cleaned
        .trim_matches(|character: char| "，,。".contains(character) || character.is_whitespace())
        .to_string()
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AutoStage {
    #[default]
    Waiting,
    Loading,
    LegendSelect,
    InMatch,
    Settlement,
    Saving,
    Saved,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AutoRecordStatus {
    pub running: bool,
    pub stage: AutoStage,
    pub phase: String,
    pub pause_reason: String,
    pub roster: String,
    pub last_saved: String,
    pub saved_count: i64,
    pub processed_frames: u64,
    pub interval_ms: u64,
    pub error: String,
}

impl Default for AutoRecordStatus {
    fn default() -> Self {
        Self {
            running: false,
            stage: AutoStage::Waiting,
            phase: "等待开始自动记录".into(),
            pause_reason: String::new(),
            roster: "本场名单：未采集".into(),
            last_saved: String::new(),
            saved_count: 0,
            processed_frames: 0,
            interval_ms: 2000,
            error: String::new(),
        }
    }
}

struct Worker {
    stop: Arc<AtomicBool>,
    join: JoinHandle<()>,
}

type StatusListener = Box<dyn Fn(AutoRecordStatus) + Send + Sync>;

#[derive(Default)]
struct StatusChannel {
    snapshot: Mutex<AutoRecordStatus>,
    listener: Option<StatusListener>,
}

impl StatusChannel {
    fn get(&self) -> AutoRecordStatus {
        self.snapshot.lock().unwrap().clone()
    }

    fn publish(&self, next: AutoRecordStatus) {
        let mut snapshot = self.snapshot.lock().unwrap();
        if *snapshot == next {
            return;
        }
        *snapshot = next.clone();
        drop(snapshot);
        // Never call the desktop bridge with the snapshot mutex held.
        if let Some(listener) = &self.listener {
            listener(next);
        }
    }
}

pub struct AutoManager {
    status: Arc<StatusChannel>,
    worker: Mutex<Option<Worker>>,
}

impl Default for AutoManager {
    fn default() -> Self {
        Self {
            status: Arc::new(StatusChannel::default()),
            worker: Mutex::new(None),
        }
    }
}

impl AutoManager {
    pub fn with_status_listener(
        listener: impl Fn(AutoRecordStatus) + Send + Sync + 'static,
    ) -> Self {
        Self {
            status: Arc::new(StatusChannel {
                listener: Some(Box::new(listener)),
                ..StatusChannel::default()
            }),
            worker: Mutex::new(None),
        }
    }

    pub fn status(&self) -> AutoRecordStatus {
        self.status.get()
    }

    pub fn start(
        &self,
        target: Target,
        capture: Arc<CaptureManager>,
        storage: LocalStore,
        resources: std::path::PathBuf,
        debug: Arc<crate::nickname_debug::NicknameDebug>,
        settings: Arc<crate::ocr_settings::OcrSettings>,
        auto_next: Arc<crate::auto_next::Settings>,
    ) -> Result<()> {
        let mut worker = self.worker.lock().unwrap();
        if worker
            .as_ref()
            .is_some_and(|worker| worker.join.is_finished())
        {
            if let Some(worker) = worker.take() {
                let _ = worker.join.join();
            }
        }
        ensure!(worker.is_none(), "自动记录已经运行");
        let backend = settings.backend();
        capture.set_interval_ms(2000);
        capture.start(target.clone())?;
        let stop = Arc::new(AtomicBool::new(false));
        let status = self.status.clone();
        status.publish(AutoRecordStatus {
            running: true,
            phase: "等待目标窗口前台画面".into(),
            ..AutoRecordStatus::default()
        });
        let stopping = stop.clone();
        let capture_thread = capture.clone();
        let join = match thread::Builder::new()
            .name("low-priority-match-ocr".into())
            .spawn(move || {
                run_recorder(
                    stopping,
                    capture_thread,
                    storage,
                    resources,
                    status,
                    debug,
                    backend,
                    target,
                    auto_next,
                )
            }) {
            Ok(join) => join,
            Err(error) => {
                capture.stop();
                let mut status = self.status.get();
                status.running = false;
                self.status.publish(status);
                return Err(error.into());
            }
        };
        *worker = Some(Worker { stop, join });
        Ok(())
    }

    pub fn set_ocr_backend(
        &self,
        settings: &crate::ocr_settings::OcrSettings,
        backend: crate::ocr_settings::OcrBackend,
        resources: &Path,
    ) -> Result<()> {
        let worker = self.worker.lock().unwrap();
        crate::ocr_settings::require_idle(
            worker
                .as_ref()
                .is_some_and(|worker| !worker.join.is_finished()),
        )?;
        // Initialize before persisting: missing/broken assets must not replace a working choice.
        let _reader = crate::nickname_ocr::NicknameRecognizer::new(resources, backend)?;
        settings.save(backend)
    }

    pub fn stop(&self, capture: &CaptureManager) {
        if let Some(worker) = self.worker.lock().unwrap().take() {
            worker.stop.store(true, Ordering::Relaxed);
            let _ = worker.join.join();
        }
        capture.stop();
        capture.set_interval_ms(2000);
        let mut status = self.status.get();
        status.running = false;
        status.phase = "自动记录已停止".into();
        status.pause_reason.clear();
        status.interval_ms = 2000;
        self.status.publish(status);
    }
}

fn run_recorder(
    stopping: Arc<AtomicBool>,
    capture: Arc<CaptureManager>,
    storage: LocalStore,
    resources: std::path::PathBuf,
    status: Arc<StatusChannel>,
    debug: Arc<crate::nickname_debug::NicknameDebug>,
    backend: crate::ocr_settings::OcrBackend,
    target: Target,
    auto_next: Arc<crate::auto_next::Settings>,
) {
    unsafe {
        let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL);
    }
    let mut detector = match PageDetector::with_backend(&resources, backend) {
        Ok(detector) => detector,
        Err(error) => {
            let mut next = status.get();
            next.error = format!("自动记录无法启动：{error:#}");
            next.running = false;
            status.publish(next);
            capture.stop();
            capture.set_interval_ms(2000);
            return;
        }
    };
    let mut recorder = Recorder::default();
    recorder.trace_round = debug.round("auto");
    recorder.trace = Some(debug.clone());
    if let Some(trace) = recorder.trace_context("lifecycle", 0) {
        trace.event("ocr_backend",serde_json::json!({"backend":backend,"label":backend.label(),"scope":"nickname_regions_and_settlement_hints","cpu_threads":1}));
    }
    recorder.status.saved_count = storage
        .status()
        .map_or(0, |store_status| store_status.record_count);
    let mut seen = 0_u64;
    let mut last_frame_at: Option<Instant> = None;
    recover_pending(&storage, &mut recorder.status);
    status.publish(recorder.status.clone());
    let mut last_recovery = Instant::now();
    let mut next_gate = crate::auto_next::Gate::default();
    while !stopping.load(Ordering::Relaxed) {
        let snapshot = capture.snapshot();
        if snapshot.frames > seen {
            seen = snapshot.frames;
            if let Some(image) = snapshot.last_image.as_ref() {
                last_frame_at = Some(Instant::now());
                recorder.status.pause_reason.clear();
                if let Err(error) = recorder.process(&image, &mut detector, &storage) {
                    if let Some(trace) = recorder.trace_context("processing", 0) {
                        trace.event(
                            "processing_error",
                            serde_json::json!({"error":format!("{error:#}")}),
                        );
                    }
                    recorder.status.error = format!("自动记录暂未完成：{error:#}");
                } else {
                    recorder.status.error.clear();
                }
                if let Some(id) = recorder.auto_next_request.take() {
                    if auto_next.enabled() {
                        next_gate.arm(id, seen, Instant::now());
                        recorder.auto_next_notice =
                            Some("战绩已保存，确认结算画面后自动按1".into());
                    }
                }
                let debug_error = debug.status().error;
                if debug.enabled() && !debug_error.is_empty() {
                    recorder.status.error.push_str(&format!(" {debug_error}"));
                }
                let interval_ms = recorder.capture_interval_ms();
                capture.set_interval_ms(interval_ms);
                recorder.status.interval_ms = interval_ms;
                recorder.status.processed_frames = seen;
                status.publish(recorder.status.clone());
            }
        } else if last_frame_at.is_some_and(|last| last.elapsed() > Duration::from_secs(20)) {
            let reason = if snapshot.image.is_some() {
                "目标画面暂无更新。"
            } else {
                &snapshot.status
            };
            recorder.on_capture_gap(last_frame_at.unwrap().elapsed(), reason);
            capture.set_interval_ms(2000);
            recorder.status.interval_ms = 2000;
            last_frame_at = None;
            status.publish(recorder.status.clone());
        }
        let fresh_frame = if snapshot.image.is_some()
            && observation_time_ms().saturating_sub(snapshot.captured_at_ms) <= 1500
        {
            snapshot.frames.min(seen)
        } else {
            0
        };
        match next_gate.poll(
            fresh_frame,
            Instant::now(),
            auto_next.enabled(),
            crate::auto_next::foreground(&target),
            recorder.locked && recorder.status.stage == AutoStage::Saved,
        ) {
            crate::auto_next::Decision::Wait => {}
            crate::auto_next::Decision::Cancel(reason) => {
                if let Some(trace) = recorder.trace_context("auto_next", 0) {
                    trace.event("auto_next_cancelled", serde_json::json!({"reason":reason}));
                }
                recorder.auto_next_notice = if recorder.status.stage == AutoStage::Saved {
                    Some(format!("战绩已保存；{reason}"))
                } else {
                    None
                };
            }
            crate::auto_next::Decision::Ready(id) => {
                let outcome = (|| -> Result<()> {
                    let image = snapshot.image.as_ref().context("缺少当前游戏画面")?;
                    let header = detector.read_text(image, (0.690, 0.135, 0.255, 0.085))?;
                    ensure!(
                        header.contains("小队排名") && header.contains("小队总击杀"),
                        "当前已不是结算页，请手动继续"
                    );
                    let prompt = detector.windows.read(&crate::imaging::region(
                        image, 0.730, 0.0, 0.270, 0.105, 2.0,
                    ))?;
                    ensure!(
                        crate::auto_next::rematch_prompt(&prompt),
                        "没有可用的重新匹配提示，请手动按1"
                    );
                    ensure!(
                        capture.snapshot().frames == snapshot.frames
                            && observation_time_ms().saturating_sub(snapshot.captured_at_ms)
                                <= 1500,
                        "确认画面已变化，请手动继续"
                    );
                    crate::auto_next::press_one(&target, &stopping, &auto_next)
                })();
                if let Some(trace) = recorder.trace_context("auto_next", 0) {
                    trace.event("auto_next_result",serde_json::json!({"match_id":id,"sent":outcome.is_ok(),"error":outcome.as_ref().err().map(|e|format!("{e:#}"))}));
                }
                recorder.auto_next_notice = Some(match outcome {
                    Ok(()) => "战绩已保存，已发送数字1，等待新局".into(),
                    Err(e) => format!("战绩已保存；自动下一局未完成：{e:#}"),
                });
            }
        }
        if recorder.status.stage == AutoStage::Saved {
            if let Some(notice) = &recorder.auto_next_notice {
                recorder.status.phase = notice.clone();
                status.publish(recorder.status.clone());
            }
        }
        if last_recovery.elapsed() >= Duration::from_secs(10) {
            recover_pending(&storage, &mut recorder.status);
            recorder.status.processed_frames = seen;
            status.publish(recorder.status.clone());
            last_recovery = Instant::now();
        }
        thread::sleep(Duration::from_millis(100));
    }
    let mut next = status.get();
    if let Some(trace) = recorder.trace_context("lifecycle", 0) {
        trace.event("auto_stopped", serde_json::json!({"stage":recorder.status.stage,"phase":recorder.status.phase,"roster":recorder.status.roster}));
    }
    next.running = false;
    status.publish(next);
}

#[derive(Default)]
struct RosterSnapshot {
    names: Vec<String>,
    local_slot: Option<usize>,
    image: Option<Arc<RgbaImage>>,
}

struct Recorder {
    auto_next_request: Option<String>,
    auto_next_notice: Option<String>,
    trace: Option<Arc<crate::nickname_debug::NicknameDebug>>,
    trace_round: String,
    trace_frame: u64,
    settlement_trace_frame: u64,
    status: AutoRecordStatus,
    active: bool,
    locked: bool,
    settlement_seen: bool,
    continuity_uncertain: bool,
    intro_seen: bool,
    intro_reset_done: bool,
    hud_confirmations: u8,
    intro_votes: u8,
    token_votes: u8,
    last_token: Option<String>,
    session_token: Option<String>,
    roster: Option<RosterSnapshot>,
    roster_readings: Vec<String>,
    roster_image: Option<Arc<RgbaImage>>,
    roster_attempts: u8,
    settlement_frames: Vec<Arc<RgbaImage>>,
    last_settlement_result: Option<SettlementResult>,
    damage_retries: u8,
}

impl Default for Recorder {
    fn default() -> Self {
        Self {
            auto_next_request: None,
            auto_next_notice: None,
            trace: None,
            trace_round: String::new(),
            trace_frame: 0,
            settlement_trace_frame: 0,
            status: AutoRecordStatus {
                running: true,
                phase: "等待进入对局".into(),
                ..AutoRecordStatus::default()
            },
            active: false,
            locked: false,
            settlement_seen: false,
            continuity_uncertain: false,
            intro_seen: false,
            intro_reset_done: false,
            hud_confirmations: 0,
            intro_votes: 0,
            token_votes: 0,
            last_token: None,
            session_token: None,
            roster: None,
            roster_readings: Vec::new(),
            roster_image: None,
            roster_attempts: 0,
            settlement_frames: Vec::new(),
            last_settlement_result: None,
            damage_retries: 0,
        }
    }
}

impl Recorder {
    fn trace_context(
        &self,
        stage: &str,
        sample: u8,
    ) -> Option<crate::nickname_debug::TraceContext> {
        self.trace
            .as_ref()
            .filter(|log| log.enabled())
            .map(|log| log.context(&self.trace_round, stage, self.trace_frame, sample))
    }

    fn process(
        &mut self,
        image: &Arc<RgbaImage>,
        detector: &mut PageDetector,
        storage: &LocalStore,
    ) -> Result<()> {
        self.trace_frame = self.trace_frame.saturating_add(1);
        let observation = detector.observe(
            image,
            !self.active
                || self.locked
                || self.intro_seen
                || self.continuity_uncertain
                || self.settlement_seen,
        )?;
        self.process_observation(image, observation, detector, storage)
    }

    fn process_observation(
        &mut self,
        image: &Arc<RgbaImage>,
        observation: Observation,
        detector: &mut PageDetector,
        storage: &LocalStore,
    ) -> Result<()> {
        self.status.pause_reason.clear();
        if self.session_token.is_some() && observation.token == self.session_token {
            self.continuity_uncertain = false;
        }
        if observation.new_round_intro {
            if !self.intro_reset_done {
                self.begin_new_round();
                self.intro_reset_done = true;
            }
            self.intro_votes = self.intro_votes.saturating_add(1);
            if self.intro_votes >= 2 {
                self.intro_seen = true;
            }
        } else if matches!(observation.page, Page::InMatch | Page::Settlement) {
            self.intro_votes = 0;
        } else {
            self.intro_votes = self.intro_votes.saturating_sub(1);
        }
        if observation.page == Page::LegendSelect {
            self.settlement_frames.clear();
            self.last_settlement_result = None;
            self.hud_confirmations = 0;
            self.status.stage = AutoStage::LegendSelect;
            self.status.phase = "已识别选传奇阶段，进入局内后采样昵称".into();
            self.status.roster = "本场名单：等待局内采样".into();
            return Ok(());
        }
        if observation.page == Page::Loading {
            self.settlement_frames.clear();
            self.last_settlement_result = None;
            self.hud_confirmations = 0;
            self.status.stage = AutoStage::Loading;
            self.status.phase = "新一局正在加载，等待选传奇界面".into();
            return Ok(());
        }
        if observation.page == Page::InMatch {
            // Once a summary was observed, a stray HUD read cannot send this
            // round backwards, discard its pending summary, or unlock a save.
            if self.settlement_seen && !self.intro_seen {
                self.hud_confirmations = 0;
                return Ok(());
            }
            self.settlement_frames.clear();
            self.last_settlement_result = None;
            if let Some(token) = observation.token.as_ref() {
                self.token_votes = if self.last_token.as_ref() == Some(token) {
                    self.token_votes.saturating_add(1)
                } else {
                    1
                };
                self.last_token = Some(token.clone());
            } else {
                self.last_token = None;
                self.token_votes = 0;
            }
            if self.active
                && self.session_token.is_some()
                && observation.token.is_some()
                && observation.token != self.session_token
                && self.token_votes >= 3
            {
                self.forget_live_round("检测到新对局标记，重新采集本场名单");
            }
            if self.locked {
                if !self.intro_seen {
                    self.status.stage = AutoStage::Saved;
                    self.status.phase = "本局已处理，等待下一局".into();
                    return Ok(());
                }
                self.locked = false;
                self.hud_confirmations = 0;
            }
            self.status.stage = AutoStage::InMatch;
            if !self.active {
                self.hud_confirmations = self.hud_confirmations.saturating_add(1);
                if self.hud_confirmations < 2 {
                    self.status.phase = "发现局内画面，正在确认…".into();
                    return Ok(());
                }
                self.active = true;
                self.session_token = (self.token_votes >= 2)
                    .then(|| observation.token.clone())
                    .flatten();
                self.roster = None;
                self.roster_readings.clear();
                self.roster_attempts = 0;
                self.roster_image = None;
                self.intro_seen = false;
                self.intro_reset_done = false;
            }
            if self.session_token.is_none() && self.token_votes >= 2 {
                self.session_token = observation.token.clone();
            }
            if self.roster_sampling_open() {
                self.collect_roster(image, &mut detector.names)?;
            } else {
                self.status.phase = "局内昵称单次识别完成，对局进行中，等待结算".into();
            }
            let _ = observation.squads;
            return Ok(());
        }

        self.hud_confirmations = 0;
        if observation.page != Page::Settlement {
            self.settlement_frames.clear();
            self.last_settlement_result = None;
            return Ok(());
        }
        self.settlement_seen = true;
        self.status.stage = AutoStage::Settlement;
        if !self.active || self.locked {
            if self.locked {
                self.status.stage = AutoStage::Saved;
            }
            self.status.phase = if self.locked {
                "本局已处理，等待下一局".into()
            } else {
                "已看到结算，但未观察到本局入局；可手动识别保存".into()
            };
            return Ok(());
        }
        if self.session_token.is_some()
            && observation.token.is_some()
            && self.session_token != observation.token
        {
            self.status.phase = "结算与局内标记不一致，请核对当前画面".into();
            self.status.error = "未使用旧名单关联不同对局。".into();
            self.settlement_frames.clear();
            self.last_settlement_result = None;
            return Ok(());
        }
        let previous_frame = self.settlement_trace_frame;
        self.settlement_trace_frame = self.trace_frame;
        self.settlement_frames.push(image.clone());
        if self.settlement_frames.len() > 2 {
            self.settlement_frames.remove(0);
        }
        if self.settlement_frames.len() < 2 {
            self.status.phase = "发现结算，正在确认关键数字…".into();
            return Ok(());
        }
        self.status.stage = AutoStage::Saving;
        self.status.phase = "结算已稳定，正在解析并保存…".into();
        let last = self.settlement_frames.len() - 1;
        let previous = match self.last_settlement_result.as_ref() {
            Some(previous) => previous.clone(),
            None => crate::settlement::recognize_image_numbers_selected(
                &self.settlement_frames[last - 1],
                &detector.root,
                &mut detector.names,
                self.trace
                    .as_ref()
                    .filter(|log| log.enabled())
                    .map(|log| {
                        log.context(
                            &self.trace_round,
                            "03_settlement_previous",
                            previous_frame,
                            1,
                        )
                    })
                    .as_ref(),
            )?,
        };
        let current_trace = self.trace_context("03_settlement_current", 2);
        let current = crate::settlement::recognize_image_numbers_selected(
            &self.settlement_frames[last],
            &detector.root,
            &mut detector.names,
            current_trace.as_ref(),
        )?;
        self.last_settlement_result = Some(current.clone());
        let mut result = corroborate(current, &previous);
        if let Some(trace) = &current_trace {
            trace.event("settlement_corroborated", serde_json::json!({"previous_nickname_hints":previous.players.iter().map(|p|&p.nickname_hint).collect::<Vec<_>>(),"result":result}));
        }
        if !result
            .rank
            .value
            .is_some_and(|rank| (1..=20).contains(&rank))
            || result.squad_kills.value.is_none()
        {
            self.status.stage = AutoStage::Settlement;
            self.status.phase = "结算关键数字尚未稳定；继续等待下一帧".into();
            return Ok(());
        }
        if self.retry_missing_damage(&mut result) {
            return Ok(());
        }
        // A long capture gap is not proof of a new match. Keep the round so
        // its numbers can still be saved, but only reuse identity evidence
        // after a matching session token proves continuity.
        let roster = self.roster.as_ref().filter(|_| !self.continuity_uncertain);
        if roster.is_some_and(|roster| roster.names.iter().any(|name| !name.is_empty())) {
            crate::settlement::fill_nickname_hints(
                &self.settlement_frames[last],
                &mut detector.names,
                &mut result,
                current_trace.as_ref(),
            );
        }
        let annotations = associate(roster, &result);
        if let Some(trace) = &current_trace {
            trace.event("final_association",serde_json::json!({"identity_policy":"02_hud_single_read_only","settlement_text_is_hint_only":true,"annotations":annotations,"continuity_uncertain":self.continuity_uncertain,"roster_used":roster.map(|r|&r.names)}));
        }
        let settlement_png = crate::imaging::png(&self.settlement_frames[last])?;
        let selection_png = roster
            .and_then(|roster| roster.image.as_ref())
            .map(|image| crate::imaging::png(image))
            .transpose()?;
        let pending_dir = write_pending(
            storage.data_dir(),
            &settlement_png,
            selection_png.as_deref(),
            &result,
            &annotations,
        )?;
        let saved = storage.save_settlement_capture(
            &result,
            &settlement_png,
            &annotations,
            selection_png.as_deref(),
        )?;
        self.auto_next_request = next_match_id(&saved, &result);
        self.auto_next_notice = if self.auto_next_request.is_none() && !saved.deduplicated {
            Some("战绩已保存，部分数字待核对，请手动继续".into())
        } else {
            None
        };
        if let Some(trace) = &current_trace {
            trace.event(
                "match_saved",
                serde_json::json!({"saved":saved,"annotations":annotations}),
            );
        }
        if let Err(error) = clear_pending(&pending_dir) {
            self.status.error = format!("战绩已保存，待处理文件清理稍后重试：{error}");
        }
        self.locked = true;
        self.active = false;
        self.session_token = None;
        self.settlement_frames.clear();
        self.last_settlement_result = None;
        self.status.phase = if result
            .players
            .iter()
            .any(|player| player.damage.value.is_none())
        {
            "战绩已保存；伤害重试后仍未确认，请查看结算图核对".into()
        } else {
            "战绩已保存，等待下一局".into()
        };
        self.status.stage = AutoStage::Saved;
        self.status.last_saved = format!("最近保存：{}", saved.saved_at);
        self.status.saved_count = storage.status()?.record_count;
        self.status.roster = "本场名单：".to_string()
            + &roster
                .map(|roster| {
                    roster
                        .names
                        .iter()
                        .map(|name| if name.is_empty() { "待核对" } else { name })
                        .collect::<Vec<_>>()
                        .join(" / ")
                })
                .unwrap_or_else(|| "未采集".into());
        Ok(())
    }

    fn retry_missing_damage(&mut self, result: &mut SettlementResult) -> bool {
        let missing = result
            .players
            .iter()
            .filter(|p| p.damage.value.is_none())
            .count();
        if missing == 0 {
            return false;
        }
        let retry = self.damage_retries < DAMAGE_RETRY_LIMIT;
        if retry {
            self.damage_retries += 1;
            self.status.stage = AutoStage::Settlement;
            self.status.phase = format!(
                "{missing} 项伤害未确认，等待新画面重试（{}/{}）",
                self.damage_retries, DAMAGE_RETRY_LIMIT
            );
        } else {
            result
                .warnings
                .push("伤害经 3 张新帧复核仍未确认，保留未知，请对照结算图核对；未补成 0。".into());
        }
        if let Some(trace) = self.trace_context("03_damage_retry", self.damage_retries) {
            trace.event(
                "damage_retry",
                serde_json::json!({
                    "retry":retry,"retries":self.damage_retries,"limit":DAMAGE_RETRY_LIMIT,
                    "missing":missing,"result":result
                }),
            );
        }
        retry
    }

    fn collect_roster(
        &mut self,
        image: &Arc<RgbaImage>,
        reader: &mut crate::nickname_ocr::NicknameRecognizer,
    ) -> Result<()> {
        if !self.roster_sampling_open() {
            return Ok(());
        }
        // Spend the attempt before OCR. Empty/error outcomes never trigger a retry.
        self.roster_attempts += 1;
        self.roster_image = Some(image.clone());
        let trace = self.trace_context("02_hud", self.roster_attempts);
        if let Some(trace) = &trace {
            trace.event("sample_started",serde_json::json!({"width":image.width(),"height":image.height(),"sample_limit":HUD_SAMPLE_LIMIT,"rule":"single_read_no_vote"}));
        }
        match crate::ocr::roster_names_selected(image, reader, trace.as_ref()) {
            Ok(readings) => self.roster_readings = readings,
            Err(error) => {
                self.roster_readings = vec![String::new(); 3];
                self.finalize_roster();
                self.status.phase = "局内昵称单次识别失败，保留未知，等待结算".into();
                return Err(error);
            }
        }
        self.finalize_roster();
        self.status.phase = "局内昵称单次识别完成，对局进行中，等待结算".into();
        Ok(())
    }

    fn finalize_roster(&mut self) {
        let (names, local_slot) = nickname::roster(&self.roster_readings);
        if let Some(trace) = self.trace_context("02_hud", self.roster_attempts) {
            let mut report = nickname::audit_roster(&self.roster_readings);
            report["attempts"] = serde_json::json!(self.roster_attempts);
            report["sampling_complete"] =
                serde_json::json!(self.roster_attempts >= HUD_SAMPLE_LIMIT);
            trace.event("hud_result", report);
        }
        self.status.roster = format!(
            "本场名单（单次识别完成）：{}",
            names
                .iter()
                .map(|name| if name.is_empty() { "待核对" } else { name })
                .collect::<Vec<_>>()
                .join(" / ")
        );
        self.roster = Some(RosterSnapshot {
            names,
            local_slot,
            image: self.roster_image.clone(),
        });
    }

    fn roster_sampling_open(&self) -> bool {
        self.active && !self.settlement_seen && self.roster_attempts < HUD_SAMPLE_LIMIT
    }

    fn capture_interval_ms(&self) -> u64 {
        // A single HUD read uses the confirming frame; only settlement needs a burst.
        if !self.locked && !self.settlement_frames.is_empty() {
            1000
        } else {
            2000
        }
    }

    fn begin_new_round(&mut self) {
        self.auto_next_request = None;
        self.auto_next_notice = None;
        if self.active && self.settlement_seen && !self.locked {
            if let Some(trace) = self.trace_context("lifecycle", 0) {
                trace.event("unresolved_settlement_left",serde_json::json!({
                    "reason":"new_loading_or_selection","last_result":self.last_settlement_result,
                    "retained_frames":self.settlement_frames.len(),"saved":false
                }));
            }
        }
        self.damage_retries = 0;
        if let Some(trace) = self.trace_context("lifecycle", 0) {
            trace.event("round_ended",serde_json::json!({"reason":"new_loading_or_selection","stage":self.status.stage,"roster":self.status.roster}));
        }
        if let Some(debug) = &self.trace {
            self.trace_round = debug.round("auto");
        }
        self.settlement_seen = false;
        self.continuity_uncertain = false;
        self.active = false;
        self.session_token = None;
        self.roster = None;
        self.roster_readings.clear();
        self.roster_image = None;
        self.roster_attempts = 0;
        self.settlement_frames.clear();
        self.last_settlement_result = None;
        self.hud_confirmations = 0;
        self.token_votes = 0;
        self.last_token = None;
        self.status.roster = "本场名单：等待局内采样".into();
        self.status.stage = AutoStage::Loading;
        self.status.phase = "发现新一局加载画面，等待进入局内".into();
    }

    fn forget_live_round(&mut self, phase: &str) {
        self.auto_next_request = None;
        self.auto_next_notice = None;
        self.damage_retries = 0;
        if let Some(trace) = self.trace_context("lifecycle", 0) {
            trace.event(
                "round_ended",
                serde_json::json!({"reason":phase,"roster":self.status.roster}),
            );
        }
        if let Some(debug) = &self.trace {
            self.trace_round = debug.round("auto");
        }
        self.settlement_seen = false;
        self.continuity_uncertain = false;
        self.active = false;
        self.session_token = None;
        self.roster = None;
        self.roster_readings.clear();
        self.roster_image = None;
        self.roster_attempts = 0;
        self.settlement_frames.clear();
        self.last_settlement_result = None;
        self.hud_confirmations = 0;
        self.intro_seen = false;
        self.intro_votes = 0;
        self.status.roster = "本场名单：未采集".into();
        self.status.stage = AutoStage::Waiting;
        self.status.phase = phase.into();
    }

    fn on_capture_gap(&mut self, elapsed: Duration, reason: &str) {
        if elapsed <= Duration::from_secs(20) {
            return;
        }
        if let Some(trace) = self.trace_context("capture_pause", 0) {
            trace.event("capture_paused",serde_json::json!({"elapsed_ms":elapsed.as_millis(),"reason":reason,"stage":self.status.stage,"active":self.active,"locked":self.locked}));
        }
        // Foreground loss, a static desktop or a DXGI interruption is a
        // capture condition, not a match boundary. Preserve the save lock,
        // stage, match eligibility and evidence; restart two-frame validation.
        self.continuity_uncertain |= self.active;
        self.settlement_frames.clear();
        self.last_settlement_result = None;
        self.hud_confirmations = 0;
        self.token_votes = 0;
        self.last_token = None;
        self.status.pause_reason = format!("暂未取得新游戏画面，已保留本局进度。{reason}");
    }
}

fn next_match_id(
    saved: &crate::storage::SaveMatchResult,
    result: &SettlementResult,
) -> Option<String> {
    (!saved.deduplicated
        && result.rank.value.is_some_and(|n| (1..=20).contains(&n))
        && result.squad_kills.value.is_some_and(|n| n >= 0)
        && result.players.len() == 3
        && result.players.iter().all(|p| {
            [&p.kills, &p.assists, &p.knocks, &p.damage]
                .iter()
                .all(|f| f.value.is_some_and(|n| n >= 0))
        }))
    .then(|| saved.id.clone())
}

fn observation_time_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis())
}

#[derive(Deserialize, Serialize)]
struct PendingJob {
    id: String,
    settlement_sha256: String,
    selection_sha256: Option<String>,
    settlement: SettlementResult,
    annotations: Vec<PlayerAnnotation>,
}

fn write_pending(
    data_dir: &Path,
    settlement_png: &[u8],
    selection_png: Option<&[u8]>,
    settlement: &SettlementResult,
    annotations: &[PlayerAnnotation],
) -> Result<std::path::PathBuf> {
    ensure!(
        !settlement_png.is_empty(),
        "结算截图为空，不能创建待保存任务"
    );
    ensure!(
        selection_png.is_none_or(|image| !image.is_empty()),
        "局内昵称截图为空，不能创建待保存任务"
    );
    let id = image_hash(settlement_png);
    let root = data_dir.join("pending");
    std::fs::create_dir_all(&root)?;
    let directory = root.join(&id);
    if !directory.exists() {
        let pending_count = std::fs::read_dir(&root)?
            .filter_map(std::result::Result::ok)
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
            .count();
        ensure!(
            pending_count < 20,
            "待保存战绩达到 20 场上限，请先处理本地数据问题"
        );
    }
    std::fs::create_dir_all(&directory)?;
    std::fs::write(directory.join("settlement.png"), settlement_png)?;
    let selection_sha256 = if let Some(image) = selection_png {
        std::fs::write(directory.join("selection.png"), image)?;
        Some(image_hash(image))
    } else {
        None
    };
    let job = PendingJob {
        id,
        settlement_sha256: image_hash(settlement_png),
        selection_sha256,
        settlement: settlement.clone(),
        annotations: annotations.to_vec(),
    };
    let manifest = directory.join("job.json");
    let temporary = directory.join(format!("job.{}.tmp", observation_time_ms()));
    std::fs::write(&temporary, serde_json::to_vec(&job)?)?;
    if manifest.exists() {
        std::fs::remove_file(&manifest)?;
    }
    std::fs::rename(&temporary, &manifest)?;
    Ok(directory)
}

fn recover_pending(storage: &LocalStore, status: &mut AutoRecordStatus) {
    let root = storage.data_dir().join("pending");
    if !root.is_dir() {
        return;
    }
    let entries = match std::fs::read_dir(&root) {
        Ok(entries) => entries,
        Err(error) => {
            status.error = format!("无法读取待保存战绩：{error}");
            return;
        }
    };
    let mut saved = 0_i64;
    let mut failures = Vec::new();
    for entry in entries.flatten() {
        let kind = match entry.file_type() {
            Ok(kind) => kind,
            Err(_) => continue,
        };
        if !kind.is_dir() || kind.is_symlink() {
            continue;
        }
        let directory = entry.path();
        let valid_name = entry.file_name().to_str().is_some_and(|name| {
            name.len() == 64 && name.bytes().all(|byte| byte.is_ascii_hexdigit())
        });
        if !valid_name {
            continue;
        }
        match recover_pending_job(storage, &directory) {
            Ok(was_saved) => saved += i64::from(was_saved),
            Err(error) => failures.push(format!(
                "{}：{error:#}",
                entry.file_name().to_string_lossy()
            )),
        }
    }
    if saved > 0 {
        status.saved_count = storage
            .status()
            .map_or(status.saved_count, |result| result.record_count);
        status.last_saved = format!("已恢复并保存 {saved} 场待保存战绩");
        status.phase = "待保存战绩已恢复".into();
    }
    if !failures.is_empty() {
        status.error = format!(
            "有 {} 场战绩待处理：{}",
            failures.len(),
            failures.join("；")
        );
    }
}

fn recover_pending_job(storage: &LocalStore, directory: &Path) -> Result<bool> {
    let id = directory
        .file_name()
        .and_then(|name| name.to_str())
        .context("待保存任务目录名无效")?;
    ensure!(
        id.len() == 64 && id.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "待保存任务编号无效"
    );
    let manifest = directory.join("job.json");
    ensure!(manifest.is_file(), "待保存任务清单缺失");
    ensure!(
        std::fs::metadata(&manifest)?.len() <= 4 * 1024 * 1024,
        "待保存任务清单过大"
    );
    let job: PendingJob = serde_json::from_slice(&std::fs::read(&manifest)?)?;
    ensure!(job.id == id, "待保存任务编号与目录不一致");
    let settlement_png = std::fs::read(directory.join("settlement.png"))?;
    ensure!(
        image_hash(&settlement_png) == job.settlement_sha256 && job.settlement_sha256 == id,
        "待保存结算截图校验失败"
    );
    let selection_png = match job.selection_sha256.as_deref() {
        Some(expected) => {
            let bytes = std::fs::read(directory.join("selection.png"))?;
            ensure!(image_hash(&bytes) == expected, "待保存局内截图校验失败");
            Some(bytes)
        }
        None => None,
    };
    storage.save_settlement_capture(
        &job.settlement,
        &settlement_png,
        &job.annotations,
        selection_png.as_deref(),
    )?;
    clear_pending(directory)?;
    Ok(true)
}

fn clear_pending(directory: &Path) -> Result<()> {
    for name in ["job.json", "settlement.png", "selection.png"] {
        let file = directory.join(name);
        if file.exists() {
            std::fs::remove_file(file)?;
        }
    }
    std::fs::remove_dir(directory)?;
    Ok(())
}

fn image_hash(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn corroborate(mut current: SettlementResult, previous: &SettlementResult) -> SettlementResult {
    fn check(current: &mut NumberField, previous: &NumberField) {
        if previous.value.is_some() && current.value != previous.value {
            current.value = None;
            current.confidence = 0.0;
        }
    }
    check(&mut current.rank, &previous.rank);
    check(&mut current.squad_kills, &previous.squad_kills);
    for (current, previous) in current.players.iter_mut().zip(&previous.players) {
        check(&mut current.kills, &previous.kills);
        check(&mut current.assists, &previous.assists);
        check(&mut current.knocks, &previous.knocks);
        check(&mut current.damage, &previous.damage);
        // A failed preceding read is not corroboration, including for zero.
        if previous.damage.value.is_none() {
            current.damage.value = None;
            current.damage.confidence = 0.0;
        }
    }
    current
}

#[derive(Clone)]
struct Assignment {
    members: [Option<usize>; 3],
    score: f64,
}

fn associate(
    roster: Option<&RosterSnapshot>,
    settlement: &SettlementResult,
) -> Vec<PlayerAnnotation> {
    let Some(roster) = roster.filter(|roster| roster.names.len() == 3) else {
        return settlement
            .players
            .iter()
            .map(|_player| PlayerAnnotation {
                nickname: None,
                name_confirmed: false,
                is_local: None,
                nickname_source: NicknameSource::Unresolved,
            })
            .collect();
    };
    let scores: [[f64; 3]; 3] = std::array::from_fn(|row| {
        std::array::from_fn(|column| {
            settlement.players[row]
                .nickname_hint
                .as_deref()
                .map_or(0.0, |hint| {
                    settlement_hint_score(hint, &roster.names[column])
                })
        })
    });
    fn enumerate(
        row: usize,
        used: u8,
        members: &mut [Option<usize>; 3],
        score: f64,
        scores: &[[f64; 3]; 3],
        output: &mut Vec<Assignment>,
    ) {
        if row == 3 {
            output.push(Assignment {
                members: *members,
                score,
            });
            return;
        }
        members[row] = None;
        enumerate(row + 1, used, members, score, scores, output);
        for member in 0..3 {
            if used & (1 << member) == 0 && scores[row][member] >= 0.82 {
                members[row] = Some(member);
                enumerate(
                    row + 1,
                    used | (1 << member),
                    members,
                    score + scores[row][member],
                    scores,
                    output,
                );
            }
        }
        members[row] = None;
    }
    let mut assignments = Vec::new();
    enumerate(0, 0, &mut [None; 3], 0.0, &scores, &mut assignments);
    assignments.sort_by(|left, right| right.score.total_cmp(&left.score));
    let best = assignments.first().cloned().unwrap_or(Assignment {
        members: [None; 3],
        score: 0.0,
    });
    let mut linked = best.members;
    for row in 0..3 {
        let alternative = assignments
            .iter()
            .filter(|assignment| assignment.members[row] != best.members[row])
            .map(|assignment| assignment.score)
            .fold(0.0_f64, f64::max);
        if best.score - alternative < 0.15 {
            linked[row] = None;
        }
    }
    if linked.iter().flatten().count() < 2 {
        linked = [None; 3];
    }
    settlement
        .players
        .iter()
        .enumerate()
        .map(|(slot, player)| {
            if let Some(member_slot) = linked[slot] {
                let nickname = roster.names[member_slot].clone();
                let exact = player
                    .nickname_hint
                    .as_deref()
                    .is_some_and(|name| canonical(name) == canonical(&nickname));
                PlayerAnnotation {
                    nickname: Some(nickname),
                    nickname_source: NicknameSource::VotedRoster,
                    name_confirmed: exact,
                    is_local: roster
                        .local_slot
                        .map(|local_slot| member_slot == local_slot),
                }
            } else {
                PlayerAnnotation {
                    nickname: None,
                    nickname_source: NicknameSource::Unresolved,
                    name_confirmed: false,
                    is_local: None,
                }
            }
        })
        .collect()
}

fn canonical(value: &str) -> String {
    value
        .chars()
        .filter(|character| !character.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect()
}

// A summary can append banner words (e.g. "vjk0ok 0 DANGER"). Compare
// bounded leading word groups against the trusted roster; never copy any of
// these prefixes into an identity. Fully hidden/ambiguous cards stay unmatched.
fn settlement_hint_score(hint: &str, trusted: &str) -> f64 {
    if trusted.trim().is_empty() {
        return 0.0;
    }
    let mut best = similarity(hint, trusted);
    if canonical(trusted).chars().count() < 4 {
        return best;
    }
    let mut prefix = String::new();
    for word in hint.split_whitespace().take(6) {
        if !prefix.is_empty() {
            prefix.push(' ');
        }
        prefix.push_str(word);
        if prefix.chars().count() > 80 {
            break;
        }
        best = best.max(similarity(&prefix, trusted));
    }
    best
}

pub(crate) fn similarity(observed: &str, target: &str) -> f64 {
    let left = canonical(observed);
    let right = canonical(target);
    if left.is_empty() || right.is_empty() {
        return 0.0;
    }
    if left == right {
        return 1.0;
    }
    let a: Vec<char> = left.chars().collect();
    let b: Vec<char> = right.chars().collect();
    if a.len() < 4 || b.len() < 4 || a.len() > 64 || b.len() > 64 {
        return 0.0;
    }
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    let mut current = vec![0; b.len() + 1];
    for i in 1..=a.len() {
        current[0] = i;
        for j in 1..=b.len() {
            current[j] = (previous[j] + 1)
                .min(current[j - 1] + 1)
                .min(previous[j - 1] + usize::from(a[i - 1] != b[j - 1]));
        }
        std::mem::swap(&mut previous, &mut current);
    }
    1.0 - previous[b.len()] as f64 / a.len().max(b.len()) as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn automatic_next_match_requires_complete_newly_saved_numbers() {
        let field = NumberField {
            raw_text: "0".into(),
            value: Some(0),
            confidence: 1.0,
        };
        let mut result = SettlementResult {
            rank: NumberField {
                value: Some(12),
                ..field.clone()
            },
            squad_kills: field,
            players: vec![player("a"), player("b"), player("c")],
            warnings: vec![],
        };
        let mut saved = crate::storage::SaveMatchResult {
            id: "test".into(),
            saved_at: String::new(),
            deduplicated: false,
        };
        assert_eq!(next_match_id(&saved, &result), Some("test".into())); // legitimate zero is complete
        saved.deduplicated = true;
        assert_eq!(next_match_id(&saved, &result), None);
        saved.deduplicated = false;
        result.players[0].damage.value = None;
        assert_eq!(next_match_id(&saved, &result), None);
        result.players[0].damage.value = Some(74);
        result.rank.value = None;
        assert_eq!(next_match_id(&saved, &result), None);
    }

    #[test]
    fn supplied_summary_has_the_rematch_prompt_without_sending_input() -> Result<()> {
        let project = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let image = crate::imaging::load(&project.join("image_for_test/异常识别问题1.png"))?;
        let detector = PageDetector::new(&crate::resources()?)?;
        let prompt = detector.read_text(&image, (0.730, 0.0, 0.270, 0.105))?;
        assert!(crate::auto_next::rematch_prompt(&prompt), "prompt={prompt}");
        Ok(())
    }

    #[test]
    fn unsaved_summary_does_not_block_next_loading_or_reuse_old_roster() -> Result<()> {
        let project = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let image = Arc::new(crate::imaging::load(
            &project.join("image_for_test/异常识别问题2-结算后进入下一局.jpg"),
        )?);
        let mut detector = PageDetector::new(&crate::resources()?)?;
        assert!(matches!(
            detector.observe(&image, true)?.page,
            Page::Loading
        ));
        let directory = project.join(format!(
            ".local/storage-tests/unfinished-summary-{}",
            observation_time_ms()
        ));
        let storage = LocalStore::open(directory.clone())?;
        let mut recorder = Recorder::default();
        // Synthetic lifecycle context, not same-match evidence from two images.
        recorder.active = true;
        recorder.settlement_seen = true;
        recorder.status.stage = AutoStage::Settlement;
        recorder.damage_retries = DAMAGE_RETRY_LIMIT;
        recorder.roster = Some(RosterSnapshot {
            names: vec!["old-a".into(), "old-b".into(), "old-c".into()],
            local_slot: Some(2),
            image: None,
        });
        recorder.process(&image, &mut detector, &storage)?;
        assert_eq!(recorder.status.stage, AutoStage::Loading);
        assert!(!recorder.active);
        assert!(recorder.roster.is_none());
        assert_eq!(recorder.roster_attempts, 0);
        assert_eq!(recorder.damage_retries, 0);
        assert_eq!(recorder.capture_interval_ms(), 2000);
        assert_eq!(storage.status()?.record_count, 0);
        drop(storage);
        std::fs::remove_dir_all(directory)?;
        Ok(())
    }

    #[test]
    fn damage_retry_requires_confirmation_accepts_zero_and_stops_at_limit() {
        let mut parsed = SettlementResult {
            rank: NumberField {
                raw_text: "12".into(),
                value: Some(12),
                confidence: 0.9,
            },
            squad_kills: NumberField {
                raw_text: "1".into(),
                value: Some(1),
                confidence: 0.9,
            },
            players: vec![player("a"), player("b"), player("c")],
            warnings: Vec::new(),
        };
        let mut recorder = Recorder::default();
        assert!(!recorder.retry_missing_damage(&mut parsed)); // true zero is valid
        let mut failed = parsed.clone();
        failed.players[0].damage.value = None;
        let mut unconfirmed = corroborate(parsed.clone(), &failed);
        assert_eq!(unconfirmed.players[0].damage.value, None);
        recorder
            .settlement_frames
            .push(Arc::new(RgbaImage::new(1, 1)));
        for expected in 1..=DAMAGE_RETRY_LIMIT {
            assert!(recorder.retry_missing_damage(&mut unconfirmed));
            assert_eq!(recorder.damage_retries, expected);
            assert_eq!(recorder.status.stage, AutoStage::Settlement);
            assert_eq!(recorder.capture_interval_ms(), 1000);
        }
        // A capture interruption does not replenish the per-round budget.
        recorder.on_capture_gap(Duration::from_secs(21), "test");
        assert!(!recorder.retry_missing_damage(&mut unconfirmed));
        assert_eq!(unconfirmed.players[0].damage.value, None);
        assert!(unconfirmed
            .warnings
            .iter()
            .any(|warning| warning.contains("未补成 0")));
        assert!(!recorder.retry_missing_damage(&mut corroborate(parsed.clone(), &parsed)));
        let mut conflict = parsed.clone();
        conflict.players[0].damage.value = Some(74);
        assert_eq!(
            corroborate(conflict.clone(), &parsed).players[0]
                .damage
                .value,
            None
        );
        assert_eq!(
            corroborate(conflict.clone(), &conflict).players[0]
                .damage
                .value,
            Some(74)
        );
        recorder.begin_new_round();
        assert_eq!(recorder.damage_retries, 0);
        assert!(recorder.retry_missing_damage(&mut failed));
    }

    #[test]
    fn running_worker_blocks_backend_change_before_loading_or_persisting() -> Result<()> {
        let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!(
            "../../.local/storage-tests/ocr-switch-{}",
            time::OffsetDateTime::now_utc().unix_timestamp_nanos()
        ));
        let settings = crate::ocr_settings::OcrSettings::open(&directory);
        let manager = AutoManager::default();
        let (release, wait) = std::sync::mpsc::channel::<()>();
        *manager.worker.lock().unwrap() = Some(Worker {
            stop: Arc::new(AtomicBool::new(false)),
            join: thread::spawn(move || {
                let _ = wait.recv();
            }),
        });
        let result = manager.set_ocr_backend(
            &settings,
            crate::ocr_settings::OcrBackend::WindowsTesseract,
            Path::new("missing-models"),
        );
        release.send(())?;
        let worker = manager.worker.lock().unwrap().take().unwrap();
        worker.join.join().unwrap();
        assert!(format!("{:#}", result.unwrap_err()).contains("请先停止自动记录"));
        assert_eq!(
            settings.backend(),
            crate::ocr_settings::OcrBackend::PpOcrV6Small
        );
        assert!(!directory.exists());
        // Idle initialization failure also leaves the saved choice untouched.
        assert!(manager
            .set_ocr_backend(
                &settings,
                crate::ocr_settings::OcrBackend::PpOcrV6Small,
                Path::new("missing-models")
            )
            .is_err());
        assert!(!directory.exists());
        Ok(())
    }

    #[test]
    fn supplied_rank9_settlement_is_not_a_live_hud() -> Result<()> {
        let project = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let resources = crate::resources()?;
        let image = crate::imaging::load(&project.join("image_for_test/2K/结算图-06.png"))?;
        let mut detector = PageDetector::new(&resources)?;
        let observed = detector.observe(&image, true)?;
        println!(
            "feedback page: {:?}; header: {:?}; HUD: {:?}",
            observed.page,
            detector.read_text(&image, (0.690, 0.135, 0.255, 0.085))?,
            detector.read_text(&image, (0.800, 0.035, 0.145, 0.060))?
        );
        assert_eq!(observed.page, Page::Settlement);
        let parsed = crate::settlement::recognize_image(&image, &resources)?;
        assert_eq!(parsed.rank.value, Some(9));
        assert_eq!(parsed.squad_kills.value, Some(5));
        Ok(())
    }

    #[test]
    fn regression_capture_gap_and_stray_hud_do_not_discard_or_reopen_summary() -> Result<()> {
        let project = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let resources = crate::resources()?;
        let summary = Arc::new(crate::imaging::load(
            &project.join("image_for_test/2K/结算图-06.png"),
        )?);
        let data_dir = project.join(format!(
            ".local/storage-tests/gap-{}",
            observation_time_ms()
        ));
        let storage = LocalStore::open(data_dir.clone())?;
        let mut detector = PageDetector::new(&resources)?;
        // Synthetic preceding match context; this is not a paired real-game
        // sample. Sentinel names must never leak into the feedback summary.
        let mut recorder = Recorder {
            active: true,
            roster: Some(RosterSnapshot {
                names: vec!["old-one".into(), "old-two".into(), "old-three".into()],
                local_slot: Some(1),
                image: Some(summary.clone()),
            }),
            ..Recorder::default()
        };
        recorder.status.stage = AutoStage::InMatch;
        recorder.status.roster = "本场名单：保留的证据".into();
        recorder.on_capture_gap(Duration::from_secs(21), "等待新帧");
        assert!(recorder.active);
        assert!(recorder.roster.is_some());
        assert_eq!(recorder.status.stage, AutoStage::InMatch);
        assert!(recorder.continuity_uncertain);

        recorder.process(&summary, &mut detector, &storage)?;
        assert_eq!(recorder.status.stage, AutoStage::Settlement);
        assert!(recorder.status.pause_reason.is_empty());
        assert_eq!(recorder.settlement_frames.len(), 1);
        for _ in 0..3 {
            recorder.process_observation(
                &summary,
                Observation {
                    page: Page::InMatch,
                    token: Some("deadbeef".into()),
                    squads: Some(8),
                    new_round_intro: false,
                },
                &mut detector,
                &storage,
            )?;
        }
        assert_eq!(recorder.status.stage, AutoStage::Settlement);
        assert_eq!(recorder.settlement_frames.len(), 1);
        assert!(recorder.active);

        recorder.on_capture_gap(Duration::from_secs(30), "窗口暂时不可采集");
        assert_eq!(recorder.status.stage, AutoStage::Settlement);
        assert!(recorder.active);
        assert!(recorder.settlement_frames.is_empty());
        recorder.process(&summary, &mut detector, &storage)?;
        assert_eq!(
            storage.list_matches()?.len(),
            0,
            "must await two fresh frames"
        );
        recorder.process(&summary, &mut detector, &storage)?;
        let records = storage.list_matches()?;
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].rank, Some(9));
        assert_eq!(records[0].squad_kills, Some(5));
        assert!(records[0]
            .players
            .iter()
            .all(|player| player.is_local.is_none() && !player.name_confirmed));
        assert!(records[0].players.iter().all(|player| !player
            .nickname
            .as_deref()
            .unwrap_or_default()
            .starts_with("old-")));
        assert!(!records[0].has_selection_image);
        assert!(recorder.locked);

        recorder.on_capture_gap(Duration::from_secs(60), "停留结算页");
        for _ in 0..3 {
            recorder.process_observation(
                &summary,
                Observation {
                    page: Page::InMatch,
                    token: Some("deadbeef".into()),
                    squads: Some(8),
                    new_round_intro: false,
                },
                &mut detector,
                &storage,
            )?;
        }
        recorder.process(&summary, &mut detector, &storage)?;
        assert_eq!(recorder.status.stage, AutoStage::Saved);
        assert!(recorder.locked);
        assert!(!recorder.active);
        assert_eq!(storage.list_matches()?.len(), 1);
        drop(storage);
        std::fs::remove_dir_all(data_dir)?;
        Ok(())
    }

    #[test]
    fn regression_gap_keeps_identity_only_when_session_continuity_is_proven() -> Result<()> {
        let project = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut detector = PageDetector::new(&crate::resources()?)?;
        let data_dir = project.join(format!(
            ".local/storage-tests/gap-token-{}",
            observation_time_ms()
        ));
        let storage = LocalStore::open(data_dir.clone())?;
        let image = Arc::new(RgbaImage::new(8, 8));
        let mut recorder = Recorder {
            active: true,
            session_token: Some("1234abcd".into()),
            ..Recorder::default()
        };
        recorder.on_capture_gap(Duration::from_secs(60), "切出");
        recorder.process_observation(
            &image,
            Observation {
                page: Page::Settlement,
                token: Some("bad0bad0".into()),
                squads: None,
                new_round_intro: false,
            },
            &mut detector,
            &storage,
        )?;
        assert!(recorder.continuity_uncertain);
        assert!(recorder.settlement_frames.is_empty());
        recorder.process_observation(
            &image,
            Observation {
                page: Page::Settlement,
                token: Some("1234abcd".into()),
                squads: None,
                new_round_intro: false,
            },
            &mut detector,
            &storage,
        )?;
        assert!(!recorder.continuity_uncertain);
        assert_eq!(recorder.settlement_frames.len(), 1);
        assert_eq!(storage.list_matches()?.len(), 0);
        drop(storage);
        std::fs::remove_dir_all(data_dir)?;
        Ok(())
    }

    #[test]
    fn status_events_preserve_saved_result_and_skip_unchanged_snapshots() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let received = events.clone();
        let manager = AutoManager::with_status_listener(move |status| {
            received.lock().unwrap().push(status);
        });
        let saved = AutoRecordStatus {
            running: true,
            phase: "战绩已保存，等待下一局".into(),
            last_saved: "最近保存：2026-10-31T08:30:00Z".into(),
            saved_count: 1,
            ..AutoRecordStatus::default()
        };
        manager.status.publish(saved.clone());
        manager.status.publish(saved.clone());
        assert_eq!(*events.lock().unwrap(), vec![saved.clone()]);
        assert_eq!(manager.status(), saved);
        manager.stop(&CaptureManager::default());
        let stopped = manager.status();
        assert!(!stopped.running);
        assert_eq!(stopped.saved_count, 1);
        assert_eq!(events.lock().unwrap().last(), Some(&stopped));
    }

    #[test]
    fn pending_capture_recovers_idempotently_with_both_images() -> Result<()> {
        let project = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let root = project.join(".local/storage-tests");
        std::fs::create_dir_all(&root)?;
        let nonce = observation_time_ms();
        let data_dir = root.join(format!("pending-{nonce}"));
        let storage = LocalStore::open(data_dir.clone())?;
        let settlement_image =
            crate::imaging::png(&RgbaImage::from_pixel(8, 8, image::Rgba([18, 32, 48, 255])))?;
        let selection_image =
            crate::imaging::png(&RgbaImage::from_pixel(8, 8, image::Rgba([52, 64, 76, 255])))?;
        let settlement = SettlementResult {
            rank: NumberField {
                raw_text: "2".into(),
                value: Some(2),
                confidence: 1.0,
            },
            squad_kills: NumberField {
                raw_text: "5".into(),
                value: Some(5),
                confidence: 1.0,
            },
            players: vec![player("Archer"), player("trust you"), player("焚梦玩家")],
            warnings: Vec::new(),
        };
        let annotations = vec![
            PlayerAnnotation {
                nickname: Some("Archer".into()),
                nickname_source: NicknameSource::VotedRoster,
                name_confirmed: true,
                is_local: Some(false),
            },
            PlayerAnnotation {
                nickname: Some("trust you".into()),
                nickname_source: NicknameSource::VotedRoster,
                name_confirmed: true,
                is_local: Some(false),
            },
            PlayerAnnotation {
                nickname: Some("焚梦玩家".into()),
                nickname_source: NicknameSource::VotedRoster,
                name_confirmed: true,
                is_local: Some(true),
            },
        ];
        let pending_dir = write_pending(
            storage.data_dir(),
            &settlement_image,
            Some(&selection_image),
            &settlement,
            &annotations,
        )?;
        let mut status = AutoRecordStatus::default();
        recover_pending(&storage, &mut status);
        assert_eq!(status.saved_count, 1);
        assert_eq!(storage.list_matches()?.len(), 1);
        assert!(!pending_dir.exists());
        recover_pending(&storage, &mut status);
        assert_eq!(storage.list_matches()?.len(), 1);
        assert!(storage.list_matches()?[0].has_selection_image);

        drop(storage);
        std::fs::remove_dir_all(data_dir)?;
        Ok(())
    }

    #[test]
    fn detects_the_four_reference_flow_pages() -> Result<()> {
        let project = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let resources = crate::resources()?;
        let mut detector = PageDetector::new(&resources)?;
        let samples = [
            (
                "00-开始加载-页面一个图片关键字是-三人赛.jpg",
                Page::Loading,
            ),
            (
                "01-开始选传奇页面-主要关注左侧靠上的昵称 跟底部的昵称进行定位.jpg",
                Page::LegendSelect,
            ),
            (
                "02-游戏中-主要靠两个点进行定位首先是右上角固定出现的剩余小队数量（这个数量是不固定的 不要写死哈）-左下角昵称.jpg",
                Page::InMatch,
            ),
            (
                "03-游戏结束结算界面-主要用小队排名跟小队总击杀进行定位.jpg",
                Page::Settlement,
            ),
        ];
        for (sample, expected) in samples {
            let image = crate::imaging::load(&project.join("image_for_test").join(sample))?;
            let observation = detector.observe(&image, true)?;
            assert_eq!(observation.page, expected, "{sample}");
            assert_eq!(
                observation.new_round_intro,
                matches!(expected, Page::Loading | Page::LegendSelect),
                "{sample}"
            );
            if expected == Page::InMatch {
                assert_eq!(observation.squads, Some(20));
            }
            if expected == Page::Settlement {
                let parsed = crate::settlement::recognize_image(&image, &resources)?;
                assert_eq!(parsed.rank.value, Some(12));
                assert_eq!(parsed.squad_kills.value, Some(2));
            }
        }
        assert_eq!(remaining_squads("7剩余小队数量59"), Some(7));
        Ok(())
    }

    #[test]
    fn derived_1080p_flow_pages_and_all_hud_layouts_are_classified() -> Result<()> {
        let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../image_for_test/1080p");
        let resources = crate::resources()?;
        let mut detector = PageDetector::new(&resources)?;
        let mut samples=vec![
            ("00-开始加载-页面一个图片关键字是-三人赛.png".to_string(),Page::Loading),
            ("01-开始选传奇页面-主要关注左侧靠上的昵称 跟底部的昵称进行定位.png".to_string(),Page::LegendSelect),
            ("02-游戏中-主要靠两个点进行定位首先是右上角固定出现的剩余小队数量（这个数量是不固定的 不要写死哈）-左下角昵称.png".to_string(),Page::InMatch),
            ("03-游戏结束结算界面-主要用小队排名跟小队总击杀进行定位.png".to_string(),Page::Settlement),
        ];
        samples.extend((1..=6).map(|number| (format!("局内-{number}.png"), Page::InMatch)));
        samples.extend((1..=7).map(|number| (format!("结算图-{number:02}.png"), Page::Settlement)));
        for (name, expected) in samples {
            let image = crate::imaging::load(&directory.join(&name))?;
            assert_eq!(image.dimensions(), (1920, 1080));
            let observed = detector.observe(&image, true)?;
            assert_eq!(observed.page, expected, "1080p page {name}");
            assert_eq!(
                observed.new_round_intro,
                matches!(expected, Page::Loading | Page::LegendSelect),
                "1080p intro {name}"
            );
            if expected == Page::InMatch {
                assert!(
                    observed.squads.is_some_and(|count| count > 0),
                    "1080p dynamic squads {name}"
                );
            }
        }
        Ok(())
    }

    #[test]
    fn derived_1080p_summaries_keep_rank_kda_damage_and_native_evidence() -> Result<()> {
        let project = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let samples = project.join("image_for_test/1080p");
        let resources = crate::resources()?;
        let mut reader = crate::nickname_ocr::NicknameRecognizer::new(
            &resources,
            crate::ocr_settings::OcrBackend::PpOcrV6Small,
        )?;
        let cases = [
            (
                "03-游戏结束结算界面-主要用小队排名跟小队总击杀进行定位.png",
                12,
                2,
                [[0, 0, 0, 174], [1, 0, 1, 158], [1, 0, 1, 321]],
            ),
            (
                "结算图-06.png",
                9,
                5,
                [[1, 0, 1, 340], [3, 1, 4, 664], [1, 3, 1, 411]],
            ),
        ];
        let directory = project.join(format!(
            ".local/storage-tests/1080p-{}",
            time::OffsetDateTime::now_utc().unix_timestamp_nanos()
        ));
        let store = LocalStore::open(directory.clone())?;
        for (name, rank, kills, expected) in cases {
            let image = crate::imaging::load(&samples.join(name))?;
            let parsed =
                crate::settlement::recognize_image_selected(&image, &resources, &mut reader, None)?;
            assert_eq!(parsed.rank.value, Some(rank), "rank {name}");
            assert!(parsed.rank.confidence >= 0.70, "rank confidence {name}");
            assert_eq!(parsed.squad_kills.value, Some(kills), "squad kills {name}");
            for (player, values) in parsed.players.iter().zip(expected) {
                assert_eq!(
                    [
                        player.kills.value,
                        player.assists.value,
                        player.knocks.value,
                        player.damage.value
                    ],
                    values.map(Some),
                    "numbers {name}"
                );
            }
            let bytes = crate::imaging::png(&image)?;
            // No cross-image identity is assumed. Validate numbers/evidence only.
            let annotations = vec![
                PlayerAnnotation {
                    nickname: None,
                    nickname_source: NicknameSource::Unresolved,
                    name_confirmed: false,
                    is_local: None
                };
                3
            ];
            let saved = store.save_settlement_capture(&parsed, &bytes, &annotations, None)?;
            assert!(
                store
                    .save_settlement_capture(&parsed, &bytes, &annotations, None)?
                    .deduplicated
            );
            let evidence = crate::imaging::load(&store.evidence_path(&saved.id, "settlement")?)?;
            assert_eq!(evidence.dimensions(), (1920, 1080));
            assert_eq!(crate::imaging::png(&evidence)?, bytes);
        }
        let records = store.list_matches()?;
        assert_eq!(records.len(), 2);
        assert!(records
            .iter()
            .flat_map(|record| &record.players)
            .all(|player| player.nickname.is_none()));
        drop(store);
        std::fs::remove_dir_all(directory)?;
        Ok(())
    }

    #[test]
    fn automatic_roster_reads_three_ppocr_regions_once_after_two_hud_frames() -> Result<()> {
        let project = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let resources = crate::resources()?;
        let mut detector =
            PageDetector::with_backend(&resources, crate::ocr_settings::OcrBackend::PpOcrV6Small)?;
        let image = Arc::new(crate::imaging::load(
            &project.join("image_for_test/1080p/局内-1.png"),
        )?);
        let data_dir = project.join(format!(
            ".local/storage-tests/single-hud-{}",
            observation_time_ms()
        ));
        let storage = LocalStore::open(data_dir.clone())?;
        let debug = Arc::new(crate::nickname_debug::NicknameDebug::new(&data_dir));
        let mut recorder = Recorder::default();
        recorder.trace_round = debug.round("single_read_test");
        recorder.trace = Some(debug.clone());
        recorder.process(&image, &mut detector, &storage)?;
        assert_eq!(recorder.roster_attempts, 0);
        assert!(recorder.roster.is_none());
        recorder.process(&image, &mut detector, &storage)?;
        assert_eq!(recorder.roster_attempts, 1);
        assert!(!recorder.roster_sampling_open());
        assert_eq!(recorder.capture_interval_ms(), 2000);
        let names = recorder.roster.as_ref().unwrap().names.clone();
        assert!(names.iter().all(|name| !name.is_empty()), "{names:?}");
        // Later frames cannot trigger OCR or change the single captured roster.
        let invalid = Arc::new(RgbaImage::new(8, 8));
        for _ in 0..6 {
            recorder.process_observation(
                &invalid,
                Observation {
                    page: Page::InMatch,
                    token: None,
                    squads: Some(8),
                    new_round_intro: false,
                },
                &mut detector,
                &storage,
            )?;
        }
        assert_eq!(recorder.roster_attempts, 1);
        assert_eq!(recorder.roster.as_ref().unwrap().names, names);
        assert!(Arc::ptr_eq(recorder.roster_image.as_ref().unwrap(), &image));
        let events = std::fs::read_to_string(debug.status().current_file.unwrap())?
            .lines()
            .map(serde_json::from_str::<serde_json::Value>)
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let hud_calls = events
            .iter()
            .filter(|e| {
                e["event"] == "ocr"
                    && e["data"]["stage"] == "02_hud"
                    && e["data"]["detail"]["engine"] == "ppocr_v6_small"
            })
            .count();
        assert_eq!(hud_calls, 3, "one PP-OCR call per player, not five rounds");
        assert_eq!(
            events
                .iter()
                .filter(|e| e["event"] == "sample_started")
                .count(),
            1
        );
        assert_eq!(
            events.iter().filter(|e| e["event"] == "hud_result").count(),
            1
        );
        assert!(!events.iter().any(|e| e["event"] == "hud_vote"));
        assert_eq!(storage.status()?.record_count, 0);
        drop(recorder);
        drop(debug);
        drop(storage);
        std::fs::remove_dir_all(data_dir)?;
        Ok(())
    }

    #[test]
    fn empty_or_failed_single_hud_read_is_not_retried() -> Result<()> {
        let resources = crate::resources()?;
        let mut reader = crate::nickname_ocr::NicknameRecognizer::new(
            &resources,
            crate::ocr_settings::OcrBackend::PpOcrV6Small,
        )?;
        for image in [RgbaImage::new(1920, 1080), RgbaImage::new(8, 8)] {
            let image = Arc::new(image);
            let mut recorder = Recorder::default();
            recorder.active = true;
            assert!(recorder.collect_roster(&image, &mut reader).is_err());
            assert_eq!(recorder.roster_attempts, 1);
            assert!(!recorder.roster_sampling_open());
            assert_eq!(recorder.capture_interval_ms(), 2000);
            assert_eq!(
                recorder.roster.as_ref().unwrap().names,
                vec![String::new(); 3]
            );
            assert_eq!(recorder.roster.as_ref().unwrap().local_slot, None);
            assert!(recorder.collect_roster(&image, &mut reader).is_ok());
            recorder.on_capture_gap(Duration::from_secs(21), "pause");
            assert!(recorder.collect_roster(&image, &mut reader).is_ok());
            assert_eq!(recorder.roster_attempts, 1);
        }
        Ok(())
    }

    #[test]
    fn known_in_match_and_settlement_pair_saves_once_with_both_evidence_images() -> Result<()> {
        let project = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let resources = crate::resources()?;
        let mut detector = PageDetector::new(&resources)?;
        let loading = Arc::new(crate::imaging::load(
            &project.join("image_for_test/00-开始加载-页面一个图片关键字是-三人赛.jpg"),
        )?);
        let selection = Arc::new(crate::imaging::load(&project.join(
            "image_for_test/01-开始选传奇页面-主要关注左侧靠上的昵称 跟底部的昵称进行定位.jpg",
        ))?);
        let in_match = Arc::new(crate::imaging::load(&project.join(
            "image_for_test/02-游戏中-主要靠两个点进行定位首先是右上角固定出现的剩余小队数量（这个数量是不固定的 不要写死哈）-左下角昵称.jpg",
        ))?);
        let settlement = Arc::new(crate::imaging::load(&project.join(
            "image_for_test/03-游戏结束结算界面-主要用小队排名跟小队总击杀进行定位.jpg",
        ))?);
        let root = project.join(".local/storage-tests");
        std::fs::create_dir_all(&root)?;
        let data_dir = root.join(format!("auto-pair-{}", observation_time_ms()));
        let storage = LocalStore::open(data_dir.clone())?;
        let mut recorder = Recorder::default();
        let debug = Arc::new(crate::nickname_debug::NicknameDebug::new(&data_dir));
        recorder.trace_round = debug.round("synthetic_auto_test");
        recorder.trace = Some(debug.clone());

        for _ in 0..2 {
            recorder.process(&loading, &mut detector, &storage)?;
        }
        for _ in 0..3 {
            recorder.process(&selection, &mut detector, &storage)?;
        }
        for _ in 0..2 {
            recorder.process(&in_match, &mut detector, &storage)?;
        }
        thread::sleep(Duration::from_millis(650));
        recorder.process(&in_match, &mut detector, &storage)?;
        for _ in 0..2 {
            recorder.process(&settlement, &mut detector, &storage)?;
        }
        let records = storage.list_matches()?;
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].rank, Some(12));
        assert_eq!(records[0].squad_kills, Some(2));
        assert!(records[0].has_settlement_image);
        assert!(records[0].has_selection_image);
        assert_eq!(recorder.status.saved_count, 1);
        assert_eq!(recorder.status.phase, "战绩已保存，等待下一局");
        assert_eq!(
            recorder.auto_next_request.take(),
            Some(records[0].id.clone())
        );

        for _ in 0..3 {
            recorder.process(&settlement, &mut detector, &storage)?;
        }
        assert_eq!(storage.list_matches()?.len(), 1);
        assert!(
            recorder.auto_next_request.is_none(),
            "repeated summary must not schedule another key press"
        );

        recorder.process(&loading, &mut detector, &storage)?;
        recorder.process(&selection, &mut detector, &storage)?;
        recorder.process(&selection, &mut detector, &storage)?;
        assert!(
            recorder.locked,
            "loading and legend screens alone must not start a match"
        );
        assert!(!recorder.active);
        assert_eq!(storage.status()?.record_count, 1);

        recorder.process(&in_match, &mut detector, &storage)?;
        assert!(
            !recorder.locked,
            "new in-match HUD should release the prior match lock"
        );
        assert!(
            !recorder.active,
            "the new HUD still requires two confirming frames"
        );
        recorder.process(&in_match, &mut detector, &storage)?;
        assert!(
            recorder.active,
            "the next match should begin after HUD confirmation"
        );
        assert_eq!(storage.status()?.record_count, 1);

        let trace_file = debug.status().current_file.unwrap();
        let events: Vec<serde_json::Value> = std::fs::read_to_string(trace_file)?
            .lines()
            .map(serde_json::from_str)
            .collect::<std::result::Result<_, _>>()?;
        let raw: Vec<_> = events
            .iter()
            .filter(|entry| entry["event"] == "ocr")
            .collect();
        for (stage, source, engine) in [
            ("02_hud", "hud_lower_left", "windows"),
            ("03_settlement_current", "settlement_card", "windows"),
        ] {
            assert!(
                raw.iter().any(|entry| entry["data"]["stage"] == stage
                    && entry["data"]["detail"]["source"] == source
                    && entry["data"]["detail"]["engine"] == engine),
                "missing {stage}/{source}/{engine}"
            );
        }
        assert!(!raw
            .iter()
            .any(|entry| entry["data"]["stage"] == "01_selection"));
        assert!(events.iter().any(|entry| entry["event"] == "hud_result"));
        assert!(events
            .iter()
            .any(|entry| entry["event"] == "final_association"));
        assert!(events.iter().any(|entry| entry["event"] == "match_saved"
            && entry["data"]["detail"]["saved"]["id"] == records[0].id));
        drop(recorder);
        drop(debug);
        drop(storage);
        std::fs::remove_dir_all(data_dir)?;
        Ok(())
    }

    #[test]
    fn roster_matching_is_unique_and_requires_two_confident_names() {
        assert_eq!(clean_roster_name("摸 摸 我 的 狗 吧"), "摸摸我的狗吧");
        assert_eq!(clean_roster_name("trust you"), "trust you");
        let roster = RosterSnapshot {
            names: vec!["Archer".into(), "trust you".into(), "焚梦玩家".into()],
            local_slot: Some(2),
            image: None,
        };
        let settlement = SettlementResult {
            rank: NumberField {
                raw_text: "2".into(),
                value: Some(2),
                confidence: 0.9,
            },
            squad_kills: NumberField {
                raw_text: "5".into(),
                value: Some(5),
                confidence: 0.9,
            },
            players: vec![player("Archer"), player("trust y0u"), player("unreadable")],
            warnings: vec![],
        };
        let annotations = associate(Some(&roster), &settlement);
        assert_eq!(annotations[0].nickname.as_deref(), Some("Archer"));
        assert_eq!(annotations[0].name_confirmed, true);
        assert_eq!(annotations[1].nickname.as_deref(), Some("trust you"));
        assert_eq!(annotations[1].name_confirmed, false);
        assert_eq!(annotations[2].nickname, None);
        assert_eq!(annotations[2].is_local, None);

        let insufficient = SettlementResult {
            players: vec![player("Archer"), player("unknown"), player("unknown2")],
            ..settlement
        };
        let annotations = associate(Some(&roster), &insufficient);
        assert_eq!(annotations[0].is_local, None);
        assert_eq!(annotations[0].nickname, None);
    }

    #[test]
    fn summary_banner_text_cannot_replace_voted_names_or_move_their_stats() -> Result<()> {
        let roster = RosterSnapshot {
            names: vec!["vjkook".into(), "Archer".into(), "Teammate".into()],
            local_slot: Some(1),
            image: None,
        };
        let mut settlement = SettlementResult {
            rank: NumberField {
                raw_text: "8".into(),
                value: Some(8),
                confidence: 1.0,
            },
            squad_kills: NumberField {
                raw_text: "3".into(),
                value: Some(3),
                confidence: 1.0,
            },
            players: vec![
                player("Teammate"),
                player("Archer"),
                player("vjk0ok 0 DANGER"),
            ],
            warnings: vec![],
        };
        for (p, damage) in settlement.players.iter_mut().zip([921, 1216, 188]) {
            p.damage.value = Some(damage);
            p.kills.value = Some(1);
        }
        let annotations = associate(Some(&roster), &settlement);
        assert_eq!(
            annotations
                .iter()
                .map(|p| p.nickname.as_deref())
                .collect::<Vec<_>>(),
            vec![Some("Teammate"), Some("Archer"), Some("vjkook")]
        );
        assert!(annotations
            .iter()
            .all(|p| p.nickname_source == NicknameSource::VotedRoster));
        assert_eq!(annotations[1].is_local, Some(true));
        assert!(
            !annotations[2].name_confirmed,
            "a noisy card hint does not certify its association"
        );
        let project = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let directory = project.join(format!(
            ".local/storage-tests/voted-only-{}",
            observation_time_ms()
        ));
        let store = LocalStore::open(directory.clone())?;
        let image = crate::imaging::png(&RgbaImage::new(8, 8))?;
        store.save_settlement_capture(&settlement, &image, &annotations, None)?;
        let records = store.list_matches()?;
        assert_eq!(records[0].players[2].nickname.as_deref(), Some("vjkook"));
        assert_eq!(records[0].players[2].damage, Some(188));
        assert_eq!(records[0].players[1].damage, Some(1216));
        assert!(!serde_json::to_string(&records)?.contains("DANGER"));
        let unmapped = associate(None, &settlement);
        assert!(unmapped.iter().all(|p| p.nickname.is_none()
            && p.is_local.is_none()
            && p.nickname_source == NicknameSource::Unresolved));
        let hidden = SettlementResult {
            players: vec![player("unrelated"), player("hidden"), player("DANGER")],
            ..settlement
        };
        assert!(associate(Some(&roster), &hidden)
            .iter()
            .all(|p| p.nickname.is_none()));
        drop(store);
        std::fs::remove_dir_all(directory)?;
        Ok(())
    }

    #[test]
    fn capture_cadence_increases_only_for_summary() {
        let mut recorder = Recorder::default();
        assert_eq!(recorder.capture_interval_ms(), 2000);
        recorder.status.stage = AutoStage::LegendSelect;
        assert_eq!(recorder.capture_interval_ms(), 2000);
        recorder.active = true;
        assert!(recorder.roster_sampling_open());
        assert_eq!(
            recorder.capture_interval_ms(),
            2000,
            "single HUD read does not need faster capture"
        );
        recorder.roster_attempts = HUD_SAMPLE_LIMIT;
        assert_eq!(recorder.capture_interval_ms(), 2000);
        recorder
            .settlement_frames
            .push(Arc::new(RgbaImage::new(8, 8)));
        assert_eq!(recorder.capture_interval_ms(), 1000);
        recorder.locked = true;
        assert_eq!(recorder.capture_interval_ms(), 2000);
    }

    #[test]
    fn hud_budget_and_evidence_reset_only_at_new_round() {
        let mut recorder = Recorder::default();
        recorder.active = true;
        recorder.roster_attempts = HUD_SAMPLE_LIMIT;
        recorder.roster_readings = vec!["previous".into(); 3];
        recorder.roster_image = Some(Arc::new(RgbaImage::new(8, 8)));
        recorder.on_capture_gap(Duration::from_secs(21), "pause");
        assert_eq!(recorder.roster_attempts, HUD_SAMPLE_LIMIT);
        assert_eq!(recorder.roster_readings.len(), 3);
        recorder.begin_new_round();
        assert!(recorder.roster_readings.is_empty());
        assert!(recorder.roster_image.is_none());
        assert_eq!(recorder.roster_attempts, 0);
        assert!(recorder.roster.is_none());
    }

    #[test]
    fn selection_monitoring_never_reads_names_and_single_sample_stops_ocr() -> Result<()> {
        let resources = crate::resources()?;
        let mut detector = PageDetector::new(&resources)?;
        let image = Arc::new(RgbaImage::new(8, 8)); // Any actual OCR would reject this non-16:9 fixture.
        let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!(
            "../../.local/storage-tests/no-selection-ocr-{}",
            time::OffsetDateTime::now_utc().unix_timestamp_nanos()
        ));
        let store = LocalStore::open(directory.clone())?;
        let mut recorder = Recorder::default();
        recorder.process_observation(
            &image,
            Observation {
                page: Page::LegendSelect,
                token: None,
                squads: None,
                new_round_intro: true,
            },
            &mut detector,
            &store,
        )?;
        assert_eq!(recorder.status.stage, AutoStage::LegendSelect);
        assert!(recorder.roster_readings.is_empty());
        assert_eq!(recorder.roster_attempts, 0);
        assert_eq!(recorder.capture_interval_ms(), 2000);
        recorder.active = true;
        recorder.roster_attempts = HUD_SAMPLE_LIMIT;
        recorder.collect_roster(&image, &mut detector.names)?;
        assert_eq!(recorder.roster_attempts, 1);
        assert!(recorder.roster_readings.is_empty());
        drop(store);
        std::fs::remove_dir_all(directory)?;
        Ok(())
    }

    fn player(nickname: &str) -> crate::settlement::SettlementPlayer {
        crate::settlement::SettlementPlayer {
            position: "玩家".into(),
            nickname_hint: Some(nickname.into()),
            name_confirmed: false,
            kills: NumberField {
                raw_text: "0".into(),
                value: Some(0),
                confidence: 0.9,
            },
            assists: NumberField {
                raw_text: "0".into(),
                value: Some(0),
                confidence: 0.9,
            },
            knocks: NumberField {
                raw_text: "0".into(),
                value: Some(0),
                confidence: 0.9,
            },
            damage: NumberField {
                raw_text: "0".into(),
                value: Some(0),
                confidence: 0.9,
            },
        }
    }
}
