//! RGB 灯效:feature 0x8070 (Color LED Effects)。
//! 函数编号:f0 GetInfo  f1 GetZoneInfo(z)  f2 GetZoneEffectInfo(z,slot)
//!           f3 SetZoneEffect  f14 GetZoneEffect(z)
//! G502 LIGHTSPEED 有 2 个分区:zone 0 = 主要(灯带)、zone 1 = 标志(G logo)。
//!
//! 真机校准结论(2026-09,与 G HUB 设置库/protobuf 定义交叉验证):
//!   设备固定 4 个效果槽位(f2 枚举,两分区一致):
//!     slot 0 = off(0x0000)  slot 1 = solid(0x0001)
//!     slot 2 = cycle(0x0003) slot 3 = breathing(0x000A)
//!   f3 SetZoneEffect 的 16 字节 payload 按"目标槽位"解释,布局为:
//!     off:       [zone, 0, 0...]
//!     solid:     [zone, 1, R, G, B, 亮度(0-100)]           —— 正序 RGB,无 eid!
//!     cycle:     [zone, 2, 0x00,0x03, 强度, 饱和度, 周期hi, 周期lo]
//!     breathing: [zone, 3, 0x00,0x0A, R, G, B, 周期hi, 周期lo, 强度]
//!   注意:solid 的颜色直接从字节 2 开始(不能带效果 id 字节,
//!   否则 id 会占用 R/G 通道);cycle/breathing 的参数区以 2 字节
//!   效果 id 开头。周期单位毫秒、大端(1000 与 8000 肉眼快慢有别)。
//!   f14 读回恒为 0x0000,不可用于确认;写入只看传输层成功与否。
//! 速率以 period-ms 下发,0 表示使用设备默认节奏。

use crate::device::G502Device;
use crate::hidpp::HidppError;

const F_COLOR_LED_EFFECTS: u16 = 0x8070;

pub const ZONE_PRIMARY: u8 = 0;
pub const ZONE_LOGO: u8 = 1;

pub const RATE_SLOW_MS: u16 = 5000;
pub const RATE_MEDIUM_MS: u16 = 2000;
pub const RATE_FAST_MS: u16 = 1000;

/// 效果槽位/效果 id(真机 f2 枚举结果)。
pub const SLOT_OFF: u8 = 0;
pub const SLOT_SOLID: u8 = 1;
pub const SLOT_CYCLE: u8 = 2;
pub const SLOT_BREATHING: u8 = 3;

pub fn zone_from_key(key: &str) -> Option<u8> {
    match key {
        "primary" | "0" => Some(ZONE_PRIMARY),
        "logo" | "1" => Some(ZONE_LOGO),
        _ => None,
    }
}

pub fn zone_key(zone: u8) -> &'static str {
    match zone {
        ZONE_LOGO => "logo",
        _ => "primary",
    }
}

pub fn zone_label(zone: u8) -> &'static str {
    match zone {
        ZONE_LOGO => "标志",
        _ => "主要",
    }
}

#[allow(dead_code)]
pub fn effect_id(name: &str) -> Option<u16> {
    match name {
        "off" => Some(0x00),
        "solid" => Some(0x01),
        "cycle" => Some(0x03),
        "breathing" => Some(0x0A),
        _ => None,
    }
}

pub fn effect_name(id: u16) -> String {
    match id {
        0x00 => "off".into(),
        0x01 => "solid".into(),
        0x03 => "cycle".into(),
        0x0A => "breathing".into(),
        other => format!("{other:#06x}"),
    }
}

/// 设备实际支持的效果全集(顺序即展示顺序;off 由专门菜单项处理)。
#[allow(dead_code)]
pub const ALL_EFFECTS: &[u16] = &[0x01, 0x0A, 0x03];

/// 速率别名 → period-ms;"<毫秒>" 数字形式亦可;None/空 → 0(设备默认)。
pub fn rate_period_ms(rate: Option<&str>) -> Result<u16, HidppError> {
    let Some(rate) = rate else {
        return Ok(0);
    };
    match rate.trim().to_lowercase().as_str() {
        "" | "default" => Ok(0),
        "slow" | "慢" => Ok(RATE_SLOW_MS),
        "medium" | "mid" | "normal" | "中" => Ok(RATE_MEDIUM_MS),
        "fast" | "快" => Ok(RATE_FAST_MS),
        digits => digits.parse::<u16>().map_err(|_| {
            HidppError::Invalid(format!("无效速率: {rate} (slow/medium/fast 或毫秒数)"))
        }),
    }
}

pub struct Led<'a> {
    dev: &'a G502Device,
    index: u8,
}

impl<'a> Led<'a> {
    pub fn new(dev: &'a G502Device) -> Result<Self, HidppError> {
        let index = dev.feature(F_COLOR_LED_EFFECTS)?;
        Ok(Led { dev, index })
    }

    /// 设置灯效(发送即成功;f14 不回显,无法读回确认)。
    pub fn set_effect(
        &self,
        zone: u8,
        effect: &str,
        rgb: [u8; 3],
        brightness: u8,
        period_ms: u16,
    ) -> Result<(), HidppError> {
        match effect {
            "off" => self.set_off(zone),
            "solid" => self.set_solid(zone, rgb, brightness),
            "cycle" => self.set_cycle(zone, period_ms, brightness),
            "breathing" => self.set_breathing(zone, rgb, period_ms, brightness),
            other => Err(HidppError::Invalid(format!(
                "未知灯效: {other} (off/solid/cycle/breathing)"
            ))),
        }
    }

    /// 关闭分区灯效(slot 0,参数全零)。
    pub fn set_off(&self, zone: u8) -> Result<(), HidppError> {
        let params = [zone, SLOT_OFF, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        self.send_f3(&params)
    }

    /// 固定色:[zone, 1, R, G, B, 亮度(0-100)]。
    pub fn set_solid(&self, zone: u8, rgb: [u8; 3], brightness: u8) -> Result<(), HidppError> {
        let params: [u8; 16] = [
            zone,
            SLOT_SOLID,
            rgb[0],
            rgb[1],
            rgb[2],
            brightness.min(100),
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
        ];
        self.send_f3(&params)
    }

    /// 变色循环:[zone, 2, 0x0003, 强度, 饱和度, 周期(2B 大端 ms)]。
    pub fn set_cycle(&self, zone: u8, period_ms: u16, brightness: u8) -> Result<(), HidppError> {
        let intensity = intensity_byte(brightness);
        // 真机实测:周期 0 不渲染,0(即"默认速率")以 2000ms 下发
        let period = if period_ms == 0 {
            RATE_MEDIUM_MS
        } else {
            period_ms
        };
        let params: [u8; 16] = [
            zone,
            SLOT_CYCLE,
            0x00,
            0x03,
            intensity,
            0xFF,
            (period >> 8) as u8,
            period as u8,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
        ];
        self.send_f3(&params)
    }

    /// 呼吸:[zone, 3, 0x000A, R, G, B, 周期(2B 大端 ms), 强度]。
    pub fn set_breathing(
        &self,
        zone: u8,
        rgb: [u8; 3],
        period_ms: u16,
        brightness: u8,
    ) -> Result<(), HidppError> {
        let intensity = intensity_byte(brightness);
        // 真机实测:周期 0 不渲染,0(即"默认速率")以 2000ms 下发
        let period = if period_ms == 0 {
            RATE_MEDIUM_MS
        } else {
            period_ms
        };
        let params: [u8; 16] = [
            zone,
            SLOT_BREATHING,
            0x00,
            0x0A,
            rgb[0],
            rgb[1],
            rgb[2],
            (period >> 8) as u8,
            period as u8,
            intensity,
            0,
            0,
            0,
            0,
            0,
            0,
        ];
        self.send_f3(&params)
    }

    fn send_f3(&self, params: &[u8; 16]) -> Result<(), HidppError> {
        self.dev.request_long(self.index, 0x03, params)?;
        Ok(())
    }
}

/// 亮度百分比(0-100)→ 效果强度字节(0-255,cycle/breathing 用)。
fn intensity_byte(brightness: u8) -> u8 {
    ((brightness.min(100) as u16) * 255 / 100) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_effect_layout_constants() {
        assert_eq!(ALL_EFFECTS, &[0x01, 0x0A, 0x03]);
        assert_eq!(effect_id("solid"), Some(0x01));
        assert_eq!(effect_id("cycle"), Some(0x03));
        assert_eq!(effect_id("breathing"), Some(0x0A));
        assert_eq!(effect_id("off"), Some(0x00));
        assert_eq!(effect_id("wave"), None);
        assert_eq!(intensity_byte(100), 255);
        assert_eq!(intensity_byte(50), 127);
        assert_eq!(intensity_byte(0), 0);
        assert_eq!(SLOT_SOLID, 1);
        assert_eq!(SLOT_CYCLE, 2);
        assert_eq!(SLOT_BREATHING, 3);
    }
    #[test]
    fn test_rate_period_mapping() {
        assert_eq!(rate_period_ms(None).unwrap(), 0);
        assert_eq!(rate_period_ms(Some("")).unwrap(), 0);
        assert_eq!(rate_period_ms(Some("slow")).unwrap(), RATE_SLOW_MS);
        assert_eq!(rate_period_ms(Some("medium")).unwrap(), RATE_MEDIUM_MS);
        assert_eq!(rate_period_ms(Some("fast")).unwrap(), RATE_FAST_MS);
        assert_eq!(rate_period_ms(Some("1500")).unwrap(), 1500);
        assert!(rate_period_ms(Some("bogus")).is_err());
    }

    #[test]
    fn test_effect_and_zone_labels() {
        assert_eq!(effect_id("breathing"), Some(0x0A));
        assert_eq!(effect_name(0x0A), "breathing");
        assert_eq!(zone_from_key("primary"), Some(ZONE_PRIMARY));
        assert_eq!(zone_from_key("logo"), Some(ZONE_LOGO));
        assert_eq!(zone_from_key("bogus"), None);
        assert_eq!(zone_key(ZONE_LOGO), "logo");
        assert_eq!(zone_label(ZONE_PRIMARY), "主要");
    }
}
