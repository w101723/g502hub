//! G HUB 风格的临时机身电量指示。
//!
//! G HUB 的 `battery_lighting::on_start_effect(int)` 会按 16% / 30% / 50% 分档：
//!   - < 16%：1 格红色呼吸（周期 1000ms 紧急警示）
//!   - 16%–30%：1 格橙色常亮（低电量提示）
//!   - 31%–50%：2 格绿色常亮（中等电量）
//!   - 51%–100%：3 格绿色常亮（电量充足）
//!
//! 临时效果通过 HID++ 0x1300 (控制 1/2/3 亮灯格数) + 0x8070 (控制颜色与呼吸模式)
//! 协同显示，约 2.2 秒后自动恢复用户原设定的主要灯效与格数。

use crate::config::{Config, LedSpec};
use crate::controller;
use crate::device::G502Device;
use crate::features::indicator_led::IndicatorLed;
use crate::features::led::{self, Led};
use crate::features::onboard::OnboardMode;
use crate::hidpp::HidppError;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

pub const INDICATOR_DURATION_MS: u64 = 2200;

static INDICATOR_GENERATION: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatteryIndicatorTier {
    Critical,
    Low,
    Normal,
}

pub fn tier_for_percent(percent: u8) -> BatteryIndicatorTier {
    if percent < 16 {
        BatteryIndicatorTier::Critical
    } else if percent <= 30 {
        BatteryIndicatorTier::Low
    } else {
        BatteryIndicatorTier::Normal
    }
}

/// 根据当前电量百分比计算应点亮的物理灯格数量 (1..=3)。
pub fn bars_for_percent(percent: u8) -> u8 {
    if percent <= 30 {
        1
    } else if percent <= 50 {
        2
    } else {
        3
    }
}

fn indicator_spec(percent: u8) -> LedSpec {
    match tier_for_percent(percent) {
        BatteryIndicatorTier::Critical => {
            // G HUB 软件灯光层使用 500ms；直接下发 0x8070 时设备的安全下限为 1000ms。
            LedSpec::new("breathing", [255, 0, 0], 100, Some("1000"))
        }
        BatteryIndicatorTier::Low => LedSpec::new("solid", [255, 160, 0], 100, None),
        BatteryIndicatorTier::Normal => LedSpec::new("solid", [0, 255, 0], 100, None),
    }
}

fn apply_rgb_spec(led: &Led<'_>, spec: &LedSpec) -> Result<(), HidppError> {
    if spec.off {
        led.set_off(led::ZONE_PRIMARY)
    } else {
        let period = led::rate_period_ms(spec.rate.as_deref())?;
        led.set_effect(
            led::ZONE_PRIMARY,
            &spec.effect,
            spec.rgb,
            spec.brightness,
            period,
        )
    }
}

/// 在主要/指示灯分区显示临时电量效果（格数 + 颜色），结束后恢复用户配置。
///
/// 为保证恢复准确，配置中必须存在 `primary` 灯效。显示和恢复各自短暂持有
/// 设备操作锁，等待期间允许模式切换和 RGB 调节；连续触发时仅最新一次负责恢复。
pub fn show_battery_level(dev: &G502Device, percent: u8, cfg: &Config) -> Result<(), HidppError> {
    let restore = controller::get_live_spec_for_zone("primary")
        .or_else(|| cfg.led_zones.get("primary").cloned())
        .ok_or_else(|| HidppError::Invalid("缺少主要灯区配置，无法安全恢复临时电量灯效".into()))?;
    let indicator = indicator_spec(percent);
    let bars = bars_for_percent(percent);
    let generation = INDICATOR_GENERATION.fetch_add(1, Ordering::SeqCst) + 1;

    controller::with_device_lock(|| {
        let led = Led::new(dev)?;
        IndicatorLed::new(dev)?.show_bars(bars)?;
        std::thread::sleep(Duration::from_millis(12));
        apply_rgb_spec(&led, &indicator)
    })?;

    std::thread::sleep(Duration::from_millis(INDICATOR_DURATION_MS));
    if INDICATOR_GENERATION.load(Ordering::SeqCst) != generation {
        return Ok(());
    }

    controller::with_device_lock(|| {
        if controller::read_mode(dev)? == OnboardMode::Host {
            let led = Led::new(dev)?;
            controller::apply_led_spec_inner(dev, &led, led::ZONE_PRIMARY, &restore)
        } else {
            IndicatorLed::new(dev)?.release()
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_battery_indicator_thresholds_match_ghub() {
        assert_eq!(tier_for_percent(0), BatteryIndicatorTier::Critical);
        assert_eq!(tier_for_percent(15), BatteryIndicatorTier::Critical);
        assert_eq!(tier_for_percent(16), BatteryIndicatorTier::Low);
        assert_eq!(tier_for_percent(30), BatteryIndicatorTier::Low);
        assert_eq!(tier_for_percent(31), BatteryIndicatorTier::Normal);
        assert_eq!(tier_for_percent(100), BatteryIndicatorTier::Normal);
    }

    #[test]
    fn test_battery_bars_mapping() {
        assert_eq!(bars_for_percent(0), 1);
        assert_eq!(bars_for_percent(15), 1);
        assert_eq!(bars_for_percent(16), 1);
        assert_eq!(bars_for_percent(30), 1);
        assert_eq!(bars_for_percent(31), 2);
        assert_eq!(bars_for_percent(50), 2);
        assert_eq!(bars_for_percent(51), 3);
        assert_eq!(bars_for_percent(71), 3);
        assert_eq!(bars_for_percent(100), 3);
    }

    #[test]
    fn test_indicator_specs() {
        let critical = indicator_spec(15);
        assert_eq!(critical.effect, "breathing");
        assert_eq!(critical.rgb, [255, 0, 0]);
        assert_eq!(critical.rate.as_deref(), Some("1000"));

        let low = indicator_spec(20);
        assert_eq!(low.effect, "solid");
        assert_eq!(low.rgb, [255, 160, 0]);

        let normal = indicator_spec(80);
        assert_eq!(normal.effect, "solid");
        assert_eq!(normal.rgb, [0, 255, 0]);
    }
}
