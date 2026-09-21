//! RGB 灯效:feature 0x8070 (Color LED Effects)。
//! 函数编号(libratbag 确认):
//!   f0 GetInfo  f1 GetZoneInfo(z)  f2 GetZoneEffectInfo(z,e)  f3 SetZoneEffect  f14 GetZoneEffect(z)
//! 效果 id: 0 关闭 1 固定色 3 变色循环 4 波浪 5 星光 6 按压点亮 10 呼吸 11 涟漪
//! SetZoneEffect 参数布局按规范推断,首次使用时真机校准。

use crate::device::G502Device;
use crate::hidpp::HidppError;

const F_COLOR_LED_EFFECTS: u16 = 0x8070;

pub fn effect_id(name: &str) -> Option<u16> {
    match name {
        "off" => Some(0x00),
        "solid" => Some(0x01),
        "cycle" => Some(0x03),
        "wave" => Some(0x04),
        "starlight" => Some(0x05),
        "breathing" => Some(0x0A),
        "ripple" => Some(0x0B),
        _ => None,
    }
}

pub fn effect_name(id: u16) -> String {
    match id {
        0x00 => "off".into(),
        0x01 => "solid".into(),
        0x03 => "cycle".into(),
        0x04 => "wave".into(),
        0x05 => "starlight".into(),
        0x0A => "breathing".into(),
        0x0B => "ripple".into(),
        other => format!("{other:#06x}"),
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

    pub fn zone_count(&self) -> usize {
        self.dev
            .request(self.index, 0x00, &[])
            .map(|r| r[4] as usize)
            .unwrap_or(1)
            .max(1)
    }

    /// 读取当前效果 (f14 GetZoneEffect)。
    pub fn get_state(&self, zone: u8) -> Result<(u16, Option<[u8; 3]>), HidppError> {
        let resp = self
            .dev
            .request_long(self.index, 0x0E, &[zone, 0x00, 0x00])?;
        let effect = ((resp[5] as u16) << 8) | resp[6] as u16;
        let rgb = if effect == 0x01 {
            Some([resp[7], resp[8], resp[9]])
        } else {
            None
        };
        Ok((effect, rgb))
    }

    pub fn set_effect(
        &self,
        effect_name: &str,
        rgb: [u8; 3],
        brightness: u8,
        speed: u8,
        zone: u8,
    ) -> Result<(), HidppError> {
        let eid = effect_id(effect_name).ok_or_else(|| {
            HidppError::Invalid(format!(
                "未知灯效: {effect_name} (off/solid/cycle/wave/starlight/breathing/ripple)"
            ))
        })?;
        self.set_zone_effect(zone, eid, rgb, brightness, speed)
    }

    pub fn set_off(&self, zone: u8) -> Result<(), HidppError> {
        self.set_zone_effect(zone, 0x00, [0, 0, 0], 0, 0)
    }

    fn set_zone_effect(
        &self,
        zone: u8,
        effect_id: u16,
        rgb: [u8; 3],
        brightness: u8,
        speed: u8,
    ) -> Result<(), HidppError> {
        let params: [u8; 16] = [
            zone,
            0, // zone effect index
            (effect_id >> 8) as u8,
            effect_id as u8,
            0, // flags/persistency
            rgb[0],
            rgb[1],
            rgb[2],
            brightness.min(100),
            speed,
            0,
            0, // period ms
            0,
            0,
            0,
            0,
        ];
        self.dev.request_long(self.index, 0x03, &params)?;
        Ok(())
    }
}
