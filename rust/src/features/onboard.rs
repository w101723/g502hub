//! 板载内存模式:feature 0x8100 (Onboard Profiles)。
//! GetOnboardMode = function 2(返回 1=onboard 2=host,实测)
//! SetOnboardMode = function 1(参数 1=onboard 2=host,libratbag 确认)

use crate::device::G502Device;
use crate::hidpp::HidppError;

const F_ONBOARD_PROFILES: u16 = 0x8100;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OnboardMode {
    Onboard,
    Host,
}

impl OnboardMode {
    pub fn as_u8(self) -> u8 {
        match self {
            OnboardMode::Onboard => 1,
            OnboardMode::Host => 2,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            OnboardMode::Onboard => "板载模式",
            OnboardMode::Host => "主机控制模式",
        }
    }
}

pub fn get_onboard_mode(dev: &G502Device) -> Option<Result<OnboardMode, HidppError>> {
    let idx = dev.feature(F_ONBOARD_PROFILES).ok()?;
    Some((|| {
        let resp = dev.request(idx, 0x02, &[])?;
        Ok(if resp[4] == 1 {
            OnboardMode::Onboard
        } else {
            OnboardMode::Host
        })
    })())
}

/// 切换板载/主机模式，并等待固件状态机稳定后读回确认。
pub fn set_onboard_mode(dev: &G502Device, mode: OnboardMode) -> Result<OnboardMode, HidppError> {
    let idx = dev.feature(F_ONBOARD_PROFILES)?;
    let mut last = None;
    for _ in 0..3 {
        dev.request(idx, 0x01, &[mode.as_u8(), 0x00, 0x00])?;
        std::thread::sleep(std::time::Duration::from_millis(40));
        match get_onboard_mode(dev) {
            Some(Ok(actual)) if actual == mode => return Ok(actual),
            Some(Ok(actual)) => last = Some(format!("实际为 {}", actual.name())),
            Some(Err(e)) => last = Some(e.to_string()),
            None => last = Some("设备不支持板载模式 feature".into()),
        }
        std::thread::sleep(std::time::Duration::from_millis(40));
    }
    Err(HidppError::Invalid(format!(
        "模式切换读回不一致: 请求 {}, {}",
        mode.name(),
        last.unwrap_or_else(|| "无读回".into())
    )))
}
