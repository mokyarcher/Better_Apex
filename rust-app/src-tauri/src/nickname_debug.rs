use serde::Serialize;
use serde_json::{json, Value};
use std::{
    fs::{File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
};
use time::{format_description::well_known::Rfc3339, OffsetDateTime, UtcOffset};

#[derive(Serialize)]
pub struct DebugStatus {
    pub enabled: bool,
    pub directory: String,
    pub current_file: Option<String>,
    pub error: String,
}

#[derive(Default)]
struct Files {
    json: Option<File>,
    text: Option<File>,
    path: Option<PathBuf>,
    error: String,
    sequence: u64,
}

pub struct NicknameDebug {
    directory: PathBuf,
    enabled: AtomicBool,
    rounds: AtomicU64,
    files: Mutex<Files>,
}

impl NicknameDebug {
    pub fn enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }
    pub fn new(data_dir: &Path) -> Self {
        Self {
            directory: data_dir.join("nickname-debug"),
            enabled: AtomicBool::new(true),
            rounds: AtomicU64::new(0),
            files: Mutex::new(Files::default()),
        }
    }

    pub fn status(&self) -> DebugStatus {
        let files = self.files.lock().unwrap();
        DebugStatus {
            enabled: self.enabled.load(Ordering::Relaxed),
            directory: self.directory.display().to_string(),
            current_file: files.path.as_ref().map(|p| p.display().to_string()),
            error: files.error.clone(),
        }
    }

    pub fn set_enabled(&self, enabled: bool) {
        if !enabled {
            self.write(
                "",
                "debug_disabled",
                json!({"note":"本次运行已关闭；已有日志保留"}),
            );
        }
        self.enabled.store(enabled, Ordering::Relaxed);
        if enabled {
            self.write("", "debug_enabled", json!({"note":"本次运行继续记录"}));
        }
    }

    pub fn round(&self, mode: &str) -> String {
        let round = format!(
            "{mode}-{:04}",
            self.rounds.fetch_add(1, Ordering::Relaxed) + 1
        );
        self.write(&round, "round_started", json!({"mode":mode,"version":env!("CARGO_PKG_VERSION"),"identity_policy":"02_hud_single_read_only","selection_frames":0,"hud_frames":1,"recognition_rule":"single_read_no_vote","empty_readings":"unresolved_no_retry"}));
        round
    }

    pub fn context(
        self: &Arc<Self>,
        round: &str,
        stage: &str,
        frame: u64,
        sample: u8,
    ) -> TraceContext {
        TraceContext {
            log: self.clone(),
            round: round.into(),
            stage: stage.into(),
            frame,
            sample,
        }
    }

    pub fn write(&self, round: &str, event: &str, data: Value) {
        if !self.enabled.load(Ordering::Relaxed) {
            return;
        }
        let mut files = self.files.lock().unwrap();
        if !self.enabled.load(Ordering::Relaxed) {
            return;
        }
        let result = (|| -> anyhow::Result<()> {
            let now = OffsetDateTime::now_utc();
            let beijing = now.to_offset(UtcOffset::from_hms(8, 0, 0)?);
            if files.json.is_none() {
                std::fs::create_dir_all(&self.directory)?;
                let basename = format!(
                    "nickname-{:04}{:02}{:02}-{:02}{:02}{:02}-{}-{}",
                    beijing.year(),
                    u8::from(beijing.month()),
                    beijing.day(),
                    beijing.hour(),
                    beijing.minute(),
                    beijing.second(),
                    std::process::id(),
                    now.unix_timestamp_nanos()
                );
                let path = self.directory.join(format!("{basename}.jsonl"));
                let json_file = OpenOptions::new()
                    .create_new(true)
                    .write(true)
                    .open(&path)?;
                let mut text_file = OpenOptions::new()
                    .create_new(true)
                    .write(true)
                    .open(self.directory.join(format!("{basename}.txt")))?;
                text_file.write_all("\u{feff}Better Apex 临时昵称复盘日志\n时间含北京时间；slot 从0开始，sample从1开始。raw空字符串表示未识别，error表示调用失败。\nWindows raw为原文；Tesseract raw为[原文,置信度]；PP-OCR raw包含text、lines（文字/置信度/区域边界）和elapsed_ms。\n自动昵称只读取02局内一次，直接采用结果，不投票、不重试；01只监视阶段，03 settlement_card仅作不可信对齐线索。hud_result记录原始读数、三人结果和一次尝试完成状态；空读数／失败保留未知。\n02的sample固定为1；外层frame为监测已处理帧号。03位置可能与02不同，仍需一对一匹配。nickname_source=voted_roster是兼容既有存储的标签，当前表示02单次识别名单；manual表示用户明确选择，unresolved表示未知。日志不代表已确认归属，不补写开启前的样本。\n\n".as_bytes())?;
                files.json = Some(json_file);
                files.text = Some(text_file);
                files.path = Some(path);
            }
            files.sequence += 1;
            let event = json!({"schema":1,"sequence":files.sequence,"time_utc":now.format(&Rfc3339)?,"time_beijing":beijing.format(&Rfc3339)?,"round":round,"event":event,"data":data});
            let mut line = serde_json::to_vec(&event)?;
            line.push(b'\n');
            files.json.as_mut().unwrap().write_all(&line)?;
            files.json.as_mut().unwrap().flush()?;
            let text = format!(
                "#{} {} [{}] {}\n{}\n\n",
                files.sequence,
                event["time_beijing"].as_str().unwrap_or_default(),
                round,
                event["event"].as_str().unwrap_or_default(),
                serde_json::to_string_pretty(&event["data"])?
            );
            files.text.as_mut().unwrap().write_all(text.as_bytes())?;
            files.text.as_mut().unwrap().flush()?;
            Ok(())
        })();
        files.error = result
            .err()
            .map_or_else(String::new, |e| format!("昵称调试日志写入失败：{e:#}"));
    }
}

#[derive(Clone)]
pub struct TraceContext {
    log: Arc<NicknameDebug>,
    round: String,
    stage: String,
    frame: u64,
    sample: u8,
}

impl TraceContext {
    pub fn event(&self, event: &str, data: Value) {
        self.log.write(
            &self.round,
            event,
            json!({"stage":self.stage,"frame":self.frame,"sample":self.sample,"detail":data}),
        );
    }

    pub fn ocr<T: Serialize>(
        &self,
        slot: usize,
        source: &str,
        engine: &str,
        result: &anyhow::Result<T>,
    ) {
        match result {
            Ok(raw) => self.event("ocr", json!({"slot":slot,"source":source,"engine":engine,"raw":raw})),
            Err(error) => self.event("ocr", json!({"slot":slot,"source":source,"engine":engine,"raw":null,"error":format!("{error:#}")})),
        }
    }
}

// Only diagnostics are swallowed: an unwritable debug directory must never
// prevent OCR, a match save or the next round from running.
pub fn record_ocr<T: Serialize>(
    trace: Option<&TraceContext>,
    slot: usize,
    source: &str,
    engine: &str,
    result: anyhow::Result<T>,
) -> anyhow::Result<T> {
    if let Some(trace) = trace {
        trace.ocr(slot, source, engine, &result);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_root(label: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!(
            "../../.local/storage-tests/{label}-{}",
            OffsetDateTime::now_utc().unix_timestamp_nanos()
        ))
    }

    #[test]
    fn logs_raw_empty_errors_rounds_and_toggle_without_overwriting() -> anyhow::Result<()> {
        let root = test_root("nickname-log");
        let log = Arc::new(NicknameDebug::new(&root));
        let round = log.round("auto");
        let trace = log.context(&round, "01_selection", 7, 1);
        let original = " 玩家\"一\"\n错字 ".to_string();
        assert_eq!(
            record_ocr(
                Some(&trace),
                0,
                "upper_left",
                "windows",
                Ok(original.clone())
            )?,
            original
        );
        record_ocr(Some(&trace), 1, "lower_bar", "windows", Ok(String::new()))?;
        let failure: anyhow::Result<String> = Err(anyhow::anyhow!("OCR fixture error"));
        assert!(record_ocr(Some(&trace), 2, "upper_left", "tesseract", failure).is_err());
        let path = PathBuf::from(log.status().current_file.unwrap());
        log.set_enabled(false);
        let disabled = std::fs::read(&path)?;
        record_ocr(Some(&trace), 0, "upper_left", "windows", Ok("off"))?;
        assert_eq!(std::fs::read(&path)?, disabled);
        log.set_enabled(true);
        let next = log.round("auto");
        log.context(&next, "02_hud", 8, 1)
            .event("hud_normalized", json!({"names":["第二局"]}));
        let lines: Vec<Value> = std::fs::read_to_string(&path)?
            .lines()
            .map(serde_json::from_str)
            .collect::<Result<_, _>>()?;
        let ocr: Vec<_> = lines.iter().filter(|line| line["event"] == "ocr").collect();
        assert_eq!(ocr.len(), 3);
        assert_eq!(ocr[0]["data"]["detail"]["raw"], original);
        assert_eq!(ocr[1]["data"]["detail"]["raw"], "");
        assert!(ocr[2]["data"]["detail"]["error"]
            .as_str()
            .unwrap()
            .contains("OCR fixture error"));
        assert_ne!(round, next);
        assert_eq!(
            log.status().current_file.as_deref(),
            Some(path.to_str().unwrap())
        );
        assert!(lines
            .iter()
            .all(|line| line["time_beijing"].as_str().unwrap().ends_with("+08:00")));
        let text_path = path.with_extension("txt");
        assert!(std::fs::read_to_string(text_path)?.contains("临时昵称复盘日志"));
        let another = NicknameDebug::new(&root);
        another.round("manual_probe");
        assert_ne!(
            another.status().current_file.as_deref(),
            Some(path.to_str().unwrap())
        );
        assert_eq!(std::fs::read_to_string(&path)?.lines().count(), lines.len());
        drop(trace);
        drop(log);
        drop(another);
        std::fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn unwritable_log_does_not_change_the_ocr_result() -> anyhow::Result<()> {
        let root = test_root("nickname-log-error");
        std::fs::create_dir_all(&root)?;
        std::fs::write(root.join("nickname-debug"), b"directory blocked by fixture")?;
        let log = Arc::new(NicknameDebug::new(&root));
        let round = log.round("auto");
        let trace = log.context(&round, "02_hud", 1, 1);
        assert_eq!(
            record_ocr(Some(&trace), 0, "hud_lower_left", "windows", Ok("Archer"))?,
            "Archer"
        );
        assert!(!log.status().error.is_empty());
        drop(trace);
        drop(log);
        std::fs::remove_dir_all(root)?;
        Ok(())
    }
}
