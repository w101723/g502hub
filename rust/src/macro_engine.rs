//! 侧键宏:CGEventTap 拦截 otherMouse 事件 + CGEventPost 键盘回放。
//!
//! CGEvent buttonNumber: 0=左键 1=右键 2=中键 3=G4(后退) 4=G5(前进)…
//! 需要辅助功能(Accessibility)权限。事件循环运行在独立线程的 CFRunLoop。

use crate::config::{Action, MacroBinding};
use anyhow::{anyhow, Result};
use core_foundation::base::{CFRelease, TCFType};
use core_foundation::boolean::CFBoolean;
use core_foundation::dictionary::CFDictionary;
use core_foundation::string::CFString;
use core_foundation_sys::runloop::kCFRunLoopDefaultMode;
use std::collections::BTreeMap;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------- //
// CoreGraphics / CoreFoundation FFI
// ---------------------------------------------------------------------- //
type CGEventRef = *mut c_void;
type CFMachPortRef = *mut c_void;
type CFRunLoopRef = *mut c_void;
type CFRunLoopSourceRef = *mut c_void;

#[allow(dead_code)]
const K_CG_HID_EVENT_TAP: u32 = 0;
const K_CG_SESSION_EVENT_TAP: u32 = 1;
const K_CG_EVENT_KEY_DOWN: u64 = 10;
const K_CG_EVENT_KEY_UP: u64 = 11;
const K_CG_EVENT_FLAGS_CHANGED: u64 = 12;
const K_CG_EVENT_OTHER_MOUSE_DOWN: u64 = 25;
const K_CG_EVENT_OTHER_MOUSE_UP: u64 = 26;
const K_CG_EVENT_TAP_DISABLED_BY_TIMEOUT: i64 = -2; // 0xFFFFFFFE as i64
const K_CG_EVENT_TAP_DISABLED_BY_USER_INPUT: i64 = -1; // 0xFFFFFFFF as i64
const K_CG_MOUSE_EVENT_BUTTON_NUMBER: i32 = 3;
const K_CG_KEYBOARD_EVENT_KEYCODE: i32 = 9;
const K_CG_EVENT_SOURCE_USER_DATA: i32 = 110;
const PLAYBACK_EVENT_TAG: i64 = 0x4750_3530_3248;

const FLAG_SHIFT: u64 = 1 << 17;
const FLAG_CONTROL: u64 = 1 << 18;
const FLAG_ALTERNATE: u64 = 1 << 19;
const FLAG_COMMAND: u64 = 1 << 20;

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGEventTapCreate(
        tap: u32,
        place: u32,
        options: u32,
        event_mask: u64,
        callback: CGEventTapCallBack,
        user_info: *mut c_void,
    ) -> CFMachPortRef;
    fn CGEventTapEnable(tap: CFMachPortRef, enable: bool);
    fn CGEventCreateKeyboardEvent(source: *mut c_void, keycode: u16, keydown: bool) -> CGEventRef;
    fn CGEventSetFlags(event: CGEventRef, flags: u64);
    fn CGEventGetFlags(event: CGEventRef) -> u64;
    fn CGEventPost(tap: u32, event: CGEventRef);
    fn CGEventGetIntegerValueField(event: CGEventRef, field: i32) -> i64;
    fn CGEventSetIntegerValueField(event: CGEventRef, field: i32, value: i64);
    fn CGEventKeyboardSetUnicodeString(event: CGEventRef, len: usize, s: *const u16);
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFMachPortCreateRunLoopSource(
        allocator: *mut c_void,
        port: CFMachPortRef,
        order: i64,
    ) -> CFRunLoopSourceRef;
    fn CFMachPortInvalidate(port: CFMachPortRef);
    fn CFRunLoopAddSource(rl: CFRunLoopRef, source: CFRunLoopSourceRef, mode: *const c_void);
    fn CFRunLoopRemoveSource(rl: CFRunLoopRef, source: CFRunLoopSourceRef, mode: *const c_void);
    fn CFRunLoopGetCurrent() -> CFRunLoopRef;
    fn CFRunLoopStop(rl: CFRunLoopRef);
    fn CFRunLoopRun();
}

type CGEventTapCallBack = unsafe extern "C" fn(
    proxy: *mut c_void,
    etype: u32,
    event: CGEventRef,
    user_info: *mut c_void,
) -> CGEventRef;

// ---------------------------------------------------------------------- //
// 辅助功能权限
// ---------------------------------------------------------------------- //
#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXIsProcessTrustedWithOptions(options: *const c_void) -> bool;
}

/// 检查/引导辅助功能授权。prompt=true 时弹出系统引导。
pub fn accessibility_granted(prompt: bool) -> bool {
    let key = CFString::new("AXTrustedCheckOptionPrompt");
    let value = CFBoolean::from(prompt);
    let pairs: Vec<(core_foundation::base::CFType, core_foundation::base::CFType)> =
        vec![(key.into_CFType(), value.into_CFType())];
    let dict = CFDictionary::from_CFType_pairs(&pairs);
    unsafe { AXIsProcessTrustedWithOptions(dict.as_concrete_TypeRef() as *const c_void) }
}

#[allow(dead_code)]
pub fn open_accessibility_settings() {
    let _ = std::process::Command::new("open")
        .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility")
        .spawn();
}

// ---------------------------------------------------------------------- //
// 按键表与回放
// ---------------------------------------------------------------------- //
fn keycode(name: &str) -> Option<u16> {
    let letters: &[(char, u16)] = &[
        ('a', 0),
        ('s', 1),
        ('d', 2),
        ('f', 3),
        ('h', 4),
        ('g', 5),
        ('z', 6),
        ('x', 7),
        ('c', 8),
        ('v', 9),
        ('b', 11),
        ('q', 12),
        ('w', 13),
        ('e', 14),
        ('r', 15),
        ('y', 16),
        ('t', 17),
        ('o', 31),
        ('u', 32),
        ('i', 34),
        ('p', 35),
        ('l', 37),
        ('j', 38),
        ('k', 40),
        ('n', 45),
        ('m', 46),
    ];
    let digits: &[(char, u16)] = &[
        ('1', 18),
        ('2', 19),
        ('3', 20),
        ('4', 21),
        ('5', 23),
        ('6', 22),
        ('7', 26),
        ('8', 28),
        ('9', 25),
        ('0', 29),
    ];
    for (c, k) in letters.iter().chain(digits.iter()) {
        if name.len() == 1 && name.chars().next() == Some(*c) {
            return Some(*k);
        }
    }
    let named: &[(&str, u16)] = &[
        ("return", 36),
        ("enter", 36),
        ("tab", 48),
        ("space", 49),
        ("delete", 51),
        ("esc", 53),
        ("forwarddelete", 117),
        ("home", 115),
        ("end", 119),
        ("pageup", 116),
        ("pagedown", 121),
        ("left", 123),
        ("right", 124),
        ("down", 125),
        ("up", 126),
        ("f1", 122),
        ("f2", 120),
        ("f3", 99),
        ("f4", 118),
        ("f5", 96),
        ("f6", 97),
        ("f7", 98),
        ("f8", 100),
        ("f9", 101),
        ("f10", 109),
        ("f11", 103),
        ("f12", 111),
        ("f13", 105),
        ("f14", 107),
        ("f15", 113),
        ("minus", 27),
        ("equal", 24),
        ("bracket_left", 33),
        ("bracket_right", 30),
        ("semicolon", 41),
        ("quote", 39),
        ("comma", 43),
        ("period", 47),
        ("slash", 44),
        ("backslash", 42),
        ("grave", 50),
    ];
    named.iter().find(|(n, _)| *n == name).map(|(_, k)| *k)
}

fn keycode_name(code: u16) -> Option<&'static str> {
    const NAMES: &[(u16, &str)] = &[
        (0, "a"),
        (1, "s"),
        (2, "d"),
        (3, "f"),
        (4, "h"),
        (5, "g"),
        (6, "z"),
        (7, "x"),
        (8, "c"),
        (9, "v"),
        (11, "b"),
        (12, "q"),
        (13, "w"),
        (14, "e"),
        (15, "r"),
        (16, "y"),
        (17, "t"),
        (18, "1"),
        (19, "2"),
        (20, "3"),
        (21, "4"),
        (22, "6"),
        (23, "5"),
        (24, "equal"),
        (25, "9"),
        (26, "7"),
        (27, "minus"),
        (28, "8"),
        (29, "0"),
        (30, "bracket_right"),
        (31, "o"),
        (32, "u"),
        (33, "bracket_left"),
        (34, "i"),
        (35, "p"),
        (36, "return"),
        (37, "l"),
        (38, "j"),
        (39, "quote"),
        (40, "k"),
        (41, "semicolon"),
        (42, "backslash"),
        (43, "comma"),
        (44, "slash"),
        (45, "n"),
        (46, "m"),
        (47, "period"),
        (48, "tab"),
        (49, "space"),
        (50, "grave"),
        (51, "delete"),
        (53, "esc"),
        (96, "f5"),
        (97, "f6"),
        (98, "f7"),
        (99, "f3"),
        (100, "f8"),
        (101, "f9"),
        (103, "f11"),
        (105, "f13"),
        (107, "f14"),
        (109, "f10"),
        (111, "f12"),
        (113, "f15"),
        (115, "home"),
        (116, "pageup"),
        (117, "forwarddelete"),
        (118, "f4"),
        (119, "end"),
        (120, "f2"),
        (121, "pagedown"),
        (122, "f1"),
        (123, "left"),
        (124, "right"),
        (125, "down"),
        (126, "up"),
    ];
    NAMES
        .iter()
        .find(|(kc, _)| *kc == code)
        .map(|(_, name)| *name)
}

fn modifier_names(flags: u64) -> Vec<String> {
    let mut names = Vec::new();
    if flags & FLAG_COMMAND != 0 {
        names.push("cmd".into());
    }
    if flags & FLAG_CONTROL != 0 {
        names.push("ctrl".into());
    }
    if flags & FLAG_ALTERNATE != 0 {
        names.push("alt".into());
    }
    if flags & FLAG_SHIFT != 0 {
        names.push("shift".into());
    }
    names
}

fn flags_from_modifiers(modifiers: &[String]) -> u64 {
    modifiers.iter().fold(0, |flags, name| {
        flags
            | match name.as_str() {
                "ctrl" => FLAG_CONTROL,
                "alt" | "opt" => FLAG_ALTERNATE,
                "shift" => FLAG_SHIFT,
                "cmd" => FLAG_COMMAND,
                _ => 0,
            }
    })
}

fn combo_name(flags: u64, code: u16) -> Option<String> {
    let key = keycode_name(code)?;
    let mut parts = modifier_names(flags);
    parts.push(key.into());
    Some(parts.join("+"))
}

fn parse_combo(spec: &str) -> Result<(u64, u16)> {
    let mut flags = 0u64;
    let mut key = None;
    for part in spec.split('+') {
        let p = part.trim().to_lowercase();
        match p.as_str() {
            "cmd" => flags |= FLAG_COMMAND,
            "ctrl" => flags |= FLAG_CONTROL,
            "alt" | "opt" => flags |= FLAG_ALTERNATE,
            "shift" => flags |= FLAG_SHIFT,
            _ => {
                if let Some(k) = keycode(&p) {
                    key = Some(k);
                } else if p.chars().count() == 1 {
                    // 单字符不在表里则忽略
                }
            }
        }
    }
    key.map(|k| (flags, k))
        .ok_or_else(|| anyhow!("无法解析按键: {spec}"))
}

fn tap_key(spec: &str) -> Result<()> {
    let (flags, keycode) = parse_combo(spec)?;
    unsafe {
        let down = CGEventCreateKeyboardEvent(std::ptr::null_mut(), keycode, true);
        if down.is_null() {
            return Err(anyhow!("CGEventCreateKeyboardEvent down 失败"));
        }
        if flags != 0 {
            CGEventSetFlags(down, flags);
        }
        CGEventSetIntegerValueField(down, K_CG_EVENT_SOURCE_USER_DATA, PLAYBACK_EVENT_TAG);
        CGEventPost(K_CG_SESSION_EVENT_TAP, down);
        CFRelease(down);

        let up = CGEventCreateKeyboardEvent(std::ptr::null_mut(), keycode, false);
        if up.is_null() {
            return Err(anyhow!("CGEventCreateKeyboardEvent up 失败"));
        }
        if flags != 0 {
            CGEventSetFlags(up, flags);
        }
        CGEventSetIntegerValueField(up, K_CG_EVENT_SOURCE_USER_DATA, PLAYBACK_EVENT_TAG);
        CGEventPost(K_CG_SESSION_EVENT_TAP, up);
        CFRelease(up);
    }
    std::thread::sleep(std::time::Duration::from_millis(20));
    Ok(())
}

fn type_text(text: &str) -> Result<()> {
    let utf16: Vec<u16> = text.encode_utf16().collect();
    if utf16.is_empty() {
        return Ok(());
    }
    unsafe {
        let down = CGEventCreateKeyboardEvent(std::ptr::null_mut(), 0, true);
        if down.is_null() {
            return Err(anyhow!("CGEventCreateKeyboardEvent down 失败"));
        }
        CGEventKeyboardSetUnicodeString(down, utf16.len(), utf16.as_ptr());
        CGEventSetIntegerValueField(down, K_CG_EVENT_SOURCE_USER_DATA, PLAYBACK_EVENT_TAG);
        CGEventPost(K_CG_SESSION_EVENT_TAP, down);
        CFRelease(down);

        let up = CGEventCreateKeyboardEvent(std::ptr::null_mut(), 0, false);
        if up.is_null() {
            return Err(anyhow!("CGEventCreateKeyboardEvent up 失败"));
        }
        CGEventKeyboardSetUnicodeString(up, utf16.len(), utf16.as_ptr());
        CGEventSetIntegerValueField(up, K_CG_EVENT_SOURCE_USER_DATA, PLAYBACK_EVENT_TAG);
        CGEventPost(K_CG_SESSION_EVENT_TAP, up);
        CFRelease(up);
    }
    std::thread::sleep(std::time::Duration::from_millis(20));
    Ok(())
}

pub fn play(actions: &[Action]) -> Result<()> {
    for action in actions {
        if let Some(ms) = action.delay_ms {
            std::thread::sleep(std::time::Duration::from_millis(ms));
        } else if let Some(keys) = &action.keys {
            tap_key(keys)?;
        } else if let Some(text) = &action.text {
            type_text(text)?;
        } else if let Some(kc) = action.keycode {
            let flags = flags_from_modifiers(&action.modifiers);
            let states: &[bool] = match action.key_down {
                Some(true) => &[true],
                Some(false) => &[false],
                None => &[true, false],
            };
            unsafe {
                for &keydown in states {
                    let event =
                        CGEventCreateKeyboardEvent(std::ptr::null_mut(), kc as u16, keydown);
                    if !event.is_null() {
                        if flags != 0 {
                            CGEventSetFlags(event, flags);
                        }
                        CGEventSetIntegerValueField(
                            event,
                            K_CG_EVENT_SOURCE_USER_DATA,
                            PLAYBACK_EVENT_TAG,
                        );
                        CGEventPost(K_CG_SESSION_EVENT_TAP, event);
                        CFRelease(event);
                    }
                }
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------- //
// 宏链路日志(仅供启动/停止/回放线程外轻量记录，tap 回调内禁用)
// ---------------------------------------------------------------------- //
pub fn mlog(msg: &str) {
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("/tmp/g502hub_macro.log")
    {
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let _ = writeln!(f, "[{ts}] {msg}");
    }
}

// ---------------------------------------------------------------------- //
// 绑定缓存与全局运行状态
// ---------------------------------------------------------------------- //
pub type MacroBindings = Arc<RwLock<BTreeMap<String, MacroBinding>>>;

static GLOBAL_BINDINGS: RwLock<Option<MacroBindings>> = RwLock::new(None);
static TAP_PORT: RwLock<usize> = RwLock::new(0); // CFMachPortRef as usize
static TAP_RUNLOOP: RwLock<usize> = RwLock::new(0); // CFRunLoopRef as usize
static TAP_SOURCE: RwLock<usize> = RwLock::new(0); // CFRunLoopSourceRef as usize
static PLAYBACK_TX: RwLock<Option<mpsc::Sender<Vec<Action>>>> = RwLock::new(None);
static LAST_BUTTON: AtomicI64 = AtomicI64::new(-1);
static RECORDING_SERIAL: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordingKind {
    Shortcut,
    Sequence,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordingPhase {
    Idle,
    AwaitMouse(RecordingKind),
    Shortcut { button: u32 },
    Sequence { button: u32, events: usize },
}

#[derive(Debug, Clone)]
pub struct RecordingResult {
    pub button: u32,
    pub kind: RecordingKind,
    pub label: String,
    pub actions: Vec<Action>,
}

#[derive(Debug, Clone)]
pub enum RecordingOutcome {
    Completed(RecordingResult),
    Cancelled(String),
}

enum RecordingState {
    Idle,
    AwaitMouse {
        kind: RecordingKind,
        started: Instant,
    },
    Shortcut {
        button: u32,
        started: Instant,
        pending: Option<(u16, u64)>,
        completed: Option<RecordingResult>,
        mouse_up_pending: bool,
    },
    Sequence {
        button: u32,
        last_event: Instant,
        actions: Vec<Action>,
        mouse_up_pending: bool,
    },
}

static RECORDING_STATE: Mutex<RecordingState> = Mutex::new(RecordingState::Idle);
static RECORDING_OUTCOME: Mutex<Option<RecordingOutcome>> = Mutex::new(None);

fn empty_action() -> Action {
    Action {
        keys: None,
        text: None,
        keycode: None,
        delay_ms: None,
        key_down: None,
        modifiers: Vec::new(),
    }
}

/// 获取最近接收到的鼠标按键编号
pub fn last_button() -> Option<u32> {
    let btn = LAST_BUTTON.load(Ordering::Relaxed);
    if btn >= 0 {
        Some(btn as u32)
    } else {
        None
    }
}

pub fn recording_phase() -> RecordingPhase {
    let Ok(state) = RECORDING_STATE.lock() else {
        return RecordingPhase::Idle;
    };
    match &*state {
        RecordingState::Idle => RecordingPhase::Idle,
        RecordingState::AwaitMouse { kind, .. } => RecordingPhase::AwaitMouse(*kind),
        RecordingState::Shortcut { button, .. } => RecordingPhase::Shortcut { button: *button },
        RecordingState::Sequence {
            button, actions, ..
        } => RecordingPhase::Sequence {
            button: *button,
            events: actions.iter().filter(|a| a.keycode.is_some()).count(),
        },
    }
}

pub fn recording_outcome_pending() -> bool {
    RECORDING_OUTCOME
        .lock()
        .map(|outcome| outcome.is_some())
        .unwrap_or(false)
}

pub fn take_recording_outcome() -> Option<RecordingOutcome> {
    RECORDING_OUTCOME.lock().ok()?.take()
}

fn set_recording_outcome(outcome: RecordingOutcome) {
    if let Ok(mut result) = RECORDING_OUTCOME.lock() {
        *result = Some(outcome);
    }
    RECORDING_SERIAL.fetch_add(1, Ordering::Relaxed);
}

pub fn recording_serial() -> u64 {
    RECORDING_SERIAL.load(Ordering::Relaxed)
}

/// 全局更新按键宏绑定
#[allow(dead_code)]
pub fn update_bindings(new_bindings: BTreeMap<String, MacroBinding>) {
    if let Ok(g) = GLOBAL_BINDINGS.read() {
        if let Some(bindings) = g.as_ref() {
            if let Ok(mut lock) = bindings.write() {
                *lock = new_bindings;
            }
        }
    }
}

pub trait IntoBindings {
    fn into_bindings(self) -> MacroBindings;
}

impl IntoBindings for MacroBindings {
    fn into_bindings(self) -> MacroBindings {
        self
    }
}

impl IntoBindings for BTreeMap<String, MacroBinding> {
    fn into_bindings(self) -> MacroBindings {
        Arc::new(RwLock::new(self))
    }
}

impl<F> IntoBindings for Arc<F>
where
    F: Fn() -> BTreeMap<String, MacroBinding> + Send + Sync + 'static,
{
    fn into_bindings(self) -> MacroBindings {
        Arc::new(RwLock::new(self()))
    }
}

struct TapThreads {
    runloop_thread: Option<std::thread::JoinHandle<()>>,
    playback_thread: Option<std::thread::JoinHandle<()>>,
}

pub struct MacroTap {
    #[allow(dead_code)]
    bindings: MacroBindings,
    running: Arc<AtomicBool>,
    threads: Mutex<Option<TapThreads>>,
}

impl MacroTap {
    pub fn new<T: IntoBindings>(bindings: T) -> Self {
        let arc_bindings = bindings.into_bindings();
        if let Ok(mut g) = GLOBAL_BINDINGS.write() {
            *g = Some(arc_bindings.clone());
        }
        MacroTap {
            bindings: arc_bindings,
            running: Arc::new(AtomicBool::new(false)),
            threads: Mutex::new(None),
        }
    }

    #[allow(dead_code)]
    pub fn bindings(&self) -> MacroBindings {
        self.bindings.clone()
    }

    #[allow(dead_code)]
    pub fn update_bindings(&self, new_bindings: BTreeMap<String, MacroBinding>) {
        if let Ok(mut lock) = self.bindings.write() {
            *lock = new_bindings;
        }
    }

    pub fn last_button(&self) -> Option<u32> {
        last_button()
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::Relaxed)
    }

    pub fn begin_recording(&self, kind: RecordingKind) -> Result<()> {
        if !self.is_running() {
            return Err(anyhow!("宏引擎尚未运行"));
        }
        let mut state = RECORDING_STATE
            .lock()
            .map_err(|_| anyhow!("录制状态锁已损坏"))?;
        *state = RecordingState::AwaitMouse {
            kind,
            started: Instant::now(),
        };
        if let Ok(mut outcome) = RECORDING_OUTCOME.lock() {
            *outcome = None;
        }
        RECORDING_SERIAL.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    pub fn finish_sequence(&self) -> Result<RecordingResult> {
        let mut state = RECORDING_STATE
            .lock()
            .map_err(|_| anyhow!("录制状态锁已损坏"))?;
        let RecordingState::Sequence {
            button,
            mut actions,
            ..
        } = std::mem::replace(&mut *state, RecordingState::Idle)
        else {
            return Err(anyhow!("当前没有正在录制的按键序列"));
        };
        // 结果由菜单处理线程直接保存;这里不再写入 outcome,
        // 否则 0.2s 泵会再次取到并重复保存/通知。
        RECORDING_SERIAL.fetch_add(1, Ordering::Relaxed);
        if actions.is_empty() {
            return Err(anyhow!("未录到键盘事件"));
        }
        close_pressed_keys(&mut actions);
        let event_count = actions.iter().filter(|a| a.keycode.is_some()).count();
        let result = RecordingResult {
            button,
            kind: RecordingKind::Sequence,
            label: format!("按键序列（{event_count} 个事件）"),
            actions,
        };
        Ok(result)
    }

    pub fn cancel_recording(&self, reason: impl Into<String>) {
        if let Ok(mut state) = RECORDING_STATE.lock() {
            if !matches!(*state, RecordingState::Idle) {
                *state = RecordingState::Idle;
                set_recording_outcome(RecordingOutcome::Cancelled(reason.into()));
            }
        }
    }

    pub fn expire_recording(&self) {
        let reason = {
            let Ok(state) = RECORDING_STATE.lock() else {
                return;
            };
            match &*state {
                RecordingState::AwaitMouse { started, .. }
                    if started.elapsed() >= Duration::from_secs(20) =>
                {
                    Some("等待鼠标按键超时；该按键可能未暴露给主机".to_string())
                }
                RecordingState::Shortcut { started, .. }
                    if started.elapsed() >= Duration::from_secs(30) =>
                {
                    Some("等待快捷键超时".to_string())
                }
                RecordingState::Sequence { last_event, .. }
                    if last_event.elapsed() >= Duration::from_secs(120) =>
                {
                    Some("按键序列录制空闲超时".to_string())
                }
                _ => None,
            }
        };
        if let Some(reason) = reason {
            self.cancel_recording(reason);
        }
    }

    /// 创建事件 tap;无辅助功能权限返回 Err。
    pub fn start(&self) -> Result<()> {
        if self.is_running() {
            return Ok(());
        }

        // 1. 启动单一播放队列工作线程
        let (tx, rx) = mpsc::channel::<Vec<Action>>();
        *PLAYBACK_TX.write().unwrap() = Some(tx);
        let playback_thread = std::thread::Builder::new()
            .name("g502-macro-playback".into())
            .spawn(move || {
                while let Ok(actions) = rx.recv() {
                    if let Err(e) = play(&actions) {
                        mlog(&format!("playback ERR: {e}"));
                    }
                }
            })?;

        // 2. 创建 CGEventTap
        let mask = (1u64 << K_CG_EVENT_OTHER_MOUSE_DOWN)
            | (1u64 << K_CG_EVENT_OTHER_MOUSE_UP)
            | (1u64 << K_CG_EVENT_KEY_DOWN)
            | (1u64 << K_CG_EVENT_KEY_UP)
            | (1u64 << K_CG_EVENT_FLAGS_CHANGED);
        let tap = unsafe {
            CGEventTapCreate(
                K_CG_SESSION_EVENT_TAP,
                0, // head insert
                0, // active tap(可吞键)
                mask,
                tap_callback,
                std::ptr::null_mut(),
            )
        };
        if tap.is_null() {
            // 清理已创建的播放通道
            if let Ok(mut tx_guard) = PLAYBACK_TX.write() {
                tx_guard.take();
            }
            let _ = playback_thread.join();
            mlog("tap create FAILED(需要辅助功能权限)");
            return Err(anyhow!("CGEventTapCreate 失败:需要辅助功能权限"));
        }

        mlog("tap created OK(拦截引擎启动)");
        *TAP_PORT.write().unwrap() = tap as usize;
        self.running.store(true, Ordering::Relaxed);

        // 3. 启动 RunLoop 事件循环线程
        let (init_tx, init_rx) = mpsc::channel::<Result<()>>();
        let running_clone = self.running.clone();
        let tap_addr = tap as usize;

        let runloop_thread = std::thread::Builder::new()
            .name("g502-macro-runloop".into())
            .spawn(move || unsafe {
                let tap = tap_addr as CFMachPortRef;
                let source = CFMachPortCreateRunLoopSource(std::ptr::null_mut(), tap, 0);
                if source.is_null() {
                    let _ = init_tx.send(Err(anyhow!("CFMachPortCreateRunLoopSource 失败")));
                    return;
                }
                let rl = CFRunLoopGetCurrent();
                *TAP_RUNLOOP.write().unwrap() = rl as usize;
                *TAP_SOURCE.write().unwrap() = source as usize;
                CFRunLoopAddSource(rl, source, kCFRunLoopDefaultMode as *const c_void);
                CGEventTapEnable(tap, true);
                let _ = init_tx.send(Ok(()));

                mlog("tap runloop 运行中,等待侧键事件");
                CFRunLoopRun();
                mlog("tap runloop 退出");

                CFRunLoopRemoveSource(rl, source, kCFRunLoopDefaultMode as *const c_void);
                CFRelease(source);
                *TAP_SOURCE.write().unwrap() = 0;
                *TAP_RUNLOOP.write().unwrap() = 0;
                running_clone.store(false, Ordering::Relaxed);
            })?;

        match init_rx.recv() {
            Ok(Ok(())) => {
                let mut th = self.threads.lock().unwrap();
                *th = Some(TapThreads {
                    runloop_thread: Some(runloop_thread),
                    playback_thread: Some(playback_thread),
                });
                Ok(())
            }
            Ok(Err(e)) => {
                self.stop();
                Err(e)
            }
            Err(_) => {
                self.stop();
                Err(anyhow!("tap runloop 初始化通道异常关闭"))
            }
        }
    }

    pub fn stop(&self) {
        self.cancel_recording("宏引擎已停用");
        self.running.store(false, Ordering::Relaxed);

        // 1. 关闭单一播放队列(使 worker 线程自然退出)
        if let Ok(mut tx_guard) = PLAYBACK_TX.write() {
            tx_guard.take();
        }

        // 2. 先禁用 tap，再停止并回收 RunLoop；Mach port 必须在线程退出后释放，
        // 避免 RunLoop source 仍引用已经释放的 CFMachPort。
        let port = *TAP_PORT.read().unwrap();
        if port != 0 {
            unsafe { CGEventTapEnable(port as CFMachPortRef, false) };
        }
        let rl = *TAP_RUNLOOP.read().unwrap();
        if rl != 0 {
            unsafe { CFRunLoopStop(rl as CFRunLoopRef) };
        }

        // 3. 回收线程
        if let Ok(mut th_guard) = self.threads.lock() {
            if let Some(mut th) = th_guard.take() {
                if let Some(h) = th.runloop_thread.take() {
                    let _ = h.join();
                }
                if let Some(h) = th.playback_thread.take() {
                    let _ = h.join();
                }
            }
        }

        // 4. RunLoop 已退出，可以安全注销并释放 tap port。
        let port = {
            let mut guard = TAP_PORT.write().unwrap();
            let p = *guard;
            *guard = 0;
            p
        };
        if port != 0 {
            unsafe {
                CFMachPortInvalidate(port as CFMachPortRef);
                CFRelease(port as CFMachPortRef);
            }
        }
        mlog("tap stopped");
    }
}

// ---------------------------------------------------------------------- //
// 事件 tap 回调函数
// 严格要求:
// 1. 不做磁盘IO/日志IO/格式化/播放
// 2. 避免 extern C callback panic (顶层使用 catch_unwind)
// 3. 正确吞 enabled binding 的 down/up
// 4. 自动记录 last button atomic 状态
// 5. 超时自动重启 tap
// ---------------------------------------------------------------------- //
unsafe extern "C" fn tap_callback(
    proxy: *mut c_void,
    etype: u32,
    event: CGEventRef,
    user_info: *mut c_void,
) -> CGEventRef {
    let result = std::panic::catch_unwind(|| tap_callback_inner(proxy, etype, event, user_info));
    match result {
        Ok(ret) => ret,
        Err(_) => event, // 防止跨越 FFI 边界 unwind
    }
}

#[inline(always)]
fn button_to_key(button: i64) -> Option<&'static str> {
    match button {
        0 => Some("mouse0"),
        1 => Some("mouse1"),
        2 => Some("mouse2"),
        3 => Some("mouse3"),
        4 => Some("mouse4"),
        5 => Some("mouse5"),
        6 => Some("mouse6"),
        7 => Some("mouse7"),
        8 => Some("mouse8"),
        9 => Some("mouse9"),
        10 => Some("mouse10"),
        11 => Some("mouse11"),
        12 => Some("mouse12"),
        13 => Some("mouse13"),
        14 => Some("mouse14"),
        15 => Some("mouse15"),
        16 => Some("mouse16"),
        17 => Some("mouse17"),
        18 => Some("mouse18"),
        19 => Some("mouse19"),
        20 => Some("mouse20"),
        21 => Some("mouse21"),
        22 => Some("mouse22"),
        23 => Some("mouse23"),
        24 => Some("mouse24"),
        25 => Some("mouse25"),
        26 => Some("mouse26"),
        27 => Some("mouse27"),
        28 => Some("mouse28"),
        29 => Some("mouse29"),
        30 => Some("mouse30"),
        31 => Some("mouse31"),
        _ => None,
    }
}

fn modifier_mask_for_keycode(code: u16) -> u64 {
    match code {
        54 | 55 => FLAG_COMMAND,
        56 | 60 => FLAG_SHIFT,
        58 | 61 => FLAG_ALTERNATE,
        59 | 62 => FLAG_CONTROL,
        _ => 0,
    }
}

fn is_key_pressed(actions: &[Action], code: u16) -> bool {
    actions
        .iter()
        .rev()
        .find(|a| a.keycode == Some(code as u32))
        .map(|a| a.key_down != Some(false))
        .unwrap_or(false)
}

fn close_pressed_keys(actions: &mut Vec<Action>) {
    let mut pressed: BTreeMap<u32, Vec<String>> = BTreeMap::new();
    for action in actions.iter() {
        let (Some(code), Some(down)) = (action.keycode, action.key_down) else {
            continue;
        };
        if down {
            pressed.insert(code, action.modifiers.clone());
        } else {
            pressed.remove(&code);
        }
    }
    for (code, modifiers) in pressed {
        let mut release = empty_action();
        release.keycode = Some(code);
        release.key_down = Some(false);
        release.modifiers = modifiers;
        actions.push(release);
    }
}

fn sequence_event(
    actions: &mut Vec<Action>,
    last_event: &mut Instant,
    code: u16,
    flags: u64,
    down: bool,
) {
    let now = Instant::now();
    if !actions.is_empty() {
        let delay = now
            .duration_since(*last_event)
            .as_millis()
            .min(u64::MAX as u128) as u64;
        if delay > 0 {
            let mut action = empty_action();
            action.delay_ms = Some(delay);
            actions.push(action);
        }
    }
    let mut action = empty_action();
    action.keycode = Some(code as u32);
    action.key_down = Some(down);
    action.modifiers = modifier_names(flags);
    actions.push(action);
    *last_event = now;
}

fn record_mouse(button: u32, is_down: bool) -> bool {
    let Ok(mut state) = RECORDING_STATE.lock() else {
        return false;
    };
    match &mut *state {
        RecordingState::Idle => false,
        RecordingState::AwaitMouse { kind, .. } => {
            if is_down {
                let kind = *kind;
                let now = Instant::now();
                *state = match kind {
                    RecordingKind::Shortcut => RecordingState::Shortcut {
                        button,
                        started: now,
                        pending: None,
                        completed: None,
                        mouse_up_pending: true,
                    },
                    RecordingKind::Sequence => RecordingState::Sequence {
                        button,
                        last_event: now,
                        actions: Vec::new(),
                        mouse_up_pending: true,
                    },
                };
                RECORDING_SERIAL.fetch_add(1, Ordering::Relaxed);
            }
            true
        }
        RecordingState::Shortcut {
            button: selected,
            mouse_up_pending,
            ..
        }
        | RecordingState::Sequence {
            button: selected,
            mouse_up_pending,
            ..
        } => {
            if !is_down && button == *selected && *mouse_up_pending {
                *mouse_up_pending = false;
            }
            true
        }
    }
}

fn record_keyboard(etype: u64, code: u16, flags: u64) -> bool {
    let Ok(mut state) = RECORDING_STATE.lock() else {
        return false;
    };
    match &mut *state {
        RecordingState::Idle | RecordingState::AwaitMouse { .. } => false,
        RecordingState::Shortcut {
            button,
            pending,
            completed,
            ..
        } => {
            if etype == K_CG_EVENT_KEY_DOWN {
                if pending.is_none() {
                    *pending = Some((code, flags));
                }
                return true;
            }
            if etype == K_CG_EVENT_KEY_UP {
                if let Some((pending_code, pending_flags)) = *pending {
                    if pending_code == code {
                        let mut action = empty_action();
                        let label = if let Some(combo) = combo_name(pending_flags, pending_code) {
                            action.keys = Some(combo.clone());
                            combo
                        } else {
                            action.keycode = Some(pending_code as u32);
                            action.modifiers = modifier_names(pending_flags);
                            format!("keycode {pending_code}")
                        };
                        *completed = Some(RecordingResult {
                            button: *button,
                            kind: RecordingKind::Shortcut,
                            label,
                            actions: vec![action],
                        });
                        if flags & (FLAG_COMMAND | FLAG_CONTROL | FLAG_ALTERNATE | FLAG_SHIFT) == 0
                        {
                            let result = completed.take().unwrap();
                            *state = RecordingState::Idle;
                            set_recording_outcome(RecordingOutcome::Completed(result));
                        }
                    }
                }
                return true;
            }
            if etype == K_CG_EVENT_FLAGS_CHANGED {
                if completed.is_some()
                    && flags & (FLAG_COMMAND | FLAG_CONTROL | FLAG_ALTERNATE | FLAG_SHIFT) == 0
                {
                    let result = completed.take().unwrap();
                    *state = RecordingState::Idle;
                    set_recording_outcome(RecordingOutcome::Completed(result));
                }
                return true;
            }
            false
        }
        RecordingState::Sequence {
            actions,
            last_event,
            ..
        } => {
            if etype == K_CG_EVENT_KEY_DOWN || etype == K_CG_EVENT_KEY_UP {
                let down = etype == K_CG_EVENT_KEY_DOWN;
                // 长按产生的 auto-repeat keyDown 只保留首次,维持"按下-释放"语义。
                if down && is_key_pressed(actions, code) {
                    return true;
                }
                sequence_event(actions, last_event, code, flags, down);
                RECORDING_SERIAL.fetch_add(1, Ordering::Relaxed);
                return true;
            }
            if etype == K_CG_EVENT_FLAGS_CHANGED {
                let modifier_mask = modifier_mask_for_keycode(code);
                if modifier_mask != 0 {
                    sequence_event(actions, last_event, code, flags, flags & modifier_mask != 0);
                    RECORDING_SERIAL.fetch_add(1, Ordering::Relaxed);
                }
                return true;
            }
            false
        }
    }
}

unsafe fn tap_callback_inner(
    _proxy: *mut c_void,
    etype: u32,
    event: CGEventRef,
    _user_info: *mut c_void,
) -> CGEventRef {
    if etype as i64 == K_CG_EVENT_TAP_DISABLED_BY_TIMEOUT
        || etype as i64 == K_CG_EVENT_TAP_DISABLED_BY_USER_INPUT
    {
        if let Ok(guard) = TAP_PORT.read() {
            let port = *guard;
            if port != 0 {
                CGEventTapEnable(port as CFMachPortRef, true);
            }
        }
        return event;
    }

    if CGEventGetIntegerValueField(event, K_CG_EVENT_SOURCE_USER_DATA) == PLAYBACK_EVENT_TAG {
        return event;
    }

    let event_type = etype as u64;
    if matches!(
        event_type,
        K_CG_EVENT_KEY_DOWN | K_CG_EVENT_KEY_UP | K_CG_EVENT_FLAGS_CHANGED
    ) {
        let code = CGEventGetIntegerValueField(event, K_CG_KEYBOARD_EVENT_KEYCODE) as u16;
        let flags = CGEventGetFlags(event);
        return if record_keyboard(event_type, code, flags) {
            std::ptr::null_mut()
        } else {
            event
        };
    }

    let is_down = event_type == K_CG_EVENT_OTHER_MOUSE_DOWN;
    let is_up = event_type == K_CG_EVENT_OTHER_MOUSE_UP;
    if !is_down && !is_up {
        return event;
    }

    let button = CGEventGetIntegerValueField(event, K_CG_MOUSE_EVENT_BUTTON_NUMBER);
    LAST_BUTTON.store(button, Ordering::Relaxed);
    if button >= 0 && record_mouse(button as u32, is_down) {
        return std::ptr::null_mut();
    }

    let Some(key) = button_to_key(button) else {
        return event;
    };
    let global_guard = match GLOBAL_BINDINGS.read() {
        Ok(g) => g,
        Err(_) => return event,
    };
    let Some(arc_bindings) = global_guard.as_ref() else {
        return event;
    };
    let bindings_guard = match arc_bindings.read() {
        Ok(b) => b,
        Err(_) => return event,
    };
    let Some(binding) = bindings_guard.get(key) else {
        return event;
    };
    if !binding.enabled {
        return event;
    }

    if is_down {
        let actions = binding.actions.clone();
        drop(bindings_guard);
        drop(global_guard);
        if let Ok(tx_guard) = PLAYBACK_TX.read() {
            if let Some(tx) = tx_guard.as_ref() {
                let _ = tx.send(actions);
            }
        }
        std::ptr::null_mut()
    } else {
        std::ptr::null_mut()
    }
}

// ---------------------------------------------------------------------- //
// 单元测试
// ---------------------------------------------------------------------- //
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_button_to_key() {
        assert_eq!(button_to_key(0), Some("mouse0"));
        assert_eq!(button_to_key(3), Some("mouse3"));
        assert_eq!(button_to_key(4), Some("mouse4"));
        assert_eq!(button_to_key(31), Some("mouse31"));
        assert_eq!(button_to_key(32), None);
        assert_eq!(button_to_key(-1), None);
    }

    #[test]
    fn test_parse_combo() {
        let (flags, code) = parse_combo("cmd+c").expect("parse cmd+c");
        assert_eq!(flags, FLAG_COMMAND);
        assert_eq!(code, 8); // c

        let (flags2, code2) = parse_combo("ctrl+shift+a").expect("parse ctrl+shift+a");
        assert_eq!(flags2, FLAG_CONTROL | FLAG_SHIFT);
        assert_eq!(code2, 0); // a

        assert!(parse_combo("invalid_key_name_xyz").is_err());
    }

    #[test]
    fn test_combo_name_round_trip() {
        let combos = ["cmd+c", "ctrl+shift+a", "alt+f4", "cmd+shift+4"];
        for combo in combos {
            let (flags, code) = parse_combo(combo).unwrap();
            assert_eq!(combo_name(flags, code).as_deref(), Some(combo));
        }
    }

    #[test]
    fn test_sequence_event_building() {
        let mut actions = Vec::new();
        let mut last = Instant::now();
        sequence_event(&mut actions, &mut last, 8, FLAG_COMMAND, true);
        sequence_event(&mut actions, &mut last, 8, FLAG_COMMAND, false);
        let events: Vec<_> = actions.iter().filter(|a| a.keycode.is_some()).collect();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].key_down, Some(true));
        assert_eq!(events[1].key_down, Some(false));
        assert_eq!(events[0].modifiers, vec!["cmd"]);
    }

    #[test]
    fn test_close_pressed_keys() {
        let mut press = empty_action();
        press.keycode = Some(8);
        press.key_down = Some(true);
        press.modifiers = vec!["cmd".into()];
        let mut actions = vec![press];
        close_pressed_keys(&mut actions);
        assert_eq!(actions.len(), 2);
        assert_eq!(actions[1].keycode, Some(8));
        assert_eq!(actions[1].key_down, Some(false));
        assert_eq!(actions[1].modifiers, vec!["cmd"]);
    }

    #[test]
    fn test_sequence_skips_auto_repeat() {
        *RECORDING_STATE.lock().unwrap() = RecordingState::Sequence {
            button: 4,
            last_event: Instant::now(),
            actions: Vec::new(),
            mouse_up_pending: false,
        };
        assert!(record_keyboard(K_CG_EVENT_KEY_DOWN, 8, 0));
        assert!(record_keyboard(K_CG_EVENT_KEY_DOWN, 8, 0)); // auto-repeat
        assert!(record_keyboard(K_CG_EVENT_KEY_UP, 8, 0));
        let state = RECORDING_STATE.lock().unwrap();
        let RecordingState::Sequence { actions, .. } = &*state else {
            panic!("expected sequence state");
        };
        let events: Vec<_> = actions.iter().filter(|a| a.keycode.is_some()).collect();
        assert_eq!(events.len(), 2, "auto-repeat 不应产生新事件");
        assert_eq!(events[0].key_down, Some(true));
        assert_eq!(events[1].key_down, Some(false));
    }

    #[test]
    fn test_recording_state_transitions() {
        *RECORDING_STATE.lock().unwrap() = RecordingState::AwaitMouse {
            kind: RecordingKind::Shortcut,
            started: Instant::now(),
        };
        assert!(record_mouse(7, true));
        assert_eq!(recording_phase(), RecordingPhase::Shortcut { button: 7 });
        assert!(record_keyboard(K_CG_EVENT_KEY_DOWN, 8, FLAG_COMMAND));
        assert!(record_keyboard(K_CG_EVENT_KEY_UP, 8, FLAG_COMMAND));
        assert!(record_keyboard(K_CG_EVENT_FLAGS_CHANGED, 55, 0));
        let Some(RecordingOutcome::Completed(result)) = take_recording_outcome() else {
            panic!("expected completed recording");
        };
        assert_eq!(result.button, 7);
        assert_eq!(result.label, "cmd+c");
        assert_eq!(result.actions[0].keys.as_deref(), Some("cmd+c"));
    }

    #[test]
    fn test_macro_tap_bindings_and_atomic_state() {
        let mut initial_bindings = BTreeMap::new();
        initial_bindings.insert(
            "mouse3".to_string(),
            MacroBinding {
                name: Some("测试".into()),
                enabled: true,
                actions: vec![Action {
                    keys: Some("cmd+c".into()),
                    text: None,
                    keycode: None,
                    delay_ms: None,
                    key_down: None,
                    modifiers: Vec::new(),
                }],
            },
        );

        let tap = MacroTap::new(initial_bindings);
        assert!(!tap.is_running());

        // 测试绑定缓存读取
        {
            let b = tap.bindings();
            let guard = b.read().unwrap();
            assert!(guard.contains_key("mouse3"));
            assert!(guard.get("mouse3").unwrap().enabled);
        }

        // 测试动态更新绑定
        let mut updated_bindings = BTreeMap::new();
        updated_bindings.insert(
            "mouse4".to_string(),
            MacroBinding {
                name: Some("前进".into()),
                enabled: false,
                actions: vec![],
            },
        );
        tap.update_bindings(updated_bindings);

        {
            let b = tap.bindings();
            let guard = b.read().unwrap();
            assert!(!guard.contains_key("mouse3"));
            assert!(guard.contains_key("mouse4"));
            assert!(!guard.get("mouse4").unwrap().enabled);
        }

        // 测试 last_button 原子状态
        LAST_BUTTON.store(4, Ordering::Relaxed);
        assert_eq!(tap.last_button(), Some(4));
        assert_eq!(last_button(), Some(4));

        LAST_BUTTON.store(-1, Ordering::Relaxed);
        assert_eq!(tap.last_button(), None);
    }
}
