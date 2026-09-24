//! 三格 DPI / 电量 / 配置档指示灯:feature 0x1300 (LED Software Control)。
//!
//! G502 LIGHTSPEED 侧边指示灯条由 3 颗物理 LED 构成 (physical_count = 3)。
//! 协议暴露 3 个逻辑指示器 (均复用这组物理 LED)：
//!   - index 0 = 电量 (Battery)
//!   - index 1 = DPI 档位 (DPI)
//!   - index 2 = 配置档 (Profile)
//!
//! 经真机与 LGHUB 原厂反编译校准：
//!   - `0x1300 f3 SetSWControl(true)`：主机接管指示灯软件控制权。
//!   - `0x1300 f3 SetSWControl(false)`：归还硬件板载/固件控制权。
//!   - `0x1300 f5 SetLEDState(index, mode, value, on_time, off_time)`：
//!     - mode = 0x0001 (OFF)：关闭指示灯。
//!     - mode = 0x0002 (ON)：value 传入 1..=3 表示点亮的格数；0xff 亦表示全亮。
//!       非 1..=3/0xff (如 0 或 100) 会被固件作为无效地址/参数拒绝。

use crate::device::G502Device;
use crate::hidpp::HidppError;

const F_LED_SW_CONTROL: u16 = 0x1300;

pub const BATTERY_LED_INDEX: u8 = 0;
#[allow(dead_code)]
pub const DPI_LED_INDEX: u8 = 1;
#[allow(dead_code)]
pub const PROFILE_LED_INDEX: u8 = 2;
pub const PRIMARY_SEGMENT_COUNT: u8 = 3;

const MODE_OFF: u16 = 0x0001;
const MODE_ON: u16 = 0x0002;

pub struct IndicatorLed<'a> {
    dev: &'a G502Device,
    pub(crate) index: u8,
}

impl<'a> IndicatorLed<'a> {
    pub fn new(dev: &'a G502Device) -> Result<Self, HidppError> {
        Ok(Self {
            dev,
            index: dev.feature(F_LED_SW_CONTROL)?,
        })
    }

    /// true = 主机软件接管；false = 归还固件控制。
    pub fn set_software_control(&self, enabled: bool) -> Result<(), HidppError> {
        for attempt in 0..3 {
            match self
                .dev
                .request(self.index, 0x03, &[u8::from(enabled), 0, 0])
            {
                Ok(_) => return Ok(()),
                Err(e) if attempt < 2 && crate::hidpp::is_transient_error(&e) => {
                    std::thread::sleep(std::time::Duration::from_millis(40));
                }
                Err(e) => return Err(e),
            }
        }
        unreachable!()
    }

    /// 点亮指定格数 (1..=3)。0 表示关闭全部指示灯。
    pub fn show_bars(&self, bars: u8) -> Result<(), HidppError> {
        self.set_software_control(true)?;
        match bars {
            0 => self.turn_off(),
            1..=3 => self.set_state(BATTERY_LED_INDEX, MODE_ON, bars as u16, 0, 0),
            _ => self.set_state(
                BATTERY_LED_INDEX,
                MODE_ON,
                PRIMARY_SEGMENT_COUNT as u16,
                0,
                0,
            ),
        }
    }

    /// 点亮全部 3 格指示灯（主要灯带效果）。
    pub fn show_all(&self) -> Result<(), HidppError> {
        self.show_bars(PRIMARY_SEGMENT_COUNT)
    }

    /// 关闭指示灯。
    pub fn turn_off(&self) -> Result<(), HidppError> {
        self.set_software_control(true)?;
        self.set_state(BATTERY_LED_INDEX, MODE_OFF, 0, 0, 0)
    }

    /// 归还固件硬件控制（切换到板载模式或退出程序时调用）。
    pub fn release(&self) -> Result<(), HidppError> {
        self.set_software_control(false)
    }

    fn set_state(
        &self,
        led_index: u8,
        mode: u16,
        logical_value: u16,
        on_time: u16,
        off_time: u16,
    ) -> Result<(), HidppError> {
        let params = state_params(led_index, mode, logical_value, on_time, off_time);
        for attempt in 0..3 {
            match self.dev.request_long(self.index, 0x05, &params) {
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

fn state_params(
    led_index: u8,
    mode: u16,
    logical_value: u16,
    on_time: u16,
    off_time: u16,
) -> [u8; 9] {
    [
        led_index,
        (mode >> 8) as u8,
        mode as u8,
        (logical_value >> 8) as u8,
        logical_value as u8,
        (on_time >> 8) as u8,
        on_time as u8,
        (off_time >> 8) as u8,
        off_time as u8,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_led_state_payloads() {
        assert_eq!(
            state_params(BATTERY_LED_INDEX, MODE_ON, 1, 0, 0),
            [0, 0, 2, 0, 1, 0, 0, 0, 0]
        );
        assert_eq!(
            state_params(BATTERY_LED_INDEX, MODE_ON, 2, 0, 0),
            [0, 0, 2, 0, 2, 0, 0, 0, 0]
        );
        assert_eq!(
            state_params(BATTERY_LED_INDEX, MODE_ON, 3, 0, 0),
            [0, 0, 2, 0, 3, 0, 0, 0, 0]
        );
        assert_eq!(
            state_params(BATTERY_LED_INDEX, MODE_OFF, 0, 0, 0),
            [0, 0, 1, 0, 0, 0, 0, 0, 0]
        );
    }

    #[test]
    #[ignore]
    fn live_show_all_bars() {
        let dev = crate::device::G502Device::open().expect("设备未连接");
        let indicator = IndicatorLed::new(&dev).expect("无 0x1300");
        indicator.show_all().expect("show_all 失败");
    }

    #[test]
    #[ignore]
    fn live_release_indicator_control() {
        let dev = crate::device::G502Device::open().expect("设备未连接");
        IndicatorLed::new(&dev)
            .expect("无 0x1300")
            .release()
            .expect("归还固件控制失败");
    }

    #[test]
    #[ignore]
    fn live_battery_level_readback() {
        let dev = crate::device::G502Device::open().expect("设备未连接");
        let indicator = IndicatorLed::new(&dev).expect("无 0x1300");
        let led = crate::features::led::Led::new(&dev).expect("无 0x8070");

        let result = (|| -> Result<(), HidppError> {
            led.set_solid(crate::features::led::ZONE_PRIMARY, [0, 255, 255], 100)?;
            indicator.set_software_control(true)?;
            std::thread::sleep(std::time::Duration::from_millis(80));

            for bars in [1u8, 2, 3] {
                indicator.show_bars(bars)?;
                std::thread::sleep(std::time::Duration::from_secs(1));
            }

            indicator.turn_off()?;
            std::thread::sleep(std::time::Duration::from_millis(500));
            Ok(())
        })();

        let cfg = crate::config::load().unwrap_or_default();
        if let Some(spec) = cfg.led_zones.get("primary") {
            let _ = crate::controller::apply_led_spec_inner(&dev, &led, 0, spec);
        }
        let release = indicator.release();
        result.unwrap();
        release.unwrap();
    }
}
