//! 设备期望状态控制器。
//!
//! HID++ 设置由单一操作锁串行化；模式和 DPI 只有读回一致才算成功。

use crate::config::{Config, DesiredMode};
use crate::device::G502Device;
use crate::features::dpi::Dpi;
use crate::features::led::{self, Led};
use crate::features::onboard::{get_onboard_mode, set_onboard_mode, OnboardMode};
use crate::hidpp::HidppError;
use std::sync::Mutex;
use std::time::Duration;

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
    with_device_lock(|| set_onboard_mode(dev, mode))
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

fn apply_led_spec_inner(
    led: &Led<'_>,
    zone: u8,
    spec: &crate::config::LedSpec,
) -> Result<(), HidppError> {
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
        apply_led_spec_inner(&led, zone, spec)?;
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
        apply_led_spec_inner(&led, zone, spec)
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
