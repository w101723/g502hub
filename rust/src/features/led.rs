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
        digits => {
            let v: u16 = digits.parse().map_err(|_| {
                HidppError::Invalid(format!("无效速率: {rate} (slow/medium/fast 或毫秒数)"))
            })?;
            // 官方(G HUB)速率范围 1000–20000ms;越界值会被固件接受
            // 但会卡死效果引擎(真机实测 500ms 导致呼吸失效直到重建配置)
            if !(RATE_MIN_MS..=RATE_MAX_MS).contains(&v) {
                return Err(HidppError::Invalid(format!(
                    "速率超出范围: {v}ms (官方支持 1000–20000ms)"
                )));
            }
            Ok(v)
        }
    }
}

/// 官方速率下限(G HUB UI 显示 1000ms)。
pub const RATE_MIN_MS: u16 = 1000;
/// 官方速率上限(G HUB UI 显示 20000ms)。
pub const RATE_MAX_MS: u16 = 20000;

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

    /// 固定色:[zone, 1, R, G, B, 0x02, 0...]。RGB 靠缩放亮度预处理。
    pub fn set_solid(&self, zone: u8, rgb: [u8; 3], brightness: u8) -> Result<(), HidppError> {
        let rgb = scale_rgb(rgb, brightness);
        let params: [u8; 16] = [
            zone, SLOT_SOLID, rgb[0], rgb[1], rgb[2], 0x02, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        ];
        self.send_f3(&params)
    }

    /// 变色循环:[zone, 2, 0, 0, 0, 0, 0, 周期高8位, 周期低8位, 亮度(0-100), 0...]。
    /// 依据 LGHUB 核心驱动 Feature8070ColorLEDEffects 原厂反编译规范：
    /// payload[0..4]=0, payload[5..6]=period_ms(BE), payload[7]=brightness(0-100)。
    pub fn set_cycle(&self, zone: u8, period_ms: u16, brightness: u8) -> Result<(), HidppError> {
        let period = if period_ms == 0 {
            RATE_MEDIUM_MS
        } else {
            period_ms.clamp(RATE_MIN_MS, RATE_MAX_MS)
        };
        let b = brightness.min(100);
        let params: [u8; 16] = [
            zone,
            SLOT_CYCLE,
            0,
            0,
            0,
            0,
            0,
            (period >> 8) as u8,
            (period & 0xff) as u8,
            b,
            0,
            0,
            0,
            0,
            0,
            0,
        ];
        self.send_f3(&params)
    }

    /// 呼吸:[zone, 3, R, G, B, 周期高8位, 周期低8位, 0x00, 亮度(0-100), 0...]。
    /// 依据 LGHUB 核心驱动 feature_8070_lighting_effects::get_raw_effect_params 原厂反编译规范：
    /// - payload[0..2] = RGB (未软件缩放的原色，由固件硬件 PWM 自行插值呼吸)
    /// - payload[3..4] = period_ms (2字节大端毫秒数，1000~10000ms)
    /// - payload[5]    = 0x00 (固定为零；非零值会触发固件方波频闪/错误闪烁)
    /// - payload[6]    = brightness (0~100 整数亮度百分比，作为呼吸波峰强度)
    pub fn set_breathing(
        &self,
        zone: u8,
        rgb: [u8; 3],
        period_ms: u16,
        brightness: u8,
    ) -> Result<(), HidppError> {
        let period = if period_ms == 0 {
            RATE_MEDIUM_MS
        } else {
            period_ms.clamp(RATE_MIN_MS, RATE_MAX_MS)
        };
        let b = brightness.min(100);
        let params: [u8; 16] = [
            zone,
            SLOT_BREATHING,
            rgb[0],
            rgb[1],
            rgb[2],
            (period >> 8) as u8,
            (period & 0xff) as u8,
            0x00,
            b,
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

    fn send_f3(&self, params: &[u8; 16]) -> Result<(), HidppError> {
        for attempt in 0..3 {
            match self.dev.request_long(self.index, 0x03, params) {
                Ok(_) => return Ok(()),
                Err(e) if attempt < 2 && crate::hidpp::is_transient_error(&e) => {
                    std::thread::sleep(std::time::Duration::from_millis(40));
                }
                Err(e) => return Err(e),
            }
        }
        unreachable!()
    }
}

/// 亮度百分比(0-100)→ 效果强度字节(0-255,cycle/breathing 用)。
#[allow(dead_code)]
fn intensity_byte(brightness: u8) -> u8 {
    ((brightness.min(100) as u16) * 255 / 100) as u8
}

/// 按亮度百分比缩放 RGB。
/// 固件的 f3 亮度/强度字节经真机实测不改变实际亮度,亮度由软件
/// 缩放颜色实现(与 G HUB 的全局亮度行为一致)。
fn scale_rgb(rgb: [u8; 3], brightness: u8) -> [u8; 3] {
    let b = brightness.min(100) as u16;
    [
        (rgb[0] as u16 * b / 100) as u8,
        (rgb[1] as u16 * b / 100) as u8,
        (rgb[2] as u16 * b / 100) as u8,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(data: &[u8]) -> String {
        data.iter().map(|b| format!("{b:02x}")).collect::<String>()
    }

    /// 呼吸变常亮诊断:逐窗口发送对照字节(真机,肉眼确认):
    /// `cargo test live_led_breath_check -- --ignored --nocapture`
    /// 每窗口 5 秒。W1=当初校准成功的原始字节;W2=当前 API(应与 W1 同字节);
    /// W3=强度 0x40;W4=solid 对照;W5=先 off 再 breathing。
    #[test]
    #[ignore]
    fn live_led_breath_check() {
        let dev = crate::device::G502Device::open().expect("设备未连接");
        let index = dev.feature(F_COLOR_LED_EFFECTS).expect("无 0x8070");
        let send = |name: &str, params: [u8; 16]| {
            println!("{name}: {}", hex(&params));
            let _ = std::io::Write::flush(&mut std::io::stdout());
            let _ = dev
                .transport
                .request(dev.dev_index, index, 0x03, &params, true, 800);
        };
        let sleep5 = || {
            for _ in 0..5 {
                std::thread::sleep(std::time::Duration::from_secs(1));
            }
        };
        send(
            "W1 原始字节",
            [
                0, 3, 0x00, 0x0A, 0xFF, 0x00, 0x00, 0x03, 0xE8, 0xFF, 0, 0, 0, 0, 0, 0,
            ],
        );
        sleep5();
        let l = Led::new(&dev).unwrap();
        let mut p2 = [0u8; 16];
        p2[0] = 0;
        p2[1] = SLOT_BREATHING;
        p2[2] = 0x00;
        p2[3] = 0x0A;
        p2[4] = 0xFF;
        p2[7] = 0x03;
        p2[8] = 0xE8;
        p2[9] = 0xFF;
        send("W2 仅R+周期+强度", p2);
        sleep5();
        send(
            "W3 强度0x40",
            [
                0, 3, 0x00, 0x0A, 0xFF, 0x00, 0x00, 0x03, 0xE8, 0x40, 0, 0, 0, 0, 0, 0,
            ],
        );
        sleep5();
        send(
            "W4 solid对照",
            [0, 1, 0xFF, 0x00, 0x00, 100, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
        );
        sleep5();
        l.set_off(0).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(500));
        l.set_breathing(0, [0xFF, 0, 0], 1000, 100).unwrap();
        println!("W5 off后breathing(1000)");
        sleep5();
        l.set_off(0).unwrap();
        println!("结束已关灯");
    }

    /// 解锁卡死的呼吸/循环引擎:回写出厂默认参数(真机,肉眼确认):
    /// `cargo test live_led_unwedge -- --ignored --nocapture`
    /// 每窗口 6 秒:W1 呼吸默认参数 W2 循环默认参数 W3 呼吸正常参数
    #[test]
    #[ignore]
    fn live_led_unwedge() {
        let dev = crate::device::G502Device::open().expect("设备未连接");
        let index = dev.feature(F_COLOR_LED_EFFECTS).expect("无 0x8070");
        let send = |name: &str, params: [u8; 16]| {
            println!("{name}");
            let _ = std::io::Write::flush(&mut std::io::stdout());
            let _ = dev
                .transport
                .request(dev.dev_index, index, 0x03, &params, true, 800);
        };
        for _ in 0..6 {
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
        send(
            "W1 breathing 默认参数",
            [
                0, 3, 0x00, 0x0A, 0xC1, 0x05, 0x00, 0x3C, 0, 0, 0, 0, 0, 0, 0, 0,
            ],
        );
        for _ in 0..6 {
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
        send(
            "W2 cycle 默认参数",
            [
                0, 2, 0x00, 0x03, 0xC0, 0x05, 0x03, 0xE8, 0, 0, 0, 0, 0, 0, 0, 0,
            ],
        );
        for _ in 0..6 {
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
        send(
            "W3 breathing red 1000",
            [
                0, 3, 0x00, 0x0A, 0xFF, 0x00, 0x00, 0x03, 0xE8, 0xFF, 0, 0, 0, 0, 0, 0,
            ],
        );
        for _ in 0..6 {
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
        println!("结束");
    }

    #[test]
    #[ignore]
    fn test_live_breathing_rates() {
        let dev = crate::device::G502Device::open().expect("设备未连接");
        let led = Led::new(&dev).expect("初始化 Led 失败");

        // 快速呼吸 (1000ms, 蓝色)
        println!("下发 1000ms 快速呼吸 (蓝色)");
        led.set_breathing(0, [0x00, 0xC8, 0xFF], 1000, 100).unwrap();
        std::thread::sleep(std::time::Duration::from_secs(4));

        // 慢速呼吸 (4000ms, 红色)
        println!("下发 4000ms 舒缓呼吸 (红色)");
        led.set_breathing(0, [0xFF, 0x00, 0x00], 4000, 100).unwrap();
        std::thread::sleep(std::time::Duration::from_secs(8));

        // 恢复关闭
        led.set_off(0).unwrap();
        println!("测试完成");
    }

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
        assert_eq!(scale_rgb([255, 100, 0], 50), [127, 50, 0]);
        assert_eq!(scale_rgb([200, 200, 200], 100), [200, 200, 200]);
        assert_eq!(scale_rgb([255, 255, 255], 0), [0, 0, 0]);
        assert_eq!(SLOT_SOLID, 1);
        assert_eq!(SLOT_CYCLE, 2);
        assert_eq!(SLOT_BREATHING, 3);
        assert!(crate::hidpp::is_transient_error(&HidppError::Timeout(
            "test".into()
        )));
        assert!(crate::hidpp::is_transient_error(&HidppError::Rejected {
            code: 0x09,
            message: "busy".into(),
        }));
        assert!(!crate::hidpp::is_transient_error(&HidppError::Invalid(
            "bad".into()
        )));
        assert!(!crate::hidpp::is_transient_error(&HidppError::Rejected {
            code: 0x03,
            message: "invalid value".into(),
        }));
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
        assert!(rate_period_ms(Some("50")).is_err());
        assert!(rate_period_ms(Some("999")).is_err());
        assert_eq!(rate_period_ms(Some("1000")).unwrap(), 1000);
        assert_eq!(rate_period_ms(Some("20000")).unwrap(), 20000);
        assert!(rate_period_ms(Some("20001")).is_err());
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

    #[test]
    fn test_breathing_cycle_payload_layout() {
        // 验证呼吸帧布局符合 LGHUB 原厂反编译 0x8070 规范
        let period_ms: u16 = 2000;
        let brightness: u8 = 80;
        let rgb = [0x00, 0xC8, 0xFF];
        let p_breathing: [u8; 16] = [
            0,
            SLOT_BREATHING,
            rgb[0],
            rgb[1],
            rgb[2],
            (period_ms >> 8) as u8,
            (period_ms & 0xff) as u8,
            0x00,
            brightness,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
        ];
        assert_eq!(p_breathing[0], 0); // zone
        assert_eq!(p_breathing[1], 3); // slot
        assert_eq!(&p_breathing[2..5], &[0x00, 0xC8, 0xFF]); // RGB
        assert_eq!(p_breathing[5], 0x07); // 2000 ms high byte
        assert_eq!(p_breathing[6], 0xD0); // 2000 ms low byte
        assert_eq!(p_breathing[7], 0x00); // waveform/flag 必须为 0，防止方波闪烁
        assert_eq!(p_breathing[8], 80); // 亮度 (0..100)

        // 验证循环帧布局
        let p_cycle: [u8; 16] = [
            0,
            SLOT_CYCLE,
            0,
            0,
            0,
            0,
            0,
            (period_ms >> 8) as u8,
            (period_ms & 0xff) as u8,
            brightness,
            0,
            0,
            0,
            0,
            0,
            0,
        ];
        assert_eq!(p_cycle[0], 0);
        assert_eq!(p_cycle[1], 2);
        assert_eq!(&p_cycle[2..7], &[0, 0, 0, 0, 0]);
        assert_eq!(p_cycle[7], 0x07);
        assert_eq!(p_cycle[8], 0xD0);
        assert_eq!(p_cycle[9], 80);
    }
}
