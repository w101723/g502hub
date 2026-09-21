//! 电池:feature 0x1001 (Battery Voltage) —— G502 LIGHTSPEED 实测支持。
//! 响应: [voltage_mV u16 BE][flags u8]
//! flags: bit7=充电中;bit0-1 子状态(0=充电中 1=已充满 2=未充电);bit3=快充;bit4=慢充;bit5=电量极低

use crate::device::G502Device;
use crate::hidpp::HidppError;

const F_UNIFIED_BATTERY: u16 = 0x1004;
const F_BATTERY_VOLTAGE: u16 = 0x1001;
const F_BATTERY_STATUS: u16 = 0x1000;

/// 电压 mV → 电量百分比(Solaar 线性近似表)
const VOLTAGE_CURVE: [(u16, u8); 13] = [
    (4186, 100),
    (4067, 90),
    (3989, 80),
    (3922, 70),
    (3859, 60),
    (3811, 50),
    (3778, 40),
    (3751, 30),
    (3717, 20),
    (3671, 10),
    (3646, 5),
    (3579, 2),
    (3500, 0),
];

#[derive(Debug, Clone)]
pub struct BatteryInfo {
    pub percent: u8,
    pub voltage_mv: Option<u16>,
    pub charging: bool,
    pub state_text: String,
}

fn percent_from_voltage(mv: u16) -> u8 {
    let first = VOLTAGE_CURVE[0];
    let last = VOLTAGE_CURVE[VOLTAGE_CURVE.len() - 1];
    if mv >= first.0 {
        return first.1;
    }
    if mv <= last.0 {
        return last.1;
    }
    for w in VOLTAGE_CURVE.windows(2) {
        let (v1, p1) = w[0];
        let (v2, p2) = w[1];
        if (v2..=v1).contains(&mv) {
            let ratio = (v1 - mv) as f32 / (v1 - v2) as f32;
            return (p1 as f32 + (p2 as i16 - p1 as i16) as f32 * ratio).round() as u8;
        }
    }
    0
}

fn read_voltage(dev: &G502Device) -> Option<Result<BatteryInfo, HidppError>> {
    let idx = dev.feature(F_BATTERY_VOLTAGE).ok()?;
    Some((|| {
        let resp = dev.request(idx, 0x00, &[])?;
        if resp.len() < 7 {
            return Err(HidppError::Invalid("电池响应过短".into()));
        }
        let mv = ((resp[4] as u16) << 8) | resp[5] as u16;
        let flags = resp[6];
        let charging = flags & 0x80 != 0;
        let sub = flags & 0x03;
        let mut state = match sub {
            1 => "已充满".to_string(),
            _ if charging => {
                if flags & 0x08 != 0 {
                    "充电中(快充)".to_string()
                } else if flags & 0x10 != 0 {
                    "充电中(慢充)".to_string()
                } else {
                    "充电中".to_string()
                }
            }
            _ => "放电中".to_string(),
        };
        if flags & 0x20 != 0 {
            state.push_str(" ⚠️电量极低");
        }
        Ok(BatteryInfo {
            percent: percent_from_voltage(mv),
            voltage_mv: Some(mv),
            charging,
            state_text: state,
        })
    })())
}

fn read_unified(dev: &G502Device) -> Option<Result<BatteryInfo, HidppError>> {
    let idx = dev.feature(F_UNIFIED_BATTERY).ok()?;
    Some((|| {
        let resp = dev.request(idx, 0x00, &[])?;
        let (level, _external, charge_state) =
            (resp[4], resp[5], resp.get(6).copied().unwrap_or(0));
        let percent = if level <= 100 {
            level
        } else {
            (level as u32 * 100 / 255) as u8
        };
        Ok(BatteryInfo {
            percent,
            voltage_mv: None,
            charging: charge_state == 0,
            state_text: match charge_state {
                0 => "充电中".into(),
                1 => "已充满".into(),
                2 => "放电中".into(),
                other => format!("状态{other}"),
            },
        })
    })())
}

fn read_status(dev: &G502Device) -> Option<Result<BatteryInfo, HidppError>> {
    let idx = dev.feature(F_BATTERY_STATUS).ok()?;
    Some((|| {
        let resp = dev.request(idx, 0x00, &[])?;
        let (level, state) = (resp[4], resp[5]);
        Ok(BatteryInfo {
            percent: level.min(100),
            voltage_mv: None,
            charging: state == 1,
            state_text: match state {
                0 => "放电中".into(),
                1 => "充电中".into(),
                2 => "几乎充满".into(),
                3 => "电量不足".into(),
                other => format!("状态{other}"),
            },
        })
    })())
}

/// 读取电池状态,优先 0x1001(实测支持),失败逐级回退。
pub fn read_battery(dev: &G502Device) -> Result<BatteryInfo, HidppError> {
    for reader in [read_voltage, read_unified, read_status] {
        if let Some(result) = reader(dev) {
            if result.is_ok() {
                return result;
            }
        }
    }
    Err(HidppError::Invalid(
        "设备不支持任何已知电池 feature (0x1000/0x1001/0x1004)".into(),
    ))
}
