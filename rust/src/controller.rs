//! 设备期望状态控制器。
//!
//! HID++ 设置由单一操作锁串行化；模式和 DPI 只有读回一致才算成功。

use crate::config::{Config, DesiredMode};
use crate::device::G502Device;
use crate::features::dpi::Dpi;
use crate::features::indicator_led::IndicatorLed;
use crate::features::led::{self, Led};
use crate::features::onboard::{get_onboard_mode, set_onboard_mode, OnboardMode};
use crate::hidpp::HidppError;
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

static DEVICE_OP: Mutex<()> = Mutex::new(());

#[derive(Debug, Clone, Copy)]
pub struct AppliedState {
    pub mode: OnboardMode,
    pub dpi: Option<u16>,
}

pub fn with_device_lock<T>(f: impl FnOnce() -> Result<T, HidppError>) -> Result<T, HidppError> {
    let _guard = DEVICE_OP
        .lock()
        .map_err(|_| HidppError::Io("设备操作锁已损坏".into()))?;
    f()
}

pub fn read_mode(dev: &G502Device) -> Result<OnboardMode, HidppError> {
    match get_onboard_mode(dev) {
        Some(r) => r,
        None => Err(HidppError::Invalid(
            "设备不支持板载模式 feature 0x8100".into(),
        )),
    }
}

pub fn set_mode_confirmed(dev: &G502Device, mode: OnboardMode) -> Result<OnboardMode, HidppError> {
    with_device_lock(|| {
        let actual = set_onboard_mode(dev, mode)?;
        if actual == OnboardMode::Onboard {
            IndicatorLed::new(dev)?.release()?;
        }
        Ok(actual)
    })
}

pub fn set_dpi_confirmed(dev: &G502Device, dpi: u16) -> Result<u16, HidppError> {
    with_device_lock(|| {
        let mode = read_mode(dev)?;
        if mode != OnboardMode::Host {
            return Err(HidppError::Invalid(
                "当前为板载模式；请先切换到主机控制模式再设置 DPI".into(),
            ));
        }
        Dpi::new(dev)?.set_dpi(dpi)
    })
}

fn apply_desired_state_inner(dev: &G502Device, cfg: &Config) -> Result<AppliedState, HidppError> {
    let desired_mode = match cfg.desired_mode {
        DesiredMode::Host => OnboardMode::Host,
        DesiredMode::Onboard => OnboardMode::Onboard,
    };
    let current_mode = read_mode(dev)?;
    // 旧配置没有 desired_dpi 时，在离开板载模式前先捕获用户正在使用的 DPI，
    // 避免固件切到 Host 后回落到默认 800。
    let target_dpi = cfg
        .desired_dpi
        .or_else(|| Dpi::new(dev).and_then(|d| d.get_dpi()).ok());
    let mode = if current_mode == desired_mode {
        current_mode
    } else {
        set_onboard_mode(dev, desired_mode)?
    };

    if mode == OnboardMode::Onboard {
        if let Ok(indicator) = IndicatorLed::new(dev) {
            let _ = indicator.release();
        }
    }

    let dpi = if mode == OnboardMode::Host {
        match target_dpi {
            Some(target) => {
                let d = Dpi::new(dev)?;
                let current = d.get_dpi()?;
                Some(if current == target {
                    current
                } else {
                    d.set_dpi(target)?
                })
            }
            None => Dpi::new(dev).and_then(|d| d.get_dpi()).ok(),
        }
    } else {
        Dpi::new(dev).and_then(|d| d.get_dpi()).ok()
    };

    Ok(AppliedState { mode, dpi })
}

pub(crate) fn apply_led_spec_inner(
    dev: &G502Device,
    led: &Led<'_>,
    zone: u8,
    spec: &crate::config::LedSpec,
) -> Result<(), HidppError> {
    if zone == led::ZONE_PRIMARY {
        if let Ok(indicator) = IndicatorLed::new(dev) {
            if spec.off {
                let _ = indicator.turn_off();
            } else {
                let _ = indicator.show_all();
            }
            std::thread::sleep(Duration::from_millis(12));
        }
    }

    if spec.off {
        led.set_off(zone)
    } else {
        let period = led::rate_period_ms(spec.rate.as_deref())?;
        led.set_effect(zone, &spec.effect, spec.rgb, spec.brightness, period)
    }
}

fn apply_desired_led_inner(dev: &G502Device, cfg: &Config) -> Result<(), HidppError> {
    let led = Led::new(dev)?;
    let mut wrote = false;
    for (key, zone) in [("primary", led::ZONE_PRIMARY), ("logo", led::ZONE_LOGO)] {
        let Some(spec) = cfg.led_zones.get(key) else {
            continue;
        };
        if wrote {
            std::thread::sleep(Duration::from_millis(25));
        }
        apply_led_spec_inner(dev, &led, zone, spec)?;
        wrote = true;
    }
    Ok(())
}

/// 串行应用单个分区灯效，供菜单和 CLI 使用。
pub fn apply_led_spec(
    dev: &G502Device,
    zone: u8,
    spec: &crate::config::LedSpec,
) -> Result<(), HidppError> {
    with_device_lock(|| {
        let led = Led::new(dev)?;
        apply_led_spec_inner(dev, &led, zone, spec)
    })
}

/// 串行重放两个分区灯效。
pub fn apply_desired_led(dev: &G502Device, cfg: &Config) -> Result<(), HidppError> {
    with_device_lock(|| apply_desired_led_inner(dev, cfg))
}

/// 刚上线时在同一设备锁内恢复模式、DPI 和 RGB，避免菜单写入穿插。
/// RGB 失败不把设备整体判为离线，返回值第二项表示灯效是否已同步。
pub fn apply_desired_all(
    dev: &G502Device,
    cfg: &Config,
) -> Result<(AppliedState, bool), HidppError> {
    with_device_lock(|| {
        let state = apply_desired_state_inner(dev, cfg)?;
        let led_synced = if state.mode == OnboardMode::Host {
            std::thread::sleep(Duration::from_millis(40));
            match apply_desired_led_inner(dev, cfg) {
                Ok(()) => true,
                Err(e) => {
                    eprintln!("首次 RGB 恢复失败，将自动重试: {e}");
                    false
                }
            }
        } else {
            true
        };
        Ok((state, led_synced))
    })
}

#[derive(Clone)]
pub enum TargetZone {
    Single(u8),
    All,
}

#[derive(Clone)]
struct PendingUpdate {
    target: TargetZone,
    spec: crate::config::LedSpec,
    updated_at: Instant,
}

struct LiveLedState {
    pending_dispatch: Option<PendingUpdate>,
    pending_persist: Option<PendingUpdate>,
}

static LIVE_LED: OnceLock<(Mutex<LiveLedState>, Condvar)> = OnceLock::new();

fn get_live_channel() -> &'static (Mutex<LiveLedState>, Condvar) {
    LIVE_LED.get_or_init(|| {
        let pair = (
            Mutex::new(LiveLedState {
                pending_dispatch: None,
                pending_persist: None,
            }),
            Condvar::new(),
        );
        std::thread::spawn(live_led_worker);
        pair
    })
}

fn live_led_worker() {
    let (lock, cvar) = get_live_channel();
    let mut guard = lock.lock().unwrap();
    loop {
        // 1. 如果有待下发到设备的指令，优先下发
        if let Some(cmd) = guard.pending_dispatch.take() {
            guard.pending_persist = Some(cmd.clone());
            drop(guard);

            if let Ok(dev) = crate::device::get_conn(1) {
                let _ = with_device_lock(|| {
                    if read_mode(&dev)? != OnboardMode::Host {
                        return Ok(());
                    }
                    let led = Led::new(&dev)?;
                    match cmd.target {
                        TargetZone::Single(z) => {
                            apply_led_spec_inner(&dev, &led, z, &cmd.spec)?;
                        }
                        TargetZone::All => {
                            apply_led_spec_inner(&dev, &led, led::ZONE_PRIMARY, &cmd.spec)?;
                            std::thread::sleep(Duration::from_millis(25));
                            apply_led_spec_inner(&dev, &led, led::ZONE_LOGO, &cmd.spec)?;
                        }
                    }
                    Ok(())
                });
            }

            // 下发后做短暂合流保护（30ms），防止高频拖动冲垮固件
            std::thread::sleep(Duration::from_millis(30));
            guard = lock.lock().unwrap();
            continue;
        }

        // 2. 如果没有待下发，但有待落盘的配置，检查是否满足 200ms 静止防抖
        if let Some(ref cur) = guard.pending_persist {
            let elapsed = cur.updated_at.elapsed();
            let debounce = Duration::from_millis(200);
            if elapsed >= debounce {
                let to_save = guard.pending_persist.take().unwrap();
                drop(guard);

                let _ = crate::config::update(|cfg| match to_save.target {
                    TargetZone::Single(z) => {
                        cfg.led_zones
                            .insert(led::zone_key(z).to_string(), to_save.spec);
                    }
                    TargetZone::All => {
                        cfg.led_zones.insert(
                            led::zone_key(led::ZONE_PRIMARY).to_string(),
                            to_save.spec.clone(),
                        );
                        cfg.led_zones
                            .insert(led::zone_key(led::ZONE_LOGO).to_string(), to_save.spec);
                    }
                });
                guard = lock.lock().unwrap();
            } else {
                // 未达 200ms，休眠剩余时间（若有新指令输入则被即时打断）
                let remaining = debounce - elapsed;
                let (new_guard, _) = cvar.wait_timeout(guard, remaining).unwrap();
                guard = new_guard;
            }
            continue;
        }

        // 3. 既无待下发也无待落盘，完全休眠直到收到新指令
        guard = cvar.wait(guard).unwrap();
    }
}

/// 专供 UI 滑块 / 色块快速点击使用的合流节流下发接口：
/// - 极速响应：立即合流下发给设备；
/// - 防抖落盘：滑动停止后自动持久化到配置；
/// - 不阻塞当前线程。
pub fn schedule_live_led(target: TargetZone, spec: crate::config::LedSpec) {
    let (lock, cvar) = get_live_channel();
    let mut guard = lock.lock().unwrap();
    guard.pending_dispatch = Some(PendingUpdate {
        target,
        spec,
        updated_at: Instant::now(),
    });
    cvar.notify_one();
}

/// 读取内存中尚未落盘或正在下发的最新灯效规格，避免 UI 在 200ms 防抖期内读到磁盘旧数据。
pub fn get_live_spec_for_zone(zone_key: &str) -> Option<crate::config::LedSpec> {
    let (lock, _) = get_live_channel();
    let guard = lock.lock().ok()?;
    let latest = guard
        .pending_dispatch
        .as_ref()
        .or(guard.pending_persist.as_ref())?;
    match latest.target {
        TargetZone::All => Some(latest.spec.clone()),
        TargetZone::Single(z) => {
            if led::zone_key(z) == zone_key {
                Some(latest.spec.clone())
            } else {
                None
            }
        }
    }
}

// ---- DPI 实时合流与防抖落盘通道 ----

#[derive(Clone)]
struct PendingDpiUpdate {
    dpi: u16,
    updated_at: Instant,
}

struct LiveDpiState {
    pending_dispatch: Option<PendingDpiUpdate>,
    pending_persist: Option<PendingDpiUpdate>,
}

static LIVE_DPI: OnceLock<(Mutex<LiveDpiState>, Condvar)> = OnceLock::new();

fn get_live_dpi_channel() -> &'static (Mutex<LiveDpiState>, Condvar) {
    LIVE_DPI.get_or_init(|| {
        let pair = (
            Mutex::new(LiveDpiState {
                pending_dispatch: None,
                pending_persist: None,
            }),
            Condvar::new(),
        );
        std::thread::spawn(live_dpi_worker);
        pair
    })
}

fn live_dpi_worker() {
    let (lock, cvar) = get_live_dpi_channel();
    let mut guard = lock.lock().unwrap();
    loop {
        // 1. 优先下发最新待发 DPI
        if let Some(cmd) = guard.pending_dispatch.take() {
            guard.pending_persist = Some(cmd.clone());
            drop(guard);

            if let Ok(dev) = crate::device::get_conn(1) {
                let _ = set_dpi_confirmed(&dev, cmd.dpi);
            }

            // 下发后做短暂合流保护（30ms），防止高频拖动冲垮固件
            std::thread::sleep(Duration::from_millis(30));
            guard = lock.lock().unwrap();
            continue;
        }

        // 2. 检查 200ms 静止防抖落盘
        if let Some(ref cur) = guard.pending_persist {
            let elapsed = cur.updated_at.elapsed();
            let debounce = Duration::from_millis(200);
            if elapsed >= debounce {
                let to_save = guard.pending_persist.take().unwrap();
                drop(guard);

                let _ = crate::config::update(|cfg| {
                    cfg.desired_mode = crate::config::DesiredMode::Host;
                    cfg.desired_dpi = Some(to_save.dpi);
                });
                guard = lock.lock().unwrap();
            } else {
                let remaining = debounce - elapsed;
                let (new_guard, _) = cvar.wait_timeout(guard, remaining).unwrap();
                guard = new_guard;
            }
            continue;
        }

        // 3. 休眠直到收到新指令
        guard = cvar.wait(guard).unwrap();
    }
}

/// 专供 UI DPI 滑块 / 预设快速点击使用的合流节流下发接口：
/// - 极速响应：立即合流下发给设备；
/// - 防抖落盘：滑动停止后自动持久化到配置；
/// - 不阻塞当前线程。
pub fn schedule_live_dpi(dpi: u16) {
    let (lock, cvar) = get_live_dpi_channel();
    let mut guard = lock.lock().unwrap();
    guard.pending_dispatch = Some(PendingDpiUpdate {
        dpi,
        updated_at: Instant::now(),
    });
    cvar.notify_one();
}

/// 读取内存中尚未落盘或正在下发的最新 DPI，避免 UI 在 200ms 防抖期内读到磁盘旧数据。
pub fn get_live_dpi() -> Option<u16> {
    let (lock, _) = get_live_dpi_channel();
    let guard = lock.lock().ok()?;
    let latest = guard
        .pending_dispatch
        .as_ref()
        .or(guard.pending_persist.as_ref())?;
    Some(latest.dpi)
}
