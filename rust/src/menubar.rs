//! 菜单栏常驻应用。
//!
//! 架构:主线程跑 NSApplication.run() + CFRunLoopTimer(0.2s)泵。
//! Timer 回调:轮询菜单事件 → 执行动作;状态变化 → **原地更新**菜单项文本/勾选
//! (绝不整体替换菜单,避免替换打开中的菜单;也绝不在持锁状态下嵌套加锁)。
//! 电量轮询在独立线程,只写共享 State。

use crate::config::{self, Config, DesiredMode, MacroBinding};
use crate::controller;
use crate::device::{
    connection_interface_unchanged, g502_interface_present, ghub_agent_running,
    invalidate_connection, G502Device,
};
use crate::features::battery::{read_battery, BatteryInfo};
use crate::features::dpi::Dpi;
use crate::features::onboard::OnboardMode;
use crate::macro_engine::{
    accessibility_granted, recording_outcome_pending, recording_phase, recording_serial,
    take_recording_outcome, MacroTap, RecordingKind, RecordingOutcome, RecordingPhase,
    RecordingResult,
};
use anyhow::Result;
use core_foundation_sys::date::CFAbsoluteTimeGetCurrent;
use core_foundation_sys::runloop::{
    kCFRunLoopCommonModes, CFRunLoopAddTimer, CFRunLoopGetMain, CFRunLoopTimerContext,
    CFRunLoopTimerCreate, CFRunLoopTimerRef,
};
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
use objc2_foundation::MainThreadMarker;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use tray_icon::{Icon, MouseButton, TrayIcon, TrayIconBuilder, TrayIconEvent};

static QUIT: AtomicBool = AtomicBool::new(false);
static ACTION_TX: std::sync::OnceLock<mpsc::Sender<String>> = std::sync::OnceLock::new();

pub fn dispatch_menu_action(id: &str) -> bool {
    ACTION_TX
        .get()
        .is_some_and(|tx| tx.send(id.to_string()).is_ok())
}

/// 菜单预设色(任意颜色走"自定义颜色…"调起系统取色器)。
pub const LED_COLORS: &[(&str, &str)] = &[
    ("白", "ffffff"),
    ("红", "ff0000"),
    ("橙", "ff6600"),
    ("黄", "ffcc00"),
    ("绿", "00dd00"),
    ("青", "00c8ff"),
    ("蓝", "2244ff"),
    ("紫", "aa00ff"),
    ("品红", "ff0082"),
    ("粉", "ff69b4"),
];

// ---------------------------------------------------------------------- //
#[derive(Default, Clone)]
struct Snapshot {
    device_desc: Option<String>,
    battery: Option<BatteryInfo>,
    dpi: Option<u16>,
    mode: Option<OnboardMode>,
    error: Option<String>,
}

struct State {
    snap: Snapshot,
    dirty: bool,
}

/// 动作执行核心(工作线程持有,不碰 UI)
struct Core {
    state: Arc<Mutex<State>>,
    tap: Arc<MacroTap>,
    cfg: Config,
    led_sync_pending: Arc<AtomicBool>,
}

struct App {
    state: Arc<Mutex<State>>,
    tray: TrayIcon,
    tray_visible: bool,
    ticks: u32,
    tap: Arc<MacroTap>,
    cfg: Config,
    // ---- 常驻菜单项(原地更新) ----
    header: MenuItem,
    battery_line: MenuItem,
    mode_line: MenuItem,
    mode_action: MenuItem,
    dpi_items: Vec<CheckMenuItem>,
    macro_status: MenuItem,
    macro_g4: MenuItem,
    macro_g5: MenuItem,
    macro_record_shortcut: MenuItem,
    macro_record_sequence: MenuItem,
    macro_finish_recording: MenuItem,
    macro_cancel_recording: MenuItem,
    macro_toggle: MenuItem,
    last_recording_serial: u64,
    led_rate_items: Vec<CheckMenuItem>,
    led_bright_items: Vec<CheckMenuItem>,
    ghub_item: MenuItem,
}

impl App {
    fn build_menu(&mut self) -> Result<()> {
        let menu = Menu::new();
        self.header = MenuItem::with_id("noop", "G502 未连接", false, None);
        self.battery_line = MenuItem::with_id("noop", "", false, None);
        self.mode_line = MenuItem::with_id("noop", "", false, None);
        self.mode_action = MenuItem::with_id("mode:toggle", "切换控制模式", false, None);
        let panel_item = MenuItem::with_id("panel:open", "✨ 打开控制中心浮窗…", true, None);
        let _ = menu.append(&panel_item);
        let _ = menu.append(&PredefinedMenuItem::separator());
        let _ = menu.append(&self.header);
        let _ = menu.append(&self.battery_line);
        let _ = menu.append(&self.mode_line);
        let _ = menu.append(&self.mode_action);
        let _ = menu.append(&PredefinedMenuItem::separator());

        let dpi_sub = Submenu::new("灵敏度 DPI", true);
        self.dpi_items.clear();
        for lvl in &self.cfg.dpi_levels {
            let item =
                CheckMenuItem::with_id(format!("dpi:{lvl}"), format!("{lvl}"), true, false, None);
            let _ = dpi_sub.append(&item);
            self.dpi_items.push(item);
        }
        let _ = menu.append(&dpi_sub);

        if !self.cfg.profiles.is_empty() {
            let sub = Submenu::new("配置档", true);
            for name in self.cfg.profiles.keys() {
                let _ = sub.append(&MenuItem::with_id(
                    format!("profile:{name}"),
                    name.clone(),
                    true,
                    None,
                ));
            }
            let _ = menu.append(&sub);
        }

        let led_sub = Submenu::new("RGB 灯效", true);
        self.led_rate_items.clear();
        self.led_bright_items.clear();
        for (zone_key, zone_label) in [("primary", "主要"), ("logo", "标志")] {
            let zone_menu = Submenu::new(zone_label, true);
            let _ = zone_menu.append(&MenuItem::with_id(
                format!("led:{zone_key}:off"),
                "关闭",
                true,
                None,
            ));
            let _ = zone_menu.append(&MenuItem::with_id(
                format!("led:{zone_key}:cycle"),
                "循环",
                true,
                None,
            ));
            let _ = zone_menu.append(&PredefinedMenuItem::separator());
            for (effect, label) in [("solid", "固定色"), ("breathing", "呼吸")] {
                let color_sub = Submenu::new(label, true);
                for (name, hex) in LED_COLORS {
                    let _ = color_sub.append(&MenuItem::with_id(
                        format!("led:{zone_key}:{effect}:{hex}"),
                        name,
                        true,
                        None,
                    ));
                }
                let _ = color_sub.append(&PredefinedMenuItem::separator());
                let _ = color_sub.append(&MenuItem::with_id(
                    format!("led:{zone_key}:custom:{effect}"),
                    "自定义颜色…",
                    true,
                    None,
                ));
                let _ = zone_menu.append(&color_sub);
            }
            let _ = zone_menu.append(&PredefinedMenuItem::separator());
            let rate_sub = Submenu::new("速率", true);
            for ms in [1000u16, 2000, 3000, 5000, 8000, 10000, 15000, 20000] {
                let item = CheckMenuItem::with_id(
                    format!("led:{zone_key}:rate:{ms}"),
                    format!("{ms}ms"),
                    true,
                    false,
                    None,
                );
                let _ = rate_sub.append(&item);
                self.led_rate_items.push(item);
            }
            let _ = zone_menu.append(&rate_sub);
            let bright_sub = Submenu::new("亮度", true);
            for percent in [25u8, 50, 75, 100] {
                let item = CheckMenuItem::with_id(
                    format!("led:{zone_key}:bright:{percent}"),
                    format!("{percent}%"),
                    true,
                    false,
                    None,
                );
                let _ = bright_sub.append(&item);
                self.led_bright_items.push(item);
            }
            let _ = zone_menu.append(&bright_sub);
            let _ = led_sub.append(&zone_menu);
        }
        let _ = menu.append(&led_sub);

        let _ = menu.append(&PredefinedMenuItem::separator());
        self.macro_status = MenuItem::with_id("noop", "侧键宏:未启用", false, None);
        self.macro_g4 = MenuItem::with_id("noop", "G4 / 后退(button3): 未绑定", false, None);
        self.macro_g5 = MenuItem::with_id("noop", "G5 / 前进(button4): 未绑定", false, None);
        self.macro_record_shortcut =
            MenuItem::with_id("macro:record-shortcut", "录制快捷键宏…", true, None);
        self.macro_record_sequence =
            MenuItem::with_id("macro:record-sequence", "录制按键序列宏…", true, None);
        self.macro_finish_recording =
            MenuItem::with_id("macro:finish-recording", "结束并保存序列", false, None);
        self.macro_cancel_recording =
            MenuItem::with_id("macro:cancel-recording", "取消录制", false, None);
        self.macro_toggle = MenuItem::with_id("macro:toggle", "启用宏引擎", true, None);
        let _ = menu.append(&self.macro_status);
        let _ = menu.append(&self.macro_g4);
        let _ = menu.append(&self.macro_g5);
        let _ = menu.append(&self.macro_record_shortcut);
        let _ = menu.append(&self.macro_record_sequence);
        let _ = menu.append(&self.macro_finish_recording);
        let _ = menu.append(&self.macro_cancel_recording);
        let _ = menu.append(&self.macro_toggle);

        self.ghub_item = MenuItem::with_id("ghub:quit", "G HUB: 未运行", false, None);
        let _ = menu.append(&self.ghub_item);

        let _ = menu.append(&MenuItem::with_id(
            "config:open",
            "打开配置文件",
            true,
            None,
        ));
        let _ = menu.append(&MenuItem::with_id("app:quit", "退出 g502hub", true, None));

        self.tray.set_menu(Some(Box::new(menu)));
        Ok(())
    }

    /// 状态变化后**原地刷新**菜单项(不重建、不替换)。
    fn refresh_menu(&self, snap: &Snapshot) {
        match (&snap.device_desc, &snap.battery) {
            (Some(desc), Some(b)) => {
                let volt = b
                    .voltage_mv
                    .map(|v| format!(" · {v}mV"))
                    .unwrap_or_default();
                let icon = if b.charging { "⚡️" } else { "🔋" };
                self.header.set_text(desc.clone());
                self.battery_line
                    .set_text(format!("{icon} {}% {}{volt}", b.percent, b.state_text));
            }
            (Some(desc), None) => {
                self.header.set_text(desc.clone());
                self.battery_line.set_text("");
            }
            _ => {
                self.header.set_text("G502 未连接".to_string());
                self.battery_line.set_text("");
            }
        }
        match snap.mode {
            Some(OnboardMode::Onboard) => {
                self.mode_line.set_text("模式: 板载控制 · 固件配置接管");
                self.mode_action.set_text("切换到主机控制模式");
                self.mode_action.set_enabled(true);
            }
            Some(OnboardMode::Host) => {
                self.mode_line.set_text("模式: 主机控制 · 自动恢复 DPI");
                self.mode_action.set_text("切换到板载模式");
                self.mode_action.set_enabled(true);
            }
            None => {
                self.mode_line.set_text("");
                self.mode_action.set_enabled(false);
            }
        }
        for (i, lvl) in self.cfg.dpi_levels.iter().enumerate() {
            if let Some(item) = self.dpi_items.get(i) {
                item.set_checked(snap.dpi == Some(*lvl));
            }
        }

        // 同步状态到 PopoverPanel (紧凑优雅展示，防止超出头部约束)
        let battery_str = snap
            .battery
            .as_ref()
            .map(|b| {
                let icon = if b.charging { "⚡️" } else { "🔋" };
                format!("{icon} {}%", b.percent)
            })
            .unwrap_or_else(|| "🔋 未连接".into());
        let mode_str = match snap.mode {
            Some(OnboardMode::Onboard) => "板载控制",
            Some(OnboardMode::Host) => "主机控制",
            None => "未知",
        };
        crate::panel::PopoverPanel::sync_status(&battery_str, mode_str, snap.dpi);

        let cfg = config::load().unwrap_or_else(|_| self.cfg.clone());
        let binding_text = |key: &str| {
            cfg.macros
                .get(key)
                .map(|m| {
                    let name = m.name.clone().unwrap_or_else(|| "未命名".into());
                    format!("{name} [{}]", if m.enabled { "启用" } else { "停用" })
                })
                .unwrap_or_else(|| "未绑定".into())
        };
        self.macro_g4
            .set_text(format!("G4 / 后退(button3): {}", binding_text("mouse3")));
        self.macro_g5
            .set_text(format!("G5 / 前进(button4): {}", binding_text("mouse4")));
        let running = self.tap.is_running();
        let phase = recording_phase();
        let recording = !matches!(phase, RecordingPhase::Idle);
        self.macro_record_shortcut.set_enabled(!recording);
        self.macro_record_sequence.set_enabled(!recording);
        self.macro_finish_recording
            .set_enabled(matches!(phase, RecordingPhase::Sequence { .. }));
        self.macro_cancel_recording.set_enabled(recording);
        self.macro_toggle.set_enabled(!recording);

        if running {
            self.tap.update_bindings(cfg.macros.clone());
            self.macro_toggle.set_text("停用宏引擎");
            match phase {
                RecordingPhase::AwaitMouse(RecordingKind::Shortcut) => {
                    self.macro_status.set_text("录制快捷键:请按目标鼠标侧键");
                }
                RecordingPhase::AwaitMouse(RecordingKind::Sequence) => {
                    self.macro_status.set_text("录制序列:请按目标鼠标侧键");
                }
                RecordingPhase::Shortcut { button } => {
                    self.macro_status
                        .set_text(format!("mouse{button} 已选:请按一次键盘组合"));
                }
                RecordingPhase::Sequence { button, events } => {
                    self.macro_status
                        .set_text(format!("mouse{button} 序列录制中 · {events} 个事件"));
                }
                RecordingPhase::Idle => {
                    let count = cfg.macros.values().filter(|m| m.enabled).count();
                    let recent = self
                        .tap
                        .last_button()
                        .map(|b| match b {
                            3 => " · 最近 G4/button3".to_string(),
                            4 => " · 最近 G5/button4".to_string(),
                            _ => format!(" · 最近 button{b}"),
                        })
                        .unwrap_or_default();
                    self.macro_status
                        .set_text(format!("侧键宏:运行中({count} 个绑定){recent}"));
                }
            }
        } else if !accessibility_granted(false) {
            self.macro_status.set_text("侧键宏:需要辅助功能权限");
            self.macro_toggle.set_text("启用宏引擎");
        } else {
            self.macro_status.set_text("侧键宏:未启用");
            self.macro_toggle.set_text("启用宏引擎");
        }
        if ghub_agent_running() {
            self.ghub_item.set_text("⚠️ G HUB 运行中 → 退出它");
            self.ghub_item.set_enabled(true);
        } else {
            self.ghub_item.set_text("G HUB: 未运行");
            self.ghub_item.set_enabled(false);
        }
        // 分区灯效选中态:id 形如 led:<zone>:rate:<v> / led:<zone>:bright:<n>
        let default_spec = || config::LedSpec::new("solid", [255, 255, 255], 100, Some("medium"));
        for item in &self.led_rate_items {
            let parts: Vec<&str> = item.id().0.split(':').collect();
            if let [_, zone_key, _, value] = parts[..] {
                let spec = cfg
                    .led_zones
                    .get(zone_key)
                    .cloned()
                    .unwrap_or_else(default_spec);
                item.set_checked(spec.rate.as_deref() == Some(value));
            }
        }
        for item in &self.led_bright_items {
            let parts: Vec<&str> = item.id().0.split(':').collect();
            if let [_, zone_key, _, value] = parts[..] {
                let spec = cfg
                    .led_zones
                    .get(zone_key)
                    .cloned()
                    .unwrap_or_else(default_spec);
                item.set_checked(spec.brightness == value.parse::<u8>().unwrap_or(0));
            }
        }
    }
}

impl Core {
    fn start_recording(&self, kind: RecordingKind) {
        if !accessibility_granted(false) {
            accessibility_granted(true);
            self.notify("请先在系统设置中允许 g502hub 辅助功能权限");
            return;
        }
        if !self.tap.is_running() {
            if let Err(e) = self.tap.start() {
                self.notify(&format!("宏引擎启动失败: {e}"));
                return;
            }
        }
        match self.tap.begin_recording(kind) {
            Ok(()) => self.notify(match kind {
                RecordingKind::Shortcut => "请按要绑定的鼠标侧键，然后按键盘组合",
                RecordingKind::Sequence => "请按要绑定的鼠标侧键，然后开始键盘录制",
            }),
            Err(e) => self.notify(&format!("开始录制失败: {e}")),
        }
    }

    fn start_recording_for_button(&self, button: u32, kind: RecordingKind) {
        if !accessibility_granted(false) {
            accessibility_granted(true);
            self.notify("请先在系统设置中允许 g502hub 辅助功能权限");
            return;
        }
        if !self.tap.is_running() {
            if let Err(e) = self.tap.start() {
                self.notify(&format!("宏引擎启动失败: {e}"));
                return;
            }
        }
        let button_name = if let Some(gk) = crate::macro_engine::get_gkey_by_button(button) {
            format!("{} ({})", gk.name, gk.desc)
        } else {
            format!("button{button}")
        };
        match self.tap.begin_recording_for_button(button, kind) {
            Ok(()) => self.notify(&format!("正在录制 {button_name}，请按一次键盘组合键")),
            Err(e) => self.notify(&format!("开始录制失败: {e}")),
        }
    }

    fn save_recording(&self, result: RecordingResult) {
        let key = format!("mouse{}", result.button);
        let replaced = config::load()
            .ok()
            .and_then(|cfg| cfg.macros.get(&key).cloned())
            .is_some();
        let kind_name = match result.kind {
            RecordingKind::Shortcut => "快捷键",
            RecordingKind::Sequence => "按键序列",
        };
        let binding = MacroBinding {
            name: Some(format!("录制{kind_name}: {}", result.label)),
            enabled: true,
            actions: result.actions,
        };
        let macros = match config::update(|cfg| {
            cfg.macros.insert(key.clone(), binding);
            cfg.macros.clone()
        }) {
            Ok(macros) => macros,
            Err(e) => {
                self.notify(&format!("保存宏失败: {e}"));
                return;
            }
        };
        self.tap.update_bindings(macros);
        let button = match result.button {
            3 => "G4 / mouse3".to_string(),
            4 => "G5 / mouse4".to_string(),
            n => format!("button{n} / mouse{n}"),
        };
        self.notify(&format!(
            "{button} 已{}为 {}",
            if replaced { "替换" } else { "绑定" },
            result.label
        ));
    }

    fn handle_recording_outcome(&self, outcome: RecordingOutcome) {
        match outcome {
            RecordingOutcome::Completed(result) => self.save_recording(result),
            RecordingOutcome::Cancelled(reason) => self.notify(&format!("录制已取消: {reason}")),
        }
        if let Ok(mut st) = self.state.lock() {
            st.dirty = true;
        }
    }

    fn handle_led_menu(self: &Arc<Self>, arg: &str) {
        let segs: Vec<&str> = arg.split(':').collect();
        let Some(zone) = segs
            .first()
            .copied()
            .and_then(crate::features::led::zone_from_key)
        else {
            return;
        };
        let key = crate::features::led::zone_key(zone).to_string();
        let mut spec = config::load()
            .ok()
            .and_then(|cfg| cfg.led_zones.get(&key).cloned())
            .unwrap_or_else(|| config::LedSpec::new("solid", [255, 255, 255], 100, Some("medium")));
        match &segs[1..] {
            ["off"] => {
                spec.turn_off();
            }
            ["solid", hex] => {
                spec.rgb = hex_rgb(hex);
                spec.effect = "solid".into();
                spec.turn_on();
            }
            ["breathing", hex] => {
                spec.rgb = hex_rgb(hex);
                spec.effect = "breathing".into();
                spec.turn_on();
            }
            ["custom", effect] => {
                let effect = (*effect).to_string();
                let core = Arc::clone(self);
                // 取色器对话框会阻塞到用户确认,放到独立线程,不卡菜单轮询
                std::thread::spawn(move || core.pick_custom_color(zone, &effect));
                return;
            }
            [effect] => {
                spec.effect = (*effect).into();
                spec.turn_on();
            }
            ["rate", value] => {
                spec.rate = Some((*value).into());
                if spec.off {
                    let saved = config::update(|cfg| {
                        cfg.led_zones.insert(key.clone(), spec.clone());
                    });
                    if let Err(e) = saved {
                        self.notify(&format!("保存灯效失败: {e}"));
                        return;
                    }
                    self.notify("速率已保存,开启呼吸/循环后生效");
                    if let Ok(mut st) = self.state.lock() {
                        st.dirty = true;
                    }
                    return;
                }
            }
            ["bright", value] => {
                if let Ok(v) = value.parse::<u8>() {
                    spec.brightness = v;
                }
            }
            _ => return,
        }
        if let Err(e) = config::update(|cfg| {
            cfg.led_zones.insert(key, spec.clone());
        }) {
            self.notify(&format!("保存灯效失败: {e}"));
            return;
        }
        let apply = crate::device::get_conn(1)
            .and_then(|dev| controller::apply_led_spec(&dev, zone, &spec));
        match apply {
            Ok(()) => {
                let summary = if spec.off {
                    "关闭".to_string()
                } else {
                    format!(
                        "{} 亮度{}% 速率 {}",
                        spec.effect,
                        spec.brightness,
                        spec.rate.as_deref().unwrap_or("默认")
                    )
                };
                self.notify(&format!(
                    "{} 灯效已更新: {summary}",
                    crate::features::led::zone_label(zone)
                ));
            }
            Err(e) => {
                self.led_sync_pending.store(true, Ordering::Release);
                self.notify(&format!("灯效暂未同步，将自动重试: {e}"));
            }
        }
        if let Ok(mut st) = self.state.lock() {
            st.dirty = true;
        }
    }

    /// 弹出 macOS 系统取色器(阻塞在独立线程),选定后应用并保存。
    fn pick_custom_color(self: &Arc<Self>, zone: u8, effect: &str) {
        let key = crate::features::led::zone_key(zone);
        let default_rgb = config::load()
            .ok()
            .and_then(|cfg| cfg.led_zones.get(key).cloned())
            .map(|s| s.rgb)
            .unwrap_or([255, 255, 255]);
        let to65k = |c: u8| (c as u16) * 257;
        let script = format!(
            "choose color default color {{{}, {}, {}}}",
            to65k(default_rgb[0]),
            to65k(default_rgb[1]),
            to65k(default_rgb[2])
        );
        let Ok(out) = std::process::Command::new("osascript")
            .arg("-e")
            .arg(&script)
            .output()
        else {
            self.notify("无法打开系统取色器");
            return;
        };
        if !out.status.success() {
            return; // 用户取消
        }
        let text = String::from_utf8_lossy(&out.stdout);
        let nums: Vec<u16> = text
            .trim()
            .trim_start_matches('{')
            .trim_end_matches('}')
            .split(',')
            .filter_map(|p| p.trim().parse().ok())
            .collect();
        if nums.len() != 3 {
            self.notify("取色结果解析失败");
            return;
        }
        let rgb = [
            (nums[0] / 257) as u8,
            (nums[1] / 257) as u8,
            (nums[2] / 257) as u8,
        ];
        let mut spec = config::load()
            .ok()
            .and_then(|cfg| cfg.led_zones.get(key).cloned())
            .unwrap_or_else(|| config::LedSpec::new(effect, rgb, 100, Some("medium")));
        spec.effect = effect.to_string();
        spec.rgb = rgb;
        spec.turn_on();
        if let Err(e) = config::update(|cfg| {
            cfg.led_zones.insert(key.to_string(), spec.clone());
        }) {
            self.notify(&format!("保存灯效失败: {e}"));
            return;
        }
        let r = crate::device::get_conn(1)
            .and_then(|dev| controller::apply_led_spec(&dev, zone, &spec));
        match r {
            Ok(()) => {
                self.notify(&format!(
                    "{} 灯效已更新: {} #{:02x}{:02x}{:02x} 亮度{}%",
                    crate::features::led::zone_label(zone),
                    spec.effect,
                    rgb[0],
                    rgb[1],
                    rgb[2],
                    spec.brightness
                ));
            }
            Err(e) => {
                self.led_sync_pending.store(true, Ordering::Release);
                self.notify(&format!("灯效暂未同步，将自动重试: {e}"));
            }
        }
        if let Ok(mut st) = self.state.lock() {
            st.dirty = true;
        }
    }

    fn handle(self: &Arc<Self>, id: &str) {
        let parts: Vec<&str> = id.splitn(2, ':').collect();
        let (kind, arg) = (parts[0], parts.get(1).copied().unwrap_or(""));
        match (kind, arg) {
            ("noop", _) => {}
            ("panel", "open") => {
                crate::panel::PopoverPanel::request_show();
            }
            ("dpi", v) => {
                if let Ok(val) = v.parse::<u16>() {
                    let r = crate::device::get_conn(2)
                        .and_then(|dev| controller::set_dpi_confirmed(&dev, val));
                    match r {
                        Ok(actual) => {
                            let _ = config::update(|cfg| {
                                cfg.desired_mode = DesiredMode::Host;
                                cfg.desired_dpi = Some(actual);
                            });
                            if let Ok(mut st) = self.state.lock() {
                                st.snap.dpi = Some(actual);
                                st.snap.mode = Some(OnboardMode::Host);
                                st.dirty = true;
                            }
                            self.notify(&format!("DPI 已确认: {actual}"));
                        }
                        Err(e) => self.notify(&format!("DPI 设置失败: {e}")),
                    }
                }
            }
            ("profile", name) => {
                let Some(p) = self.cfg.profiles.get(name).cloned() else {
                    return;
                };
                let target_dpi = p.active_dpi.or_else(|| p.dpi_levels.last().copied());
                let r = crate::device::get_conn(2).and_then(|dev| {
                    let mode = controller::set_mode_confirmed(&dev, OnboardMode::Host)?;
                    let dpi = target_dpi
                        .map(|v| Dpi::new(&dev).and_then(|d| d.set_dpi(v)))
                        .transpose()?;
                    if let Some(led) = &p.led {
                        controller::apply_led_spec(&dev, crate::features::led::ZONE_PRIMARY, led)?;
                    }
                    Ok((mode, dpi))
                });
                match r {
                    Ok((mode, dpi)) => {
                        let _ = config::update(|cfg| {
                            cfg.desired_mode = DesiredMode::Host;
                            if let Some(dpi) = dpi {
                                cfg.desired_dpi = Some(dpi);
                            }
                        });
                        if let Ok(mut st) = self.state.lock() {
                            st.snap.mode = Some(mode);
                            st.snap.dpi = dpi;
                            st.dirty = true;
                        }
                        self.notify(&format!("已确认配置档「{name}」"));
                    }
                    Err(e) => self.notify(&format!("应用失败: {e}")),
                }
            }
            ("led", arg) => self.handle_led_menu(arg),
            ("mode", "toggle") => {
                let current = self.state.lock().ok().and_then(|st| st.snap.mode);
                let target = if current == Some(OnboardMode::Host) {
                    OnboardMode::Onboard
                } else {
                    OnboardMode::Host
                };
                let r = crate::device::get_conn(2).and_then(|dev| {
                    let actual = controller::set_mode_confirmed(&dev, target)?;
                    let dpi = if actual == OnboardMode::Host {
                        let cfg = config::load().unwrap_or_else(|_| self.cfg.clone());
                        cfg.desired_dpi
                            .map(|v| Dpi::new(&dev).and_then(|d| d.set_dpi(v)))
                            .transpose()?
                    } else {
                        Dpi::new(&dev).and_then(|d| d.get_dpi()).ok()
                    };
                    Ok((actual, dpi))
                });
                match r {
                    Ok((actual, dpi)) => {
                        let _ = config::update(|cfg| {
                            cfg.desired_mode = if actual == OnboardMode::Host {
                                DesiredMode::Host
                            } else {
                                DesiredMode::Onboard
                            };
                        });
                        if actual == OnboardMode::Host {
                            self.led_sync_pending.store(true, Ordering::Release);
                        } else {
                            self.led_sync_pending.store(false, Ordering::Release);
                        }
                        if let Ok(mut st) = self.state.lock() {
                            st.snap.mode = Some(actual);
                            st.snap.dpi = dpi;
                            st.dirty = true;
                        }
                        self.notify(&format!("模式已确认: {}", actual.name()));
                    }
                    Err(e) => self.notify(&format!("模式切换失败: {e}")),
                }
            }
            ("macro", "record-shortcut") => {
                self.start_recording(RecordingKind::Shortcut);
            }
            ("macro", "record-sequence") => {
                self.start_recording(RecordingKind::Sequence);
            }
            ("macro", arg) if arg.starts_with("record-") => {
                let target = arg.trim_start_matches("record-");
                if let Some(gk) = crate::macro_engine::get_gkey_by_id(target) {
                    self.start_recording_for_button(gk.default_btn, RecordingKind::Shortcut);
                } else if let Some(gk) = crate::macro_engine::G_KEYS
                    .iter()
                    .find(|k| k.name.eq_ignore_ascii_case(target))
                {
                    self.start_recording_for_button(gk.default_btn, RecordingKind::Shortcut);
                }
            }
            ("macro", arg) if arg.starts_with("clear-") => {
                let target = arg.trim_start_matches("clear-");
                let key_id = if let Some(gk) = crate::macro_engine::get_gkey_by_id(target) {
                    gk.id
                } else if let Some(gk) = crate::macro_engine::G_KEYS
                    .iter()
                    .find(|k| k.name.eq_ignore_ascii_case(target))
                {
                    gk.id
                } else {
                    target
                };
                let _ = crate::macro_engine::clear_binding(key_id);
                if let Ok(mut st) = self.state.lock() {
                    st.dirty = true;
                }
                let name = crate::macro_engine::get_gkey_by_id(key_id)
                    .map(|gk| format!("{} ({})", gk.name, gk.desc))
                    .unwrap_or_else(|| key_id.to_string());
                self.notify(&format!("已清除 {name} 宏绑定"));
            }
            ("macro", "default-battery") => {
                let _ = crate::macro_engine::save_battery_binding("mouse8");
                if let Ok(mut st) = self.state.lock() {
                    st.dirty = true;
                }
                self.notify("G9 已恢复默认: 设备动作 · 电池电量");
            }
            ("macro", "battery-status") => {
                let state = self.state.clone();
                std::thread::spawn(move || {
                    let result = crate::device::get_conn(2).and_then(|dev| {
                        let battery = crate::features::battery::read_battery(&dev)?;
                        let cfg = crate::config::load()
                            .map_err(|e| crate::hidpp::HidppError::Invalid(e.to_string()))?;
                        crate::features::battery_indicator::show_battery_level(
                            &dev,
                            battery.percent,
                            &cfg,
                        )?;
                        Ok(battery)
                    });
                    match result {
                        Ok(battery) => {
                            if let Ok(mut st) = state.lock() {
                                st.snap.battery = Some(battery);
                                st.dirty = true;
                            }
                        }
                        Err(e) => crate::macro_engine::mlog(&format!("G9 机身电量指示失败: {e}")),
                    }
                });
            }
            ("macro", "finish-recording") => {
                if let Err(e) = self.tap.finish_sequence() {
                    self.notify(&format!("结束录制失败: {e}"));
                }
            }
            ("macro", "cancel-recording") => {
                self.tap.cancel_recording("用户取消");
            }
            ("macro", "poll-recording") => {
                self.tap.expire_recording();
                if let Some(outcome) = take_recording_outcome() {
                    self.handle_recording_outcome(outcome);
                }
            }
            ("macro", "toggle") => {
                crate::macro_engine::mlog("菜单点击:切换宏引擎");
                if self.tap.is_running() {
                    self.tap.stop();
                    self.notify("宏引擎已停用(侧键恢复默认)");
                } else {
                    if !accessibility_granted(false) {
                        accessibility_granted(true);
                        self.notify("请在系统设置中允许 g502hub,然后再次启用");
                        return;
                    }
                    if let Err(e) = self.tap.start() {
                        self.notify(&format!("宏引擎启动失败: {e}"));
                    } else {
                        self.notify("宏引擎已启用:按绑定的侧键试试");
                    }
                }
                // 立即刷新菜单文字(运行中/未启用),给用户可见反馈
                if let Ok(mut st) = self.state.lock() {
                    st.dirty = true;
                }
            }
            ("ghub", "quit") => {
                let _ = std::process::Command::new("osascript")
                    .arg("-e")
                    .arg("tell application \"lghub\" to quit")
                    .spawn();
                std::thread::sleep(Duration::from_millis(1500));
                if ghub_agent_running() {
                    let _ = std::process::Command::new("pkill")
                        .arg("-x")
                        .arg("lghub_agent")
                        .spawn();
                    let _ = std::process::Command::new("pkill")
                        .arg("-x")
                        .arg("lghub_system_tray")
                        .spawn();
                }
            }
            ("config", "open") => {
                let path = config::config_path();
                let _ = std::fs::create_dir_all(path.parent().unwrap());
                if !path.exists() {
                    let _ = config::save(&Config::default());
                }
                let _ = std::process::Command::new("open").arg(&path).spawn();
            }
            ("app", "quit") => {
                if let Ok(dev) = crate::device::get_conn(0) {
                    let _ = controller::with_device_lock(|| {
                        if let Ok(indicator) =
                            crate::features::indicator_led::IndicatorLed::new(&dev)
                        {
                            let _ = indicator.release();
                        }
                        Ok(())
                    });
                }
                QUIT.store(true, Ordering::Relaxed);
            }
            _ => {}
        }
        if let Ok(mut st) = self.state.lock() {
            st.dirty = true;
        }
    }
    fn notify(&self, text: &str) {
        let _ = std::process::Command::new("osascript")
            .arg("-e")
            .arg(format!(
                "display notification \"{text}\" with title \"g502hub\""
            ))
            .spawn();
    }
}

pub fn hex_rgb(s: &str) -> [u8; 3] {
    if s.len() != 6 {
        return [255, 255, 255];
    }
    let b = |r: std::ops::Range<usize>| u8::from_str_radix(&s[r], 16).unwrap_or(255);
    [b(0..2), b(2..4), b(4..6)]
}

fn point_in_polygon(x: f32, y: f32, points: &[(f32, f32)]) -> bool {
    let mut inside = false;
    let mut j = points.len() - 1;
    for i in 0..points.len() {
        let (xi, yi) = points[i];
        let (xj, yj) = points[j];
        if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// 生成 36×36 macOS Template 圆环。系统负责颜色，Alpha 区分底轨与电量弧。
fn make_icon(snap: &Snapshot) -> Result<Icon> {
    let (w, h) = (36u32, 36u32);
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    let battery = snap.battery.as_ref();
    let connected = snap.device_desc.is_some() && battery.is_some();
    let level = battery.map(|b| b.percent).unwrap_or(0) as f32 / 100.0;
    let charging = battery.map(|b| b.charging).unwrap_or(false);
    let tau = std::f32::consts::TAU;
    let radius = 14.0f32;
    let progress_angle = tau * level.clamp(0.0, 1.0);
    let end_x = 18.0 + radius * progress_angle.sin();
    let end_y = 18.0 - radius * progress_angle.cos();

    for y in 0..h {
        for x in 0..w {
            let mut alpha_sum = 0u32;
            for sy in 0..4 {
                for sx in 0..4 {
                    let px = x as f32 + (sx as f32 + 0.5) / 4.0;
                    let py = y as f32 + (sy as f32 + 0.5) / 4.0;
                    let dx = px - 18.0;
                    let dy = py - 18.0;
                    let dist = dx.hypot(dy);
                    let angle = (dy.atan2(dx) + std::f32::consts::FRAC_PI_2).rem_euclid(tau);
                    let track = (dist - radius).abs() < 0.75;
                    let arc = connected
                        && level > 0.0
                        && (dist - radius).abs() < 2.25
                        && (angle <= progress_angle
                            || dx.hypot(dy + radius) < 2.25
                            || (px - end_x).hypot(py - end_y) < 2.25);
                    let slash = !connected && (px - py).abs() < 1.25 && dist < radius - 2.2;
                    let bolt = charging
                        && point_in_polygon(
                            px,
                            py,
                            &[
                                (18.5, 9.2),
                                (13.5, 17.5),
                                (16.8, 17.5),
                                (15.5, 26.8),
                                (22.5, 16.5),
                                (19.2, 16.5),
                            ],
                        );
                    let alpha = if arc || slash || bolt {
                        255u32
                    } else if track {
                        68u32
                    } else {
                        0
                    };
                    alpha_sum += alpha;
                }
            }
            let i = ((y * w + x) * 4) as usize;
            rgba[i + 3] = (alpha_sum / 16).min(255) as u8;
        }
    }
    Ok(Icon::from_rgba(rgba, w, h)?)
}

fn publish_connected(
    state: &Arc<Mutex<State>>,
    desc: String,
    battery: BatteryInfo,
    dpi: Option<u16>,
    mode: OnboardMode,
) {
    if let Ok(mut st) = state.lock() {
        st.snap.device_desc = Some(desc);
        st.snap.battery = Some(battery);
        st.snap.dpi = dpi;
        st.snap.mode = Some(mode);
        st.snap.error = None;
        st.dirty = true;
    }
}

fn publish_battery(state: &Arc<Mutex<State>>, desc: String, battery: BatteryInfo) {
    if let Ok(mut st) = state.lock() {
        st.snap.device_desc = Some(desc);
        st.snap.battery = Some(battery);
        st.snap.error = None;
        st.dirty = true;
    }
}

fn publish_partial(state: &Arc<Mutex<State>>, desc: String, battery: BatteryInfo) {
    if let Ok(mut st) = state.lock() {
        st.snap.device_desc = Some(desc);
        st.snap.battery = Some(battery);
        st.snap.dpi = None;
        st.snap.mode = None;
        st.snap.error = None;
        st.dirty = true;
    }
}

fn publish_offline(state: &Arc<Mutex<State>>, error: String) {
    if let Ok(mut st) = state.lock() {
        st.snap.device_desc = None;
        st.snap.battery = None;
        st.snap.dpi = None;
        st.snap.mode = None;
        st.snap.error = Some(error);
        st.dirty = true;
    }
}

fn is_device_connected(snap: &Snapshot) -> bool {
    snap.device_desc.is_some() && snap.battery.is_some()
}

fn keep_online_after_battery_failure(failures: u8, interface_present: bool) -> bool {
    failures < 2 && interface_present
}

fn wait_with_interface_watch(dev: &G502Device, seconds: u64) -> bool {
    let mut remaining = seconds;
    while remaining > 0 {
        let step = remaining.min(2);
        std::thread::sleep(Duration::from_secs(step));
        remaining -= step;
        if matches!(connection_interface_unchanged(dev), Ok(false)) {
            return false;
        }
    }
    true
}

/// 电量轮询线程：新连接、系统唤醒或待同步时恢复模式/DPI/RGB。
fn poll_loop(state: Arc<Mutex<State>>, led_sync_pending: Arc<AtomicBool>) {
    const SYSTEM_WAKE_GAP: Duration = Duration::from_secs(15);
    let mut active: Option<Arc<G502Device>> = None;
    let mut desc = String::new();
    let mut battery_failures = 0u8;
    let mut state_sync_pending = false;
    let mut last_iteration = Instant::now();
    loop {
        if last_iteration.elapsed() >= SYSTEM_WAKE_GAP {
            led_sync_pending.store(true, Ordering::Release);
        }
        last_iteration = Instant::now();
        let cfg = config::load().unwrap_or_default();
        let next_delay;

        if let Some(dev) = active.clone() {
            match read_battery(&dev) {
                Ok(battery) => {
                    battery_failures = 0;
                    next_delay = 5;
                    publish_battery(&state, desc.clone(), battery.clone());
                    if state_sync_pending {
                        match controller::apply_desired_all(&dev, &cfg) {
                            Ok((applied, led_synced)) => {
                                publish_connected(
                                    &state,
                                    desc.clone(),
                                    battery,
                                    applied.dpi,
                                    applied.mode,
                                );
                                state_sync_pending = false;
                                led_sync_pending.store(!led_synced, Ordering::Release);
                            }
                            Err(e) => eprintln!("设备状态同步失败，将重试: {e}"),
                        }
                    }
                    let pending = led_sync_pending.load(Ordering::Acquire);
                    if !state_sync_pending && cfg.desired_mode == DesiredMode::Host && pending {
                        let latest = config::load().unwrap_or(cfg);
                        match controller::apply_desired_led(&dev, &latest) {
                            Ok(()) => {
                                led_sync_pending.store(false, Ordering::Release);
                            }
                            Err(e) => {
                                eprintln!("RGB 自动同步失败: {e}");
                                led_sync_pending.store(true, Ordering::Release);
                            }
                        }
                    }
                }
                Err(e) => {
                    battery_failures = battery_failures.saturating_add(1);
                    eprintln!("电量轮询失败 ({battery_failures}/2): {e}");
                    if keep_online_after_battery_failure(
                        battery_failures,
                        g502_interface_present().unwrap_or(true),
                    ) {
                        next_delay = 1;
                    } else {
                        active = None;
                        battery_failures = 0;
                        state_sync_pending = false;
                        led_sync_pending.store(true, Ordering::Release);
                        invalidate_connection();
                        next_delay = 2;
                        publish_offline(&state, e.to_string());
                    }
                }
            }
        } else {
            match crate::device::get_conn(0).and_then(|dev| {
                let device_desc = crate::device::describe(&dev);
                let battery = read_battery(&dev)?;
                let applied = controller::apply_desired_all(&dev, &cfg);
                let (applied, led_synced) = match applied {
                    Ok(value) => (Some(value.0), value.1),
                    Err(e) => {
                        eprintln!("设备在线，状态同步稍后重试: {e}");
                        (None, false)
                    }
                };
                if let Some(applied) = applied {
                    if cfg.desired_dpi.is_none() && applied.mode == OnboardMode::Host {
                        if let Some(current_dpi) = applied.dpi {
                            let _ = config::update(|latest| {
                                latest.desired_dpi = Some(current_dpi);
                            });
                        }
                    }
                }
                Ok((dev, device_desc, battery, applied, led_synced))
            }) {
                Ok((dev, device_desc, battery, applied, led_synced)) => {
                    next_delay = if applied.is_some() { 5 } else { 2 };
                    desc = device_desc;
                    state_sync_pending = applied.is_none();
                    led_sync_pending.store(!led_synced, Ordering::Release);
                    if let Some(applied) = applied {
                        publish_connected(&state, desc.clone(), battery, applied.dpi, applied.mode);
                    } else {
                        publish_partial(&state, desc.clone(), battery);
                    }
                    active = Some(dev);
                }
                Err(e) => {
                    eprintln!("设备连接失败，将重试: {e}");
                    next_delay = 2;
                    publish_offline(&state, e.to_string());
                }
            }
        }

        if let Some(dev) = active.clone() {
            if !wait_with_interface_watch(&dev, next_delay) {
                active = None;
                battery_failures = 0;
                state_sync_pending = false;
                led_sync_pending.store(true, Ordering::Release);
                invalidate_connection();
                publish_offline(&state, "G502 USB 连接已变化，正在重新连接".into());
            }
        } else {
            std::thread::sleep(Duration::from_secs(next_delay));
        }
    }
}

#[cfg(test)]
mod connection_tests {
    use super::*;

    #[test]
    fn temporary_battery_failure_keeps_device_visible() {
        assert!(keep_online_after_battery_failure(1, true));
        assert!(!keep_online_after_battery_failure(2, true));
        assert!(!keep_online_after_battery_failure(1, false));
    }

    #[test]
    fn test_is_device_connected() {
        let mut snap = Snapshot::default();
        assert!(!is_device_connected(&snap));

        snap.device_desc = Some("G502 LIGHTSPEED".into());
        assert!(!is_device_connected(&snap));

        snap.battery = Some(BatteryInfo {
            percent: 80,
            charging: false,
            voltage_mv: Some(3900),
            state_text: "放电中".into(),
        });
        assert!(is_device_connected(&snap));
    }
}

pub fn run() -> Result<()> {
    let Some(mtm) = MainThreadMarker::new() else {
        anyhow::bail!("menubar 必须在主线程运行");
    };
    // HidApi 必须在主线程初始化(见 hidpp.rs 注释)
    crate::hidpp::init_shared_api()?;
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);

    let cfg = config::load()?;
    let state = Arc::new(Mutex::new(State {
        snap: Snapshot::default(),
        dirty: false,
    }));
    let tap = Arc::new(MacroTap::new(Arc::new(|| {
        config::load().map(|c| c.macros).unwrap_or_default()
    })));
    crate::macro_engine::set_global_tap(tap.clone());

    if cfg.macros.values().any(|m| m.enabled) {
        let granted = accessibility_granted(false);
        crate::macro_engine::mlog(&format!(
            "menubar 启动:有启用宏,accessibility_granted={granted},尝试启动 tap"
        ));
        if granted {
            if let Err(e) = tap.start() {
                crate::macro_engine::mlog(&format!("menubar 启动:tap 启动失败: {e}"));
            }
        } else {
            // 以最终签名 Bundle 的身份触发系统授权引导；旧的裸二进制/zcode 授权不再复用。
            accessibility_granted(true);
            crate::macro_engine::mlog("menubar 启动:已请求 g502hub.app 辅助功能权限");
            // 用户在系统设置中勾选后自动启动，不要求再点一次菜单。
            let tap_after_permission = tap.clone();
            std::thread::spawn(move || {
                for _ in 0..120 {
                    std::thread::sleep(Duration::from_secs(2));
                    if accessibility_granted(false) {
                        match tap_after_permission.start() {
                            Ok(()) => crate::macro_engine::mlog("辅助功能授权已生效,tap 自动启动"),
                            Err(e) => {
                                crate::macro_engine::mlog(&format!("授权后 tap 启动失败: {e}"))
                            }
                        }
                        break;
                    }
                }
            });
        }
    } else {
        crate::macro_engine::mlog("menubar 启动:无启用宏,tap 未启动");
    }

    // 动作工作线程:菜单事件 → 后台执行设备操作,主线程永不阻塞
    let led_sync_pending = Arc::new(AtomicBool::new(true));
    let core = Arc::new(Core {
        state: state.clone(),
        tap: tap.clone(),
        cfg: cfg.clone(),
        led_sync_pending: led_sync_pending.clone(),
    });
    let (tx, rx) = mpsc::channel::<String>();
    let _ = ACTION_TX.set(tx);
    {
        let core = core.clone();
        std::thread::spawn(move || {
            while let Ok(id) = rx.recv() {
                core.handle(&id);
                if QUIT.load(Ordering::Relaxed) {
                    std::process::exit(0);
                }
            }
        });
    }

    let initial_snap = Snapshot::default();
    let icon = make_icon(&initial_snap)?;
    let tray = TrayIconBuilder::new()
        .with_menu(Box::new(Menu::new()))
        .with_icon(icon)
        .with_icon_as_template(true)
        .with_tooltip("G502 LIGHTSPEED")
        .with_menu_on_left_click(false)
        .build()?;
    tray.set_title::<&str>(None);

    let initial_connected = is_device_connected(&initial_snap);
    let initial_visible = if cfg.hide_tray_when_disconnected {
        initial_connected
    } else {
        true
    };

    let mut app = App {
        state: state.clone(),
        tray,
        tray_visible: initial_visible,
        ticks: 0,
        tap: tap.clone(),
        cfg: cfg.clone(),
        header: MenuItem::with_id("noop", "", false, None),
        battery_line: MenuItem::with_id("noop", "", false, None),
        mode_line: MenuItem::with_id("noop", "", false, None),
        mode_action: MenuItem::with_id("mode:toggle", "", false, None),
        dpi_items: Vec::new(),
        macro_status: MenuItem::with_id("noop", "", false, None),
        macro_g4: MenuItem::with_id("noop", "", false, None),
        macro_g5: MenuItem::with_id("noop", "", false, None),
        macro_record_shortcut: MenuItem::with_id("macro:record-shortcut", "", true, None),
        macro_record_sequence: MenuItem::with_id("macro:record-sequence", "", true, None),
        macro_finish_recording: MenuItem::with_id("macro:finish-recording", "", false, None),
        macro_cancel_recording: MenuItem::with_id("macro:cancel-recording", "", false, None),
        macro_toggle: MenuItem::with_id("macro:toggle", "", true, None),
        last_recording_serial: recording_serial(),
        led_rate_items: Vec::new(),
        led_bright_items: Vec::new(),
        ghub_item: MenuItem::with_id("ghub:quit", "", false, None),
    };
    app.build_menu()?;
    {
        let snap = state.lock().unwrap().snap.clone();
        app.refresh_menu(&snap);
    }
    if !app.tray_visible {
        let _ = app.tray.set_visible(false);
    }

    // 初始化控制中心浮窗
    crate::panel::PopoverPanel::init(mtm);

    let poll_state = state.clone();
    let poll_led_sync = led_sync_pending.clone();
    std::thread::spawn(move || poll_loop(poll_state, poll_led_sync));

    // CFRunLoopTimer:主线程 0.2s 泵一次,经 context 携带 App 指针
    let app_ptr = &mut app as *mut App;
    let mut ctx = CFRunLoopTimerContext {
        version: 0,
        info: app_ptr as *mut std::ffi::c_void,
        retain: None,
        release: None,
        copyDescription: None,
    };
    let timer: CFRunLoopTimerRef = unsafe {
        CFRunLoopTimerCreate(
            std::ptr::null(),
            CFAbsoluteTimeGetCurrent() + 0.2,
            0.2,
            0,
            0,
            pump_callback,
            &mut ctx,
        )
    };
    unsafe {
        CFRunLoopAddTimer(CFRunLoopGetMain(), timer, kCFRunLoopCommonModes);
        let ns_app = NSApplication::sharedApplication(mtm);
        ns_app.finishLaunching();
        ns_app.run();
    }
    Ok(())
}

extern "C" fn pump_callback(_timer: CFRunLoopTimerRef, info: *mut std::ffi::c_void) {
    if info.is_null() {
        return;
    }
    let app = unsafe { &mut *(info as *mut App) };

    app.tap.expire_recording();
    if recording_outcome_pending() {
        if let Some(tx) = ACTION_TX.get() {
            let _ = tx.send("macro:poll-recording".into());
        }
    }
    let serial = recording_serial();
    let recording_changed = serial != app.last_recording_serial;
    if recording_changed {
        app.last_recording_serial = serial;
    }

    // 0. 消费来自菜单或其他线程的弹窗请求
    crate::panel::PopoverPanel::poll_open_request();

    // 1. 托盘点击事件：左键弹出/收起 PopoverPanel (只响应 Down 避免双触发)
    let tray_receiver = TrayIconEvent::receiver();
    while let Ok(ev) = tray_receiver.try_recv() {
        if let TrayIconEvent::Click {
            button,
            rect,
            button_state,
            ..
        } = ev
        {
            if button == MouseButton::Left && button_state == tray_icon::MouseButtonState::Down {
                crate::panel::PopoverPanel::toggle_at(Some(rect));
            }
        }
    }

    // 2. 菜单事件 → 转发到工作线程(主线程不执行设备操作)
    let receiver = MenuEvent::receiver();
    while let Ok(ev) = receiver.try_recv() {
        let id = ev.id().0.clone();
        if id == "panel:open" {
            crate::panel::PopoverPanel::show_at(None);
            continue;
        }
        if let Some(tx) = ACTION_TX.get() {
            let _ = tx.send(id);
        }
    }

    app.ticks = app.ticks.wrapping_add(1);
    let periodic_check = app.ticks % 5 == 0; // 每秒检查一次配置变更

    if periodic_check {
        if let Ok(cfg) = config::load() {
            app.cfg = cfg;
        }
    }

    // 3. 状态变化 → 快照后原地刷新(不持锁渲染)
    let (snap, is_dirty) = {
        let mut st = match app.state.try_lock() {
            Ok(st) => st,
            Err(_) => return, // 轮询线程正持有锁,下个 0.2s 周期再来
        };
        let dirty = st.dirty || recording_changed;
        if dirty {
            st.dirty = false;
            (Some(st.snap.clone()), true)
        } else if periodic_check {
            (Some(st.snap.clone()), false)
        } else {
            (None, false)
        }
    };
    if let Some(snap) = snap {
        let connected = is_device_connected(&snap);
        let target_visible = if app.cfg.hide_tray_when_disconnected {
            connected
        } else {
            true
        };

        let vis_changed = target_visible != app.tray_visible;
        if vis_changed {
            let _ = app.tray.set_visible(target_visible);
            app.tray_visible = target_visible;
        }

        if is_dirty || vis_changed {
            if app.tray_visible {
                app.tray.set_title::<&str>(None);
                if let Ok(icon) = make_icon(&snap) {
                    let _ = app.tray.set_icon_with_as_template(Some(icon), true);
                }
            }
            app.refresh_menu(&snap);
        }
    }
}
