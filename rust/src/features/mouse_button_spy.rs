//! 主机模式按键监听:feature 0x8110 (MouseButtonSpy)。
//!
//! 官方 G HUB 协议:
//! f0 GetButtonCount, f1 StartSpy, f2 StopSpy, f3 GetButtonMap。
//! 异步 function 0 的前两个参数是大端 16 位按键位图，最低位对应槽位 1。

use crate::device::G502Device;
use crate::hidpp::{HidppError, LONG_LEN, LONG_REPORT, SHORT_REPORT};
use hidapi::HidDevice;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::Duration;

const FEATURE_MOUSE_BUTTON_SPY: u16 = 0x8110;
const FUNCTION_REPORT: u8 = 0x00;
const FUNCTION_GET_BUTTON_COUNT: u8 = 0x00;
const FUNCTION_START_SPY: u8 = 0x01;
const FUNCTION_STOP_SPY: u8 = 0x02;
const FUNCTION_GET_BUTTON_MAP: u8 = 0x03;
const MAX_BUTTONS: usize = 16;

struct Listener {
    identity: String,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Listener {
    fn is_running(&self) -> bool {
        self.thread
            .as_ref()
            .is_some_and(|thread| !thread.is_finished())
    }

    fn stop(mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

static LISTENER: OnceLock<Mutex<Option<Listener>>> = OnceLock::new();

fn listener_slot() -> &'static Mutex<Option<Listener>> {
    LISTENER.get_or_init(|| Mutex::new(None))
}

fn response_param(resp: &[u8], offset: usize, operation: &str) -> Result<u8, HidppError> {
    resp.get(4 + offset).copied().ok_or_else(|| {
        HidppError::Io(format!(
            "MouseButtonSpy {operation} 响应过短: {} 字节",
            resp.len()
        ))
    })
}

fn button_count(dev: &G502Device, feature_index: u8) -> Result<u8, HidppError> {
    let resp = dev.request(feature_index, FUNCTION_GET_BUTTON_COUNT, &[])?;
    Ok(response_param(&resp, 0, "GetButtonCount")?.min(MAX_BUTTONS as u8))
}

fn button_map(dev: &G502Device, feature_index: u8) -> Result<[u8; MAX_BUTTONS], HidppError> {
    let resp = dev.request(feature_index, FUNCTION_GET_BUTTON_MAP, &[])?;
    if resp.len() < 4 + MAX_BUTTONS {
        return Err(HidppError::Io(format!(
            "MouseButtonSpy GetButtonMap 响应过短: {} 字节",
            resp.len()
        )));
    }
    let mut map = [0u8; MAX_BUTTONS];
    map.copy_from_slice(&resp[4..4 + MAX_BUTTONS]);
    Ok(map)
}

fn parse_button_bitmap(report: &[u8], dev_index: u8, feature_index: u8) -> Option<u16> {
    if report.len() < 6
        || !matches!(report[0], SHORT_REPORT | LONG_REPORT)
        || report[1] != dev_index
        || report[2] != feature_index
        || report[3] != FUNCTION_REPORT
    {
        return None;
    }
    Some(u16::from_be_bytes([report[4], report[5]]))
}

fn logical_button(hardware_button: u32) -> u32 {
    match hardware_button {
        9 => 10,
        10 => 9,
        _ => hardware_button,
    }
}

fn changed_buttons(previous: u16, current: u16, count: u8) -> Vec<(u32, bool)> {
    let changed = previous ^ current;
    let mut events = Vec::new();
    for slot in 0..usize::from(count.min(MAX_BUTTONS as u8)) {
        let mask = 1u16 << slot;
        if changed & mask != 0 {
            events.push((logical_button(slot as u32), current & mask != 0));
        }
    }
    events
}

fn listen(dev: HidDevice, stop: Arc<AtomicBool>, dev_index: u8, feature_index: u8, count: u8) {
    let mut previous = 0u16;
    let mut failures = 0u8;
    while !stop.load(Ordering::Acquire) {
        let should_sleep = objc2::rc::autoreleasepool(|_| {
            let mut buf = [0u8; LONG_LEN];
            match dev.read_timeout(&mut buf, 200) {
                Ok(0) => false,
                Ok(n) => {
                    failures = 0;
                    let Some(current) = parse_button_bitmap(&buf[..n], dev_index, feature_index) else {
                        return false;
                    };
                    if current == previous {
                        return false;
                    }
                    crate::macro_engine::mlog(&format!(
                        "MouseButtonSpy 报告: previous={previous:#06x},current={current:#06x}"
                    ));
                    for (button, is_down) in changed_buttons(previous, current, count) {
                        crate::macro_engine::mlog(&format!(
                            "MouseButtonSpy 槽位: G{},mouse{},{}",
                            button + 1,
                            button,
                            if is_down { "down" } else { "up" }
                        ));
                        if crate::macro_engine::get_gkey_by_button(button).is_some() {
                            let _ = crate::macro_engine::handle_device_button(button, is_down);
                        }
                    }
                    previous = current;
                    false
                }
                Err(error) => {
                    failures = failures.saturating_add(1);
                    crate::macro_engine::mlog(&format!(
                        "MouseButtonSpy 读取失败 ({failures}/3): {error}"
                    ));
                    if failures >= 3 {
                        return false;
                    }
                    true
                }
            }
        });
        if failures >= 3 {
            break;
        }
        if should_sleep {
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    crate::macro_engine::mlog("MouseButtonSpy 监听线程退出");
}

/// 在 Host 模式启动按键监听。相同设备上的健康监听器会直接复用。
pub fn start(dev: &G502Device) -> Result<(), HidppError> {
    let identity = dev.interface_identity();
    let stale = {
        let mut slot = listener_slot()
            .lock()
            .map_err(|_| HidppError::Io("MouseButtonSpy 监听锁已损坏".into()))?;
        if slot
            .as_ref()
            .is_some_and(|listener| listener.identity == identity && listener.is_running())
        {
            return Ok(());
        }
        slot.take()
    };
    if let Some(listener) = stale {
        listener.stop();
    }

    let feature_index = dev.feature(FEATURE_MOUSE_BUTTON_SPY)?;
    let count = button_count(dev, feature_index)?;
    if count == 0 {
        return Err(HidppError::Invalid(
            "MouseButtonSpy 报告按键数量为 0".into(),
        ));
    }
    let map = button_map(dev, feature_index)?;
    let notification_dev = dev.open_additional_handle()?;
    dev.request(feature_index, FUNCTION_START_SPY, &[])?;

    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = stop.clone();
    let dev_index = dev.dev_index;
    let thread = std::thread::Builder::new()
        .name("g502-button-spy".into())
        .spawn(move || {
            listen(
                notification_dev,
                thread_stop,
                dev_index,
                feature_index,
                count,
            )
        })
        .map_err(|error| HidppError::Io(format!("启动 MouseButtonSpy 线程失败: {error}")))?;

    crate::macro_engine::mlog(&format!(
        "MouseButtonSpy 已启动: feature={feature_index:#04x},buttons={count},map={map:02x?}"
    ));
    *listener_slot()
        .lock()
        .map_err(|_| HidppError::Io("MouseButtonSpy 监听锁已损坏".into()))? = Some(Listener {
        identity,
        stop,
        thread: Some(thread),
    });
    Ok(())
}

/// 停止当前监听器；设备仍在线时同时发送 f2 StopSpy。
pub fn stop(dev: &G502Device, notify_device: bool) -> Result<(), HidppError> {
    let listener = listener_slot()
        .lock()
        .map_err(|_| HidppError::Io("MouseButtonSpy 监听锁已损坏".into()))?
        .take();
    if let Some(listener) = listener {
        listener.stop();
    }

    if notify_device {
        let feature_index = dev.feature(FEATURE_MOUSE_BUTTON_SPY)?;
        dev.request(feature_index, FUNCTION_STOP_SPY, &[])?;
    }
    crate::macro_engine::mlog("MouseButtonSpy 已停止");
    Ok(())
}

/// 连接已经失效时只终止本地监听，不再向旧设备写请求。
pub fn stop_local() {
    let listener = listener_slot().lock().ok().and_then(|mut slot| slot.take());
    if let Some(listener) = listener {
        listener.stop();
        crate::macro_engine::mlog("MouseButtonSpy 本地监听已清理");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_big_endian_async_bitmap() {
        let report = [SHORT_REPORT, 0x01, 0x0a, 0x00, 0x40, 0x01];
        assert_eq!(parse_button_bitmap(&report, 0x01, 0x0a), Some(0x4001));
        assert_eq!(parse_button_bitmap(&report, 0x02, 0x0a), None);
        assert_eq!(parse_button_bitmap(&report, 0x01, 0x0b), None);
    }

    #[test]
    fn ignores_matching_function_response_with_software_id() {
        let response = [SHORT_REPORT, 0x01, 0x0a, 0x05, 0x80, 0x00];
        assert_eq!(parse_button_bitmap(&response, 0x01, 0x0a), None);
    }

    #[test]
    fn maps_lsb_to_g1_and_msb_to_g16() {
        assert_eq!(changed_buttons(0, 0x0001, 16), vec![(0, true)]);
        assert_eq!(changed_buttons(0, 0x0008, 16), vec![(3, true)]);
        assert_eq!(changed_buttons(0, 0x8000, 16), vec![(15, true)]);
    }

    #[test]
    fn swaps_hardware_wheel_tilt_slots_to_logical_g10_g11() {
        assert_eq!(changed_buttons(0, 0x0200, 16), vec![(10, true)]);
        assert_eq!(changed_buttons(0, 0x0400, 16), vec![(9, true)]);
    }

    #[test]
    fn emits_press_and_release_edges_in_slot_order() {
        assert_eq!(
            changed_buttons(0x0009, 0x8008, 16),
            vec![(0, false), (15, true)]
        );
    }

    #[test]
    fn respects_device_button_count() {
        assert_eq!(changed_buttons(0, 0x0200, 9), Vec::<(u32, bool)>::new());
        assert_eq!(changed_buttons(0, 0x0100, 9), vec![(8, true)]);
    }
}
