//! 设备期望状态控制器。
//!
//! HID++ 设置由单一操作锁串行化；模式和 DPI 只有读回一致才算成功。

use crate::config::{Config, DesiredMode};
use crate::device::G502Device;
use crate::features::dpi::Dpi;
use crate::features::onboard::{get_onboard_mode, set_onboard_mode, OnboardMode};
use crate::hidpp::HidppError;
use std::sync::Mutex;

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

/// 把配置中的期望状态重放到刚上线/唤醒的设备。
///
/// Host 模式下 DPI 属于易失状态，必须在每次重连后重新下发；Onboard 模式下
/// 固件配置接管，刻意不写 0x2201。
pub fn apply_desired_state(dev: &G502Device, cfg: &Config) -> Result<AppliedState, HidppError> {
    with_device_lock(|| {
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
    })
}

/// 把配置中的分区灯效重放到刚上线/唤醒的设备(仅首次连接调用)。
/// 逐分区尽力而为:单个分区失败(如旧配置含已不支持的效果)不影响其它分区。
pub fn apply_desired_led(dev: &G502Device, cfg: &Config) {
    let Ok(led) = crate::features::led::Led::new(dev) else {
        return;
    };
    for (key, spec) in &cfg.led_zones {
        let Some(zone) = crate::features::led::zone_from_key(key) else {
            continue;
        };
        let r = if spec.off {
            led.set_off(zone)
        } else {
            let period = crate::features::led::rate_period_ms(spec.rate.as_deref()).unwrap_or(0);
            led.set_effect(zone, &spec.effect, spec.rgb, spec.brightness, period)
        };
        if let Err(e) = r {
            eprintln!("恢复分区 {key} 灯效失败: {e}");
        }
    }
}
