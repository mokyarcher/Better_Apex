use crate::capture::Target;
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
    time::{Duration, Instant},
};
use windows::Win32::{
    Foundation::HWND,
    UI::{Input::KeyboardAndMouse::*, WindowsAndMessaging::*},
};

#[derive(Clone, Serialize, Deserialize)]
pub struct Status {
    pub enabled: bool,
    #[serde(skip_deserializing)]
    pub error: String,
}
impl Default for Status {
    fn default() -> Self {
        Self {
            enabled: true,
            error: String::new(),
        }
    }
}
pub struct Settings {
    path: PathBuf,
    value: Mutex<Status>,
}
impl Settings {
    pub fn open(directory: &Path) -> Self {
        let path = directory.join("auto-next-settings.json");
        let value = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| Status {
                enabled: false,
                error: format!("自动下一局设置读取失败：{e}"),
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Status::default(),
            Err(e) => Status {
                enabled: false,
                error: e.to_string(),
            },
        };
        Self {
            path,
            value: Mutex::new(value),
        }
    }
    pub fn status(&self) -> Status {
        self.value.lock().unwrap().clone()
    }
    pub fn enabled(&self) -> bool {
        self.value.lock().unwrap().enabled
    }
    pub fn save(&self, enabled: bool) -> Result<Status> {
        let mut value = self.value.lock().unwrap();
        let next = Status {
            enabled,
            error: String::new(),
        };
        std::fs::create_dir_all(self.path.parent().context("自动下一局设置目录无效")?)?;
        let temporary = self.path.with_extension("json.tmp");
        std::fs::write(&temporary, serde_json::to_vec_pretty(&next)?)?;
        std::fs::rename(temporary, &self.path)?;
        *value = next.clone();
        Ok(next)
    }
}

struct Pending {
    id: String,
    frame: u64,
    deadline: Instant,
}
#[derive(Default)]
pub struct Gate {
    pending: Option<Pending>,
    last_id: Option<String>,
}
#[derive(Debug, PartialEq)]
pub enum Decision {
    Wait,
    Cancel(String),
    Ready(String),
}
impl Gate {
    pub fn arm(&mut self, id: String, frame: u64, now: Instant) {
        if self.last_id.as_ref() == Some(&id) {
            return;
        }
        self.last_id = Some(id.clone());
        self.pending = Some(Pending {
            id,
            frame,
            deadline: now + Duration::from_secs(10),
        });
    }
    pub fn poll(
        &mut self,
        frame: u64,
        now: Instant,
        enabled: bool,
        foreground: bool,
        saved: bool,
    ) -> Decision {
        let Some(pending) = self.pending.as_ref() else {
            return Decision::Wait;
        };
        let cancelled = if !enabled {
            Some("自动下一局已关闭")
        } else if !foreground {
            Some("游戏已切出，未发送按键")
        } else if !saved {
            Some("已离开本局结算，未发送按键")
        } else if now >= pending.deadline {
            Some("未取得新的结算确认画面，请手动按1")
        } else {
            None
        };
        if let Some(reason) = cancelled {
            self.pending = None;
            return Decision::Cancel(reason.into());
        }
        if frame <= pending.frame {
            return Decision::Wait;
        }
        Decision::Ready(self.pending.take().unwrap().id)
    }
}

pub fn rematch_prompt(text: &str) -> bool {
    let text = text
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>();
    text.contains("重新")
        && text.contains("匹配")
        && !["取消", "匹配中", "正在"]
            .iter()
            .any(|word| text.contains(word))
}
pub fn foreground(target: &Target) -> bool {
    unsafe {
        let hwnd = HWND(target.handle as *mut _);
        let mut pid = 0;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        IsWindow(Some(hwnd)).as_bool()
            && !IsIconic(hwnd).as_bool()
            && pid == target.pid
            && GetAncestor(GetForegroundWindow(), GA_ROOT) == hwnd
    }
}
fn key_input(up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wScan: 0x02,
                dwFlags: if up {
                    KEYEVENTF_SCANCODE | KEYEVENTF_KEYUP
                } else {
                    KEYEVENTF_SCANCODE
                },
                ..Default::default()
            },
        },
    }
}
pub fn press_one(target: &Target, stopping: &AtomicBool, settings: &Settings) -> Result<()> {
    ensure!(
        !stopping.load(Ordering::Relaxed) && settings.enabled(),
        "自动下一局已取消"
    );
    ensure!(foreground(target), "游戏不在前台，未发送按键");
    unsafe {
        for key in [VK_CONTROL, VK_MENU, VK_SHIFT, VK_LWIN, VK_RWIN, VK_1] {
            ensure!(
                GetAsyncKeyState(key.0 as i32) >= 0,
                "检测到修饰键或数字1正在按住，请手动继续"
            );
        }
        // Recheck immediately before input; never activate or refocus a game.
        ensure!(
            !stopping.load(Ordering::Relaxed) && settings.enabled() && foreground(target),
            "发送前已取消或游戏已切出"
        );
        ensure!(
            SendInput(&[key_input(false)], std::mem::size_of::<INPUT>() as i32) == 1,
            "数字1按下未发送，可能被系统或游戏阻止"
        );
        // Always release once down was sent, including if stop/focus changes.
        std::thread::sleep(Duration::from_millis(50));
        let released = SendInput(&[key_input(true)], std::mem::size_of::<INPUT>() as i32);
        if released != 1 {
            let _ = SendInput(&[key_input(true)], std::mem::size_of::<INPUT>() as i32);
        }
        ensure!(released == 1, "数字1释放未确认，请手动检查");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn one_shot_gate_requires_save_and_fresh_frame_and_cancels_on_changes() {
        let now = Instant::now();
        let mut gate = Gate::default();
        assert_eq!(gate.poll(10, now, true, true, true), Decision::Wait);
        gate.arm("a".into(), 10, now);
        assert_eq!(gate.poll(10, now, true, true, true), Decision::Wait);
        assert_eq!(
            gate.poll(11, now, true, true, true),
            Decision::Ready("a".into())
        );
        assert_eq!(gate.poll(12, now, true, true, true), Decision::Wait);
        gate.arm("a".into(), 12, now);
        assert_eq!(gate.poll(13, now, true, true, true), Decision::Wait);
        for (id, enabled, foreground, saved) in [
            ("b", false, true, true),
            ("c", true, false, true),
            ("d", true, true, false),
        ] {
            gate.arm(id.into(), 20, now);
            assert!(matches!(
                gate.poll(21, now, enabled, foreground, saved),
                Decision::Cancel(_)
            ));
            assert_eq!(gate.poll(22, now, true, true, true), Decision::Wait);
        }
        gate.arm("e".into(), 30, now);
        assert!(matches!(
            gate.poll(31, now + Duration::from_secs(10), true, true, true),
            Decision::Cancel(_)
        ));
    }
    #[test]
    fn prompt_and_key_are_specific_to_rematch_and_top_row_one() {
        assert!(rematch_prompt("1 重 新 单 人 匹 配"));
        assert!(rematch_prompt("1 重新匹配"));
        for text in [
            "取消重新匹配",
            "正在重新匹配",
            "重新匹配中",
            "1选择传奇",
            "小队排名",
        ] {
            assert!(!rematch_prompt(text));
        }
        unsafe {
            let down = key_input(false).Anonymous.ki;
            let up = key_input(true).Anonymous.ki;
            assert_eq!(down.wScan, 0x02);
            assert_eq!(down.wVk.0, 0);
            assert!(down.dwFlags.contains(KEYEVENTF_SCANCODE));
            assert!(!down.dwFlags.contains(KEYEVENTF_KEYUP));
            assert!(up.dwFlags.contains(KEYEVENTF_KEYUP));
        }
    }
    #[test]
    fn option_persists_and_invalid_settings_do_not_enable_input() -> Result<()> {
        let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!(
            "../../.local/storage-tests/auto-next-{}",
            time::OffsetDateTime::now_utc().unix_timestamp_nanos()
        ));
        let settings = Settings::open(&directory);
        assert!(settings.enabled());
        settings.save(false)?;
        assert!(!Settings::open(&directory).enabled());
        std::fs::write(directory.join("auto-next-settings.json"), b"broken")?;
        assert!(!Settings::open(&directory).enabled());
        assert!(!Settings::open(&directory).status().error.is_empty());
        // No live keyboard input in tests, including all recorder OCR tests.
        std::fs::remove_dir_all(directory)?;
        Ok(())
    }
}
