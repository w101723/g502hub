//! 侧键宏:CGEventTap 拦截 otherMouse 事件 + CGEventPost 键盘回放。
//!
//! CGEvent buttonNumber: 0=左键 1=右键 2=中键 3=G4(后退) 4=G5(前进)…
//! 需要辅助功能(Accessibility)权限。事件循环运行在独立线程的 CFRunLoop。

use crate::config::{Action, DeviceAction, MacroBinding};
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

const K_CG_HID_EVENT_TAP: u32 = 0;
const K_CG_SESSION_EVENT_TAP: u32 = 1;
const K_CG_EVENT_KEY_DOWN: u64 = 10;
const K_CG_EVENT_KEY_UP: u64 = 11;
const K_CG_EVENT_FLAGS_CHANGED: u64 = 12;
const K_CG_EVENT_SCROLL_WHEEL: u64 = 22;
const K_CG_EVENT_OTHER_MOUSE_DOWN: u64 = 25;
const K_CG_EVENT_OTHER_MOUSE_UP: u64 = 26;
const K_CG_EVENT_TAP_DISABLED_BY_TIMEOUT: u32 = u32::MAX - 1;
const K_CG_EVENT_TAP_DISABLED_BY_USER_INPUT: u32 = u32::MAX;
const K_CG_MOUSE_EVENT_BUTTON_NUMBER: i32 = 3;
const K_CG_SCROLL_WHEEL_EVENT_DELTA_AXIS_1: i32 = 11;
const K_CG_SCROLL_WHEEL_EVENT_DELTA_AXIS_2: i32 = 12;
const K_CG_SCROLL_WHEEL_EVENT_IS_CONTINUOUS: i32 = 88;
const K_CG_SCROLL_WHEEL_EVENT_MOMENTUM_PHASE: i32 = 123;
const K_CG_KEYBOARD_EVENT_KEYCODE: i32 = 9;
const K_CG_EVENT_SOURCE_USER_DATA: i32 = 42;
const PLAYBACK_EVENT_TAG: i64 = 0x4750_3530_3248;
const TAP_PROBE_EVENT_TAG: i64 = 0x4750_3530_3250;
const K_CG_EVENT_SOURCE_STATE_COMBINED_SESSION_STATE: i32 = 0;

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
    fn CGEventSourceCreate(source_state: i32) -> *mut c_void;
    fn CGEventCreateKeyboardEvent(source: *mut c_void, keycode: u16, keydown: bool) -> CGEventRef;
    fn CGEventSetFlags(event: CGEventRef, flags: u64);
    fn CGEventGetFlags(event: CGEventRef) -> u64;
    fn CGEventPost(tap: u32, event: CGEventRef);
    fn CGEventGetIntegerValueField(event: CGEventRef, field: i32) -> i64;
    fn CGEventSetIntegerValueField(event: CGEventRef, field: i32, value: i64);
    fn CGEventKeyboardSetUnicodeString(event: CGEventRef, len: usize, s: *const u16);
    fn CGPreflightListenEventAccess() -> bool;
    fn CGRequestListenEventAccess() -> bool;
    fn CGPreflightPostEventAccess() -> bool;
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

pub fn input_monitoring_granted(prompt: bool) -> bool {
    unsafe {
        if CGPreflightListenEventAccess() {
            true
        } else if prompt {
            CGRequestListenEventAccess()
        } else {
            false
        }
    }
}

pub fn event_posting_granted() -> bool {
    unsafe { CGPreflightPostEventAccess() }
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

fn modifier_events(flags: u64) -> Vec<(u64, u16)> {
    [
        (FLAG_CONTROL, 59),
        (FLAG_ALTERNATE, 58),
        (FLAG_SHIFT, 56),
        (FLAG_COMMAND, 55),
    ]
    .into_iter()
    .filter(|(flag, _)| flags & flag != 0)
    .collect()
}

unsafe fn post_tap_probe() -> Result<()> {
    let source = CGEventSourceCreate(K_CG_EVENT_SOURCE_STATE_COMBINED_SESSION_STATE);
    let event = CGEventCreateKeyboardEvent(source, u16::MAX, true);
    if event.is_null() {
        if !source.is_null() {
            CFRelease(source);
        }
        return Err(anyhow!("CGEventCreateKeyboardEvent 自检事件创建失败"));
    }
    CGEventSetIntegerValueField(event, K_CG_EVENT_SOURCE_USER_DATA, TAP_PROBE_EVENT_TAG);
    CGEventPost(K_CG_HID_EVENT_TAP, event);
    CFRelease(event);
    if !source.is_null() {
        CFRelease(source);
    }
    Ok(())
}

unsafe fn post_keyboard_event(code: u16, down: bool, flags: u64) -> Result<()> {
    let source = CGEventSourceCreate(K_CG_EVENT_SOURCE_STATE_COMBINED_SESSION_STATE);
    let event = CGEventCreateKeyboardEvent(source, code, down);
    if event.is_null() {
        if !source.is_null() {
            CFRelease(source);
        }
        return Err(anyhow!("CGEventCreateKeyboardEvent 失败: keycode={code}"));
    }
    let current_flags = CGEventGetFlags(event);
    let modifier_mask = FLAG_SHIFT | FLAG_CONTROL | FLAG_ALTERNATE | FLAG_COMMAND;
    CGEventSetFlags(event, (current_flags & !modifier_mask) | flags);
    CGEventSetIntegerValueField(event, K_CG_EVENT_SOURCE_USER_DATA, PLAYBACK_EVENT_TAG);
    CGEventPost(K_CG_SESSION_EVENT_TAP, event);
    CFRelease(event);
    if !source.is_null() {
        CFRelease(source);
    }
    Ok(())
}

fn tap_key(spec: &str) -> Result<()> {
    let (flags, keycode) = parse_combo(spec)?;
    let modifiers = modifier_events(flags);

    // 依照罗技 G HUB 硬件模拟规范时序(每步保持 30~50ms 真实物理等待):
    // 1. 依次按下修饰键，带上累积修饰键掩码
    let mut active = 0;
    for &(flag, code) in &modifiers {
        active |= flag;
        unsafe { post_keyboard_event(code, true, active)? };
        std::thread::sleep(std::time::Duration::from_millis(30));
    }

    // 2. 按下目标主键(带完整组合键掩码)
    unsafe {
        post_keyboard_event(keycode, true, flags)?;
    }
    std::thread::sleep(std::time::Duration::from_millis(50));

    // 3. 释放目标主键
    unsafe {
        post_keyboard_event(keycode, false, flags)?;
    }
    std::thread::sleep(std::time::Duration::from_millis(30));

    // 4. 逆序释放修饰键
    for &(flag, code) in modifiers.iter().rev() {
        active &= !flag;
        unsafe { post_keyboard_event(code, false, active)? };
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    std::thread::sleep(std::time::Duration::from_millis(20));

    Ok(())
}

fn type_text_cgevent(text: &str) -> Result<()> {
    let utf16: Vec<u16> = text.encode_utf16().collect();
    if utf16.is_empty() {
        return Ok(());
    }
    for chunk in utf16.chunks(20) {
        unsafe {
            let source = CGEventSourceCreate(K_CG_EVENT_SOURCE_STATE_COMBINED_SESSION_STATE);
            let down = CGEventCreateKeyboardEvent(source, 0, true);
            if down.is_null() {
                if !source.is_null() {
                    CFRelease(source);
                }
                return Err(anyhow!("CGEventCreateKeyboardEvent down 失败"));
            }
            CGEventKeyboardSetUnicodeString(down, chunk.len(), chunk.as_ptr());
            CGEventSetIntegerValueField(down, K_CG_EVENT_SOURCE_USER_DATA, PLAYBACK_EVENT_TAG);
            CGEventPost(K_CG_SESSION_EVENT_TAP, down);
            CFRelease(down);

            let up = CGEventCreateKeyboardEvent(source, 0, false);
            if up.is_null() {
                if !source.is_null() {
                    CFRelease(source);
                }
                return Err(anyhow!("CGEventCreateKeyboardEvent up 失败"));
            }
            CGEventKeyboardSetUnicodeString(up, chunk.len(), chunk.as_ptr());
            CGEventSetIntegerValueField(up, K_CG_EVENT_SOURCE_USER_DATA, PLAYBACK_EVENT_TAG);
            CGEventPost(K_CG_SESSION_EVENT_TAP, up);
            CFRelease(up);
            if !source.is_null() {
                CFRelease(source);
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(15));
    }
    Ok(())
}

fn type_text(text: &str) -> Result<()> {
    if text.is_empty() {
        return Ok(());
    }
    // 短暂等待物理鼠标按键抬起（50ms），防止系统处于鼠标拖拽状态时抑制键盘事件
    std::thread::sleep(std::time::Duration::from_millis(50));

    // 首选方案：使用系统剪贴板注入 + Cmd+V 快速粘贴
    // 自动暂存并在 250ms 后恢复用户原剪贴板内容，彻底兼容中英文字符、符号、任何长度及第三方中文输入法
    let paste_result = (|| -> Result<()> {
        let pb = unsafe { objc2_app_kit::NSPasteboard::generalPasteboard() };
        let prev_text = unsafe {
            pb.stringForType(objc2_app_kit::NSPasteboardTypeString)
                .map(|s| s.to_string())
        };
        unsafe {
            pb.clearContents();
            pb.setString_forType(
                &objc2_foundation::NSString::from_str(text),
                objc2_app_kit::NSPasteboardTypeString,
            );
        }

        tap_key("cmd+v")?;

        if let Some(old) = prev_text {
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(250));
                let pb = unsafe { objc2_app_kit::NSPasteboard::generalPasteboard() };
                unsafe {
                    pb.clearContents();
                    pb.setString_forType(
                        &objc2_foundation::NSString::from_str(&old),
                        objc2_app_kit::NSPasteboardTypeString,
                    );
                }
            });
        }
        Ok(())
    })();

    if let Err(e) = paste_result {
        mlog(&format!("剪贴板注入粘贴失败，降级为 CGEvent: {e}"));
        type_text_cgevent(text)?;
    }
    Ok(())
}

pub fn play(actions: &[Action]) -> Result<()> {
    for action in actions {
        if let Some(ms) = action.delay_ms {
            std::thread::sleep(std::time::Duration::from_millis(ms));
        } else if let Some(dev_action) = &action.device_action {
            match dev_action {
                DeviceAction::BatteryLevel => {
                    if !crate::menubar::dispatch_menu_action("macro:battery-status") {
                        let dev = crate::device::get_conn(2)?;
                        let battery = crate::features::battery::read_battery(&dev)?;
                        let cfg = crate::config::load()?;
                        crate::features::battery_indicator::show_battery_level(
                            &dev,
                            battery.percent,
                            &cfg,
                        )?;
                    }
                }
            }
        } else if let Some(keys) = &action.keys {
            if keys == "action:battery" {
                if !crate::menubar::dispatch_menu_action("macro:battery-status") {
                    let dev = crate::device::get_conn(2)?;
                    let battery = crate::features::battery::read_battery(&dev)?;
                    let cfg = crate::config::load()?;
                    crate::features::battery_indicator::show_battery_level(
                        &dev,
                        battery.percent,
                        &cfg,
                    )?;
                }
            } else {
                tap_key(keys)?;
            }
        } else if let Some(text) = &action.text {
            type_text(text)?;
        } else if let Some(kc) = action.keycode {
            let flags = flags_from_modifiers(&action.modifiers);
            let states: &[bool] = match action.key_down {
                Some(true) => &[true],
                Some(false) => &[false],
                None => &[true, false],
            };
            for (idx, &keydown) in states.iter().enumerate() {
                unsafe {
                    post_keyboard_event(kc as u16, keydown, flags)?;
                }
                if idx + 1 < states.len() {
                    std::thread::sleep(std::time::Duration::from_millis(40));
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
static PLAYBACK_TX: RwLock<Option<mpsc::Sender<(String, Vec<Action>)>>> = RwLock::new(None);
static LAST_BUTTON: AtomicI64 = AtomicI64::new(-1);
static RECORDING_SERIAL: AtomicU64 = AtomicU64::new(0);
static TAP_RECOVERY_COUNT: AtomicU64 = AtomicU64::new(0);
static TAP_PROBE_SEEN: AtomicBool = AtomicBool::new(false);
static RAW_EVENT_SERIAL: AtomicU64 = AtomicU64::new(0);
static RAW_EVENT_KIND: AtomicI64 = AtomicI64::new(0);
static RAW_EVENT_VALUE_1: AtomicI64 = AtomicI64::new(0);
static RAW_EVENT_VALUE_2: AtomicI64 = AtomicI64::new(0);

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
    Action::default()
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

static GLOBAL_TAP: std::sync::OnceLock<Arc<MacroTap>> = std::sync::OnceLock::new();

pub fn set_global_tap(tap: Arc<MacroTap>) {
    let _ = GLOBAL_TAP.set(tap);
}

pub fn global_tap() -> Option<Arc<MacroTap>> {
    GLOBAL_TAP.get().cloned()
}

pub fn is_tap_running() -> bool {
    global_tap().map(|t| t.is_running()).unwrap_or(false)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GKeyDef {
    pub id: &'static str,
    pub name: &'static str,
    pub desc: &'static str,
    pub default_btn: u32,
    pub is_battery_default: bool,
}

pub const G_KEYS: &[GKeyDef] = &[
    GKeyDef {
        id: "mouse3",
        name: "G4",
        desc: "侧键·后退",
        default_btn: 3,
        is_battery_default: false,
    },
    GKeyDef {
        id: "mouse4",
        name: "G5",
        desc: "侧键·前进",
        default_btn: 4,
        is_battery_default: false,
    },
    GKeyDef {
        id: "mouse5",
        name: "G6",
        desc: "侧键·瞄准(DPI Shift)",
        default_btn: 5,
        is_battery_default: false,
    },
    GKeyDef {
        id: "mouse6",
        name: "G7",
        desc: "左键旁·后(DPI -)",
        default_btn: 6,
        is_battery_default: false,
    },
    GKeyDef {
        id: "mouse7",
        name: "G8",
        desc: "左键旁·前(DPI +)",
        default_btn: 7,
        is_battery_default: false,
    },
    GKeyDef {
        id: "mouse8",
        name: "G9",
        desc: "滚轮后·电量/模式",
        default_btn: 8,
        is_battery_default: true,
    },
    GKeyDef {
        id: "mouse9",
        name: "G10",
        desc: "滚轮·向左倾斜",
        default_btn: 9,
        is_battery_default: false,
    },
    GKeyDef {
        id: "mouse10",
        name: "G11",
        desc: "滚轮·向右倾斜",
        default_btn: 10,
        is_battery_default: false,
    },
];

pub fn get_gkey_by_id(id: &str) -> Option<&'static GKeyDef> {
    G_KEYS
        .iter()
        .find(|k| k.id == id || k.name.eq_ignore_ascii_case(id))
}

pub fn get_gkey_by_button(button: u32) -> Option<&'static GKeyDef> {
    G_KEYS.iter().find(|k| k.default_btn == button)
}

pub fn default_battery_binding() -> MacroBinding {
    MacroBinding {
        name: Some("设备动作 · 电池电量".into()),
        enabled: true,
        actions: vec![Action {
            device_action: Some(DeviceAction::BatteryLevel),
            ..Action::default()
        }],
    }
}

pub fn get_binding_for_key(button_key: &str) -> Option<MacroBinding> {
    let key_id = if let Some(gk) = get_gkey_by_id(button_key) {
        gk.id
    } else {
        button_key
    };
    if let Ok(cfg) = crate::config::load() {
        if let Some(b) = cfg.macros.get(key_id) {
            return Some(b.clone());
        }
    }
    None
}

pub fn can_save_text_binding(editable: bool, binding: Option<&MacroBinding>) -> bool {
    editable && binding.is_none_or(|binding| get_binding_text(binding).is_some())
}

pub fn get_binding_text(binding: &MacroBinding) -> Option<String> {
    for a in &binding.actions {
        if a.device_action.is_some() {
            return None;
        }
        if let Some(t) = &a.text {
            return Some(t.clone());
        }
    }
    None
}

pub fn save_text_binding(button_key: &str, text: &str) -> Result<()> {
    let key_id = if let Some(gk) = get_gkey_by_id(button_key) {
        gk.id
    } else {
        button_key
    };
    if text.is_empty() {
        return clear_binding(key_id);
    }
    // 忠实保留用户输入的前后及所有空格，包含纯空格宏（如输入空格键或缩进）
    let display_text = if text.chars().count() > 24 {
        let truncated: String = text.chars().take(24).collect();
        format!("{truncated}...")
    } else {
        text.to_string()
    };
    let binding = MacroBinding {
        name: Some(format!("文字: \"{display_text}\"")),
        enabled: true,
        actions: vec![Action {
            text: Some(text.to_string()),
            ..Action::default()
        }],
    };
    let macros = crate::config::update(|cfg| {
        cfg.macros.insert(key_id.to_string(), binding);
        cfg.macros.clone()
    })?;
    if let Some(tap) = global_tap() {
        tap.update_bindings(macros);
    }
    Ok(())
}

pub fn save_battery_binding(button_key: &str) -> Result<()> {
    let key_id = if let Some(gk) = get_gkey_by_id(button_key) {
        gk.id
    } else {
        button_key
    };
    let binding = default_battery_binding();
    let macros = crate::config::update(|cfg| {
        cfg.macros.insert(key_id.to_string(), binding);
        cfg.macros.clone()
    })?;
    if let Some(tap) = global_tap() {
        tap.update_bindings(macros);
    }
    Ok(())
}

pub fn clear_binding(button_key: &str) -> Result<()> {
    let key_id = if let Some(gk) = get_gkey_by_id(button_key) {
        gk.id
    } else {
        button_key
    };
    let macros = crate::config::update(|cfg| {
        cfg.macros.remove(key_id);
        cfg.macros.clone()
    })?;
    if let Some(tap) = global_tap() {
        tap.update_bindings(macros);
    }
    Ok(())
}

pub fn format_recorded_keys(binding: &MacroBinding) -> Option<String> {
    if binding.name.as_deref()?.starts_with("录制快捷键:") {
        if let Some(keys) = binding
            .actions
            .iter()
            .find_map(|action| action.keys.as_deref())
        {
            return Some(keys.to_string());
        }
        let action = binding.actions.first()?;
        let code = action.keycode?;
        let key = keycode_name(code as u16)
            .map(str::to_string)
            .unwrap_or_else(|| format!("keycode {code}"));
        let mut parts = action.modifiers.clone();
        parts.push(key);
        return Some(parts.join("+"));
    }
    if !binding.name.as_deref()?.starts_with("录制按键序列:") {
        return None;
    }
    let keys: Vec<_> = binding
        .actions
        .iter()
        .filter(|action| action.key_down == Some(true))
        .filter_map(|action| action.keycode)
        .map(|code| {
            keycode_name(code as u16)
                .map(str::to_string)
                .unwrap_or_else(|| format!("keycode {code}"))
        })
        .collect();
    (!keys.is_empty()).then(|| keys.join(" → "))
}

pub fn format_binding_summary(binding: &MacroBinding) -> String {
    if !binding.enabled {
        let name = binding.name.as_deref().unwrap_or("未命名");
        return format!("{name} [停用]");
    }
    if let Some(name) = &binding.name {
        if !name.is_empty() {
            if name == "⚡️ 电池电量" {
                return "设备动作 · 电池电量".to_string();
            }
            return name.clone();
        }
    }
    if binding.actions.is_empty() {
        return "未配置动作".to_string();
    }
    let parts: Vec<String> = binding
        .actions
        .iter()
        .filter_map(|a| {
            if let Some(dev_act) = &a.device_action {
                match dev_act {
                    DeviceAction::BatteryLevel => Some("设备动作 · 电池电量".into()),
                }
            } else if let Some(k) = &a.keys {
                if k == "action:battery" {
                    Some("设备动作 · 电池电量".into())
                } else {
                    Some(k.clone())
                }
            } else if let Some(t) = &a.text {
                Some(format!("\"{t}\""))
            } else {
                None
            }
        })
        .collect();
    if parts.is_empty() {
        format!("{} 个按键动作", binding.actions.len())
    } else {
        parts.join(" → ")
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

    pub fn begin_recording_for_button(&self, button: u32, kind: RecordingKind) -> Result<()> {
        if !self.is_running() {
            return Err(anyhow!("宏引擎尚未运行"));
        }
        let mut state = RECORDING_STATE
            .lock()
            .map_err(|_| anyhow!("录制状态锁已损坏"))?;
        *state = match kind {
            RecordingKind::Shortcut => RecordingState::Shortcut {
                button,
                started: Instant::now(),
                pending: None,
                completed: None,
                mouse_up_pending: false,
            },
            RecordingKind::Sequence => RecordingState::Sequence {
                button,
                last_event: Instant::now(),
                actions: Vec::new(),
                mouse_up_pending: false,
            },
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
        let (tx, rx) = mpsc::channel::<(String, Vec<Action>)>();
        *PLAYBACK_TX.write().unwrap() = Some(tx);
        let playback_thread = std::thread::Builder::new()
            .name("g502-macro-playback".into())
            .spawn(move || {
                let mut raw_serial = RAW_EVENT_SERIAL.load(Ordering::Relaxed);
                loop {
                    match rx.recv_timeout(Duration::from_millis(100)) {
                        Ok((key, actions)) => {
                            mlog(&format!("绑定命中: {key}, 回放 {} 个动作", actions.len()));
                            if let Err(e) = play(&actions) {
                                mlog(&format!("回放失败: {key}: {e}"));
                            } else {
                                mlog(&format!("回放已发送: {key}"));
                            }
                        }
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                    }
                    let current_serial = RAW_EVENT_SERIAL.load(Ordering::Acquire);
                    if current_serial != raw_serial {
                        raw_serial = current_serial;
                        let kind = RAW_EVENT_KIND.load(Ordering::Relaxed);
                        let value_1 = RAW_EVENT_VALUE_1.load(Ordering::Relaxed);
                        let value_2 = RAW_EVENT_VALUE_2.load(Ordering::Relaxed);
                        match kind {
                            1 => mlog(&format!("原始鼠标事件:button={value_1},down")),
                            2 => mlog(&format!("原始鼠标事件:button={value_1},up")),
                            3 => mlog(&format!(
                                "原始滚轮事件:vertical={value_1},horizontal={value_2}"
                            )),
                            _ => {}
                        }
                    }
                }
            })?;

        // 2. 创建 CGEventTap
        let mask = (1u64 << K_CG_EVENT_SCROLL_WHEEL)
            | (1u64 << K_CG_EVENT_OTHER_MOUSE_DOWN)
            | (1u64 << K_CG_EVENT_OTHER_MOUSE_UP)
            | (1u64 << K_CG_EVENT_KEY_DOWN)
            | (1u64 << K_CG_EVENT_KEY_UP)
            | (1u64 << K_CG_EVENT_FLAGS_CHANGED);
        let (tap, tap_location) = unsafe {
            let hid_tap = CGEventTapCreate(
                K_CG_HID_EVENT_TAP,
                0, // head insert
                0, // active tap(可吞键)
                mask,
                tap_callback,
                std::ptr::null_mut(),
            );
            if hid_tap.is_null() {
                (
                    CGEventTapCreate(
                        K_CG_SESSION_EVENT_TAP,
                        0,
                        0,
                        mask,
                        tap_callback,
                        std::ptr::null_mut(),
                    ),
                    K_CG_SESSION_EVENT_TAP,
                )
            } else {
                (hid_tap, K_CG_HID_EVENT_TAP)
            }
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

        mlog(&format!(
            "tap created OK(拦截引擎启动,location={})",
            if tap_location == K_CG_HID_EVENT_TAP {
                "HID"
            } else {
                "session"
            }
        ));
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
                drop(th);

                TAP_PROBE_SEEN.store(false, Ordering::Release);
                unsafe { post_tap_probe()? };
                for _ in 0..20 {
                    if TAP_PROBE_SEEN.load(Ordering::Acquire) {
                        let enabled_keys = self
                            .bindings
                            .read()
                            .map(|bindings| {
                                bindings
                                    .iter()
                                    .filter(|(_, binding)| binding.enabled)
                                    .map(|(key, _)| key.as_str())
                                    .collect::<Vec<_>>()
                                    .join(",")
                            })
                            .unwrap_or_else(|_| "<读取失败>".to_string());
                        mlog(&format!(
                            "tap 自检通过:事件回调链路正常,启用绑定=[{enabled_keys}]"
                        ));
                        return Ok(());
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                mlog("tap 自检失败:事件未进入回调");
                self.stop();
                Err(anyhow!("CGEventTap 自检失败:事件未进入回调"))
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

fn is_tap_disabled_event(etype: u32) -> bool {
    etype == K_CG_EVENT_TAP_DISABLED_BY_TIMEOUT || etype == K_CG_EVENT_TAP_DISABLED_BY_USER_INPUT
}

unsafe fn tap_callback_inner(
    _proxy: *mut c_void,
    etype: u32,
    event: CGEventRef,
    _user_info: *mut c_void,
) -> CGEventRef {
    if is_tap_disabled_event(etype) {
        if let Ok(guard) = TAP_PORT.read() {
            let port = *guard;
            if port != 0 {
                CGEventTapEnable(port as CFMachPortRef, true);
            }
        }
        TAP_RECOVERY_COUNT.fetch_add(1, Ordering::Relaxed);
        return event;
    }

    let event_tag = CGEventGetIntegerValueField(event, K_CG_EVENT_SOURCE_USER_DATA);
    if event_tag == TAP_PROBE_EVENT_TAG {
        TAP_PROBE_SEEN.store(true, Ordering::Release);
        return std::ptr::null_mut();
    }
    if event_tag == PLAYBACK_EVENT_TAG {
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

    if event_type == K_CG_EVENT_SCROLL_WHEEL {
        let vertical = CGEventGetIntegerValueField(event, K_CG_SCROLL_WHEEL_EVENT_DELTA_AXIS_1);
        let horizontal = CGEventGetIntegerValueField(event, K_CG_SCROLL_WHEEL_EVENT_DELTA_AXIS_2);
        let continuous = CGEventGetIntegerValueField(event, K_CG_SCROLL_WHEEL_EVENT_IS_CONTINUOUS);
        let momentum = CGEventGetIntegerValueField(event, K_CG_SCROLL_WHEEL_EVENT_MOMENTUM_PHASE);
        RAW_EVENT_VALUE_1.store(vertical, Ordering::Relaxed);
        RAW_EVENT_VALUE_2.store(horizontal, Ordering::Relaxed);
        RAW_EVENT_KIND.store(3, Ordering::Relaxed);
        RAW_EVENT_SERIAL.fetch_add(1, Ordering::Release);
        let Some(key) = wheel_tilt_key(vertical, horizontal, continuous != 0, momentum != 0) else {
            return event;
        };
        return if queue_binding(key) {
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
    RAW_EVENT_VALUE_1.store(button, Ordering::Relaxed);
    RAW_EVENT_VALUE_2.store(0, Ordering::Relaxed);
    RAW_EVENT_KIND.store(if is_down { 1 } else { 2 }, Ordering::Relaxed);
    RAW_EVENT_SERIAL.fetch_add(1, Ordering::Release);
    if button >= 0 && handle_device_button(button as u32, is_down) {
        std::ptr::null_mut()
    } else {
        event
    }
}

/// 将原生鼠标事件或 HID++ MouseButtonSpy 槽位统一送入录制与回放状态机。
/// 返回 true 表示该按键已被录制器或启用的宏绑定接管。
pub fn handle_device_button(button: u32, is_down: bool) -> bool {
    LAST_BUTTON.store(button as i64, Ordering::Relaxed);
    if record_mouse(button, is_down) {
        return true;
    }

    let Some(key) = button_to_key(button as i64) else {
        return false;
    };
    if is_down {
        queue_binding(key)
    } else {
        binding_enabled(key)
    }
}

fn wheel_tilt_key(
    vertical: i64,
    horizontal: i64,
    continuous: bool,
    momentum: bool,
) -> Option<&'static str> {
    if vertical != 0 || horizontal == 0 || continuous || momentum {
        return None;
    }
    if horizontal < 0 {
        Some("mouse9")
    } else {
        Some("mouse10")
    }
}

fn binding_enabled(key: &str) -> bool {
    GLOBAL_BINDINGS
        .read()
        .ok()
        .and_then(|g| g.as_ref().cloned())
        .and_then(|bindings| bindings.read().ok().and_then(|b| b.get(key).cloned()))
        .is_some_and(|binding| binding.enabled)
}

static LAST_TRIGGER: std::sync::Mutex<Option<std::collections::HashMap<String, std::time::Instant>>> =
    std::sync::Mutex::new(None);

fn queue_binding(key: &str) -> bool {
    let Ok(global_guard) = GLOBAL_BINDINGS.read() else {
        return false;
    };
    let Some(arc_bindings) = global_guard.as_ref() else {
        return false;
    };
    let Ok(bindings_guard) = arc_bindings.read() else {
        return false;
    };
    let Some(binding) = bindings_guard.get(key).filter(|binding| binding.enabled) else {
        return false;
    };
    let actions = binding.actions.clone();
    drop(bindings_guard);
    drop(global_guard);

    // 跨监听通道 (HID++ MouseButtonSpy 与系统 CGEventTap) 快速重复触发去重防抖 (120ms 窗口)
    let now = std::time::Instant::now();
    if let Ok(mut last_map_guard) = LAST_TRIGGER.lock() {
        let map = last_map_guard.get_or_insert_with(std::collections::HashMap::new);
        if let Some(last_time) = map.get(key) {
            let elapsed = now.duration_since(*last_time);
            if elapsed < std::time::Duration::from_millis(120) {
                mlog(&format!(
                    "去重防抖: 忽略 {key} 快速重复触发 (距上次仅 {}ms, 来自并行监听通道)",
                    elapsed.as_millis()
                ));
                // 必须返回 true，通知 CGEventTap 吞掉该事件，阻止系统原生后退/前进/横向滚动
                return true;
            }
        }
        map.insert(key.to_string(), now);
    }

    let Ok(tx_guard) = PLAYBACK_TX.read() else {
        return false;
    };
    let Some(tx) = tx_guard.as_ref() else {
        return false;
    };
    tx.send((key.to_string(), actions)).is_ok()
}

// ---------------------------------------------------------------------- //
// 单元测试
// ---------------------------------------------------------------------- //
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_core_graphics_location_and_source_constants() {
        assert_eq!(K_CG_HID_EVENT_TAP, 0);
        assert_eq!(K_CG_SESSION_EVENT_TAP, 1);
        assert_eq!(K_CG_EVENT_SOURCE_STATE_COMBINED_SESSION_STATE, 0);
    }

    #[test]
    fn test_tap_disabled_event_uses_unsigned_core_graphics_values() {
        assert_eq!(K_CG_EVENT_TAP_DISABLED_BY_TIMEOUT, 0xffff_fffe);
        assert_eq!(K_CG_EVENT_TAP_DISABLED_BY_USER_INPUT, 0xffff_ffff);
        assert!(is_tap_disabled_event(0xffff_fffe));
        assert!(is_tap_disabled_event(0xffff_ffff));
        assert!(!is_tap_disabled_event(K_CG_EVENT_SCROLL_WHEEL as u32));
    }

    #[test]
    fn test_modifier_events_follow_key_press_order() {
        assert_eq!(
            modifier_events(FLAG_CONTROL | FLAG_SHIFT),
            vec![(FLAG_CONTROL, 59), (FLAG_SHIFT, 56)]
        );
        assert!(modifier_events(0).is_empty());
    }

    #[test]
    fn test_wheel_tilt_key_filters_vertical_and_trackpad_scrolling() {
        assert_eq!(wheel_tilt_key(0, -1, false, false), Some("mouse9"));
        assert_eq!(wheel_tilt_key(0, 1, false, false), Some("mouse10"));
        assert_eq!(wheel_tilt_key(1, 0, false, false), None);
        assert_eq!(wheel_tilt_key(1, 1, false, false), None);
        assert_eq!(wheel_tilt_key(0, 0, false, false), None);
        assert_eq!(wheel_tilt_key(0, 1, true, false), None);
        assert_eq!(wheel_tilt_key(0, -1, false, true), None);
    }

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
    fn test_format_recorded_keys() {
        let shortcut = MacroBinding {
            name: Some("录制快捷键: cmd+shift+k".into()),
            enabled: true,
            actions: vec![Action {
                keys: Some("cmd+shift+k".into()),
                ..Action::default()
            }],
        };
        assert_eq!(
            format_recorded_keys(&shortcut).as_deref(),
            Some("cmd+shift+k")
        );

        let sequence = MacroBinding {
            name: Some("录制按键序列: 按键序列（4 个事件）".into()),
            enabled: true,
            actions: vec![
                Action {
                    keycode: Some(8),
                    key_down: Some(true),
                    ..Action::default()
                },
                Action {
                    keycode: Some(8),
                    key_down: Some(false),
                    ..Action::default()
                },
                Action {
                    keycode: Some(40),
                    key_down: Some(true),
                    ..Action::default()
                },
                Action {
                    keycode: Some(40),
                    key_down: Some(false),
                    ..Action::default()
                },
            ],
        };
        assert_eq!(format_recorded_keys(&sequence).as_deref(), Some("c → k"));
        let unknown_shortcut = MacroBinding {
            name: Some("录制快捷键: keycode 200".into()),
            enabled: true,
            actions: vec![Action {
                keycode: Some(200),
                modifiers: vec!["ctrl".into()],
                ..Action::default()
            }],
        };
        assert_eq!(
            format_recorded_keys(&unknown_shortcut).as_deref(),
            Some("ctrl+keycode 200")
        );
        assert_eq!(format_recorded_keys(&default_battery_binding()), None);
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
                    ..Action::default()
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

    #[test]
    fn test_gkey_definitions_and_defaults() {
        assert_eq!(G_KEYS.len(), 8);
        assert_eq!(G_KEYS[0].name, "G4");
        assert_eq!(G_KEYS[0].default_btn, 3);
        assert_eq!(G_KEYS[5].name, "G9");
        assert_eq!(G_KEYS[5].default_btn, 8);
        assert!(G_KEYS[5].is_battery_default);

        let g9 = get_gkey_by_id("g9").expect("find g9 by id");
        assert_eq!(g9.default_btn, 8);
        assert!(g9.is_battery_default);

        let batt_binding = default_battery_binding();
        assert!(batt_binding.enabled);
        assert_eq!(
            batt_binding.actions[0].device_action,
            Some(DeviceAction::BatteryLevel)
        );
        assert_eq!(batt_binding.actions[0].keys, None);
        assert_eq!(format_binding_summary(&batt_binding), "设备动作 · 电池电量");
    }

    #[test]
    fn test_text_binding_summary_and_extraction() {
        let text_binding = MacroBinding {
            name: Some("文字: \"hello world\"".into()),
            enabled: true,
            actions: vec![Action {
                keys: None,
                text: Some("hello world".into()),
                keycode: None,
                delay_ms: None,
                key_down: None,
                modifiers: Vec::new(),
                device_action: None,
            }],
        };
        assert_eq!(
            get_binding_text(&text_binding),
            Some("hello world".to_string())
        );
        assert_eq!(
            format_binding_summary(&text_binding),
            "文字: \"hello world\""
        );
    }

    static TEST_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn test_save_text_binding_preserves_spaces_and_whitespace_only() {
        let _guard = TEST_ENV_LOCK.lock().unwrap();
        let temp_dir =
            std::env::temp_dir().join(format!("g502hub_test_spaces_{}", std::process::id()));
        let orig_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", &temp_dir);

        // 1. 带有首尾空格的字符串必须完整保留
        save_text_binding("mouse4", "  ls -la  ").expect("save text with spaces");
        let b = get_binding_for_key("mouse4").expect("found mouse4");
        assert_eq!(get_binding_text(&b), Some("  ls -la  ".to_string()));
        assert_eq!(format_binding_summary(&b), "文字: \"  ls -la  \"");

        // 2. 纯空格字符串必须保留且不被当作空清除
        save_text_binding("mouse5", "   ").expect("save whitespace only text");
        let b2 = get_binding_for_key("mouse5").expect("found mouse5");
        assert_eq!(get_binding_text(&b2), Some("   ".to_string()));
        assert_eq!(format_binding_summary(&b2), "文字: \"   \"");

        // 3. 真正空字符串则清除绑定
        save_text_binding("mouse4", "").expect("save empty text clears");
        assert_eq!(get_binding_for_key("mouse4"), None);

        // 清理测试临时环境
        let _ = std::fs::remove_dir_all(&temp_dir);
        if let Some(h) = orig_home {
            std::env::set_var("HOME", h);
        }
    }

    #[test]
    fn test_recorded_binding_cannot_be_saved_as_display_text() {
        let shortcut = MacroBinding {
            name: Some("录制快捷键: cmd+shift+k".into()),
            enabled: true,
            actions: vec![Action {
                keys: Some("cmd+shift+k".into()),
                ..Action::default()
            }],
        };
        assert_eq!(
            format_recorded_keys(&shortcut).as_deref(),
            Some("cmd+shift+k")
        );
        assert!(!can_save_text_binding(false, Some(&shortcut)));
        assert!(!can_save_text_binding(true, Some(&shortcut)));
        assert!(!can_save_text_binding(
            true,
            Some(&default_battery_binding())
        ));

        let text = MacroBinding {
            name: Some("文字: \" hello \"".into()),
            enabled: true,
            actions: vec![Action {
                text: Some(" hello ".into()),
                ..Action::default()
            }],
        };
        assert!(can_save_text_binding(true, Some(&text)));
        assert!(!can_save_text_binding(false, Some(&text)));
        assert!(can_save_text_binding(true, None));
    }

    #[test]
    fn test_battery_action_is_not_text() {
        let batt_binding = default_battery_binding();
        assert_eq!(get_binding_text(&batt_binding), None);

        let custom_battery = MacroBinding {
            name: None,
            enabled: true,
            actions: vec![Action {
                device_action: Some(DeviceAction::BatteryLevel),
                text: Some("should be ignored".into()),
                ..Action::default()
            }],
        };
        assert_eq!(get_binding_text(&custom_battery), None);
    }

    #[test]
    fn test_battery_binding_formatting_summary() {
        // Typed device action with explicit name
        let b1 = default_battery_binding();
        assert_eq!(format_binding_summary(&b1), "设备动作 · 电池电量");

        // Typed device action without name
        let b2 = MacroBinding {
            name: None,
            enabled: true,
            actions: vec![Action {
                device_action: Some(DeviceAction::BatteryLevel),
                ..Action::default()
            }],
        };
        assert_eq!(format_binding_summary(&b2), "设备动作 · 电池电量");

        // Legacy key binding formatting
        let b3 = MacroBinding {
            name: None,
            enabled: true,
            actions: vec![Action {
                keys: Some("action:battery".into()),
                ..Action::default()
            }],
        };
        assert_eq!(format_binding_summary(&b3), "设备动作 · 电池电量");

        // Legacy name update in formatting
        let b4 = MacroBinding {
            name: Some("⚡️ 电池电量".into()),
            enabled: true,
            actions: vec![Action {
                device_action: Some(DeviceAction::BatteryLevel),
                ..Action::default()
            }],
        };
        assert_eq!(format_binding_summary(&b4), "设备动作 · 电池电量");
    }

    #[test]
    fn test_clear_and_reset_battery_binding() {
        let _guard = TEST_ENV_LOCK.lock().unwrap();
        let temp_dir = std::env::temp_dir().join(format!("g502hub_test_{}", std::process::id()));
        let orig_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", &temp_dir);

        // 1. Initial default load: mouse8 has typed battery action
        let cfg = crate::config::load().expect("load config");
        assert!(cfg.macros.contains_key("mouse8"));

        let b = get_binding_for_key("mouse8").expect("found mouse8 binding");
        assert_eq!(b.actions[0].device_action, Some(DeviceAction::BatteryLevel));

        // 2. Clear binding: mouse8 is removed and get_binding_for_key returns None (no synthetic fallback)
        clear_binding("mouse8").expect("clear mouse8");
        assert_eq!(get_binding_for_key("mouse8"), None);
        assert_eq!(get_binding_for_key("g9"), None);

        // 3. Reset binding via save_battery_binding: typed battery action restored
        save_battery_binding("mouse8").expect("reset mouse8 battery");
        let b_reset = get_binding_for_key("mouse8").expect("found mouse8 after reset");
        assert_eq!(
            b_reset.actions[0].device_action,
            Some(DeviceAction::BatteryLevel)
        );
        assert_eq!(b_reset.actions[0].keys, None);
        assert_eq!(format_binding_summary(&b_reset), "设备动作 · 电池电量");

        // Clean up
        let _ = std::fs::remove_dir_all(&temp_dir);
        if let Some(h) = orig_home {
            std::env::set_var("HOME", h);
        }
    }

    #[test]
    fn test_macro_tap_no_synthetic_g9_fallback() {
        let bindings = BTreeMap::new();
        // Mouse8 not in bindings: should not be synthesized
        let tap = MacroTap::new(bindings);
        {
            let b = tap.bindings();
            let guard = b.read().unwrap();
            assert!(!guard.contains_key("mouse8"));
        }
    }

    #[test]
    fn test_queue_binding_cross_channel_deduplication() {
        let (tx, rx) = mpsc::channel::<(String, Vec<Action>)>();
        *PLAYBACK_TX.write().unwrap() = Some(tx);

        let mut bindings = BTreeMap::new();
        bindings.insert(
            "mouse3".to_string(),
            MacroBinding {
                name: Some("测试".into()),
                enabled: true,
                actions: vec![Action {
                    keys: Some("cmd+c".into()),
                    ..Action::default()
                }],
            },
        );
        *GLOBAL_BINDINGS.write().unwrap() = Some(Arc::new(RwLock::new(bindings)));

        // 第一次触发（来自通道 A，如 MouseButtonSpy）
        assert!(queue_binding("mouse3"));
        assert!(rx.try_recv().is_ok());

        // 快速第二次触发（来自通道 B，如 CGEventTap，间隔很短）
        // 应该被去重，但返回 true 告知调用方吞掉系统事件
        assert!(queue_binding("mouse3"));
        assert!(rx.try_recv().is_err(), "短时间内重复触发不应向队列发送二次动作");

        // 模拟等待超出防抖窗口后再次触发
        if let Ok(mut last_map_guard) = LAST_TRIGGER.lock() {
            if let Some(map) = last_map_guard.as_mut() {
                map.insert("mouse3".to_string(), Instant::now() - Duration::from_millis(150));
            }
        }
        assert!(queue_binding("mouse3"));
        assert!(rx.try_recv().is_ok(), "超出防抖窗口后应正常接收下一次触发");

        // 清理测试资源
        *PLAYBACK_TX.write().unwrap() = None;
        *GLOBAL_BINDINGS.write().unwrap() = None;
    }
}
