//! 电池:feature 0x1001 (Battery Voltage) —— G502 LIGHTSPEED 实测支持。
//! 响应: [voltage_mV u16 BE][flags u8]
//! flags: bit7=充电中;bit0-1 子状态(0=充电中 1=已充满 2=未充电);bit3=快充;bit4=慢充;bit5=电量极低

use crate::device::G502Device;
use crate::hidpp::HidppError;
use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

const F_UNIFIED_BATTERY: u16 = 0x1004;
const F_BATTERY_VOLTAGE: u16 = 0x1001;
const F_BATTERY_STATUS: u16 = 0x1000;

/// 放电电压曲线：电压 mV → 电量百分比 (来自罗技 G HUB 官方 G502 LIGHTSPEED battery.xml)
pub const DISCHARGE_VOLTAGE_CURVE: [(u16, u8); 13] = [
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

/// 充电电压参考曲线：恒流(CC)阶段电压 mV → 估算百分比 (来自 G HUB battery.xml <charging format="default">)
/// 注意：进入恒压(CV)阶段后，端电压稳定在 ~4200mV (4.20V)，对应基准约为 68%。
/// 从 68% 到 99% 由充入电荷 (时间积分) 持续推升，硬件信号 sub==1 ("已充满") 时才达到 100%。
pub const CHARGING_VOLTAGE_CURVE: [(u16, u8); 25] = [
    (4200, 68),
    (4197, 67),
    (4190, 65),
    (4181, 63),
    (4172, 62),
    (4163, 60),
    (4155, 58),
    (4147, 57),
    (4139, 55),
    (4132, 53),
    (4125, 52),
    (4118, 50),
    (4104, 47),
    (4098, 45),
    (4085, 42),
    (4073, 38),
    (4061, 35),
    (4048, 32),
    (4036, 28),
    (4023, 25),
    (4007, 22),
    (3988, 18),
    (3965, 15),
    (3929, 10),
    (3786, 5),
];

#[derive(Debug, Clone)]
pub struct BatteryInfo {
    pub percent: u8,
    pub voltage_mv: Option<u16>,
    pub charging: bool,
    pub state_text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PersistentBatteryState {
    pub last_discharge_percent: u8,
    pub last_discharge_mv: u16,
    pub last_discharge_timestamp: u64,
    pub is_charging: bool,
    pub charge_start_timestamp: Option<u64>,
    pub charge_start_percent: Option<u8>,
    pub last_reported_percent: u8,
    pub last_updated_timestamp: u64,
}

impl Default for PersistentBatteryState {
    fn default() -> Self {
        Self {
            last_discharge_percent: 50,
            last_discharge_mv: 3811,
            last_discharge_timestamp: 0,
            is_charging: false,
            charge_start_timestamp: None,
            charge_start_percent: None,
            last_reported_percent: 50,
            last_updated_timestamp: 0,
        }
    }
}

pub fn percent_from_discharge_voltage(mv: u16) -> u8 {
    interpolate_curve(mv, &DISCHARGE_VOLTAGE_CURVE)
}

pub fn percent_from_charging_voltage(mv: u16) -> u8 {
    interpolate_curve(mv, &CHARGING_VOLTAGE_CURVE)
}

fn interpolate_curve(mv: u16, curve: &[(u16, u8)]) -> u8 {
    if curve.is_empty() {
        return 0;
    }
    let first = curve[0];
    let last = curve[curve.len() - 1];
    if mv >= first.0 {
        return first.1;
    }
    if mv <= last.0 {
        return last.1;
    }
    for w in curve.windows(2) {
        let (v1, p1) = w[0];
        let (v2, p2) = w[1];
        if (v2..=v1).contains(&mv) {
            let ratio = (v1 - mv) as f32 / (v1 - v2) as f32;
            return (p1 as f32 + (p2 as i16 - p1 as i16) as f32 * ratio).round() as u8;
        }
    }
    0
}

fn current_timestamp_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn load_battery_state() -> Option<PersistentBatteryState> {
    let path = crate::config::battery_state_path();
    let text = std::fs::read_to_string(&path).ok()?;
    serde_json::from_str(&text).ok()
}

fn save_battery_state(st: &PersistentBatteryState) {
    let path = crate::config::battery_state_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(st) {
        let temp = path.with_extension(format!("tmp.{}", std::process::id()));
        if std::fs::write(&temp, json).is_ok() {
            let _ = std::fs::rename(temp, path);
        }
    }
}

static BATTERY_TRACKER: Mutex<Option<PersistentBatteryState>> = Mutex::new(None);

pub fn calculate_charging_percent_inner(
    start_p: u8,
    elapsed_secs: u64,
    flags: u8,
    sub: u8,
    last_reported: u8,
) -> u8 {
    if sub == 1 {
        return 100;
    }
    let rate_secs = if flags & 0x08 != 0 {
        50
    } else if flags & 0x10 != 0 {
        120
    } else {
        60
    };
    let gain = (elapsed_secs / rate_secs) as u8;
    let mut current = start_p.saturating_add(gain);
    if current >= 100 {
        current = 99;
    }
    current.max(last_reported).min(99)
}

pub fn resolve_battery_percent(mv: u16, charging: bool, sub: u8, flags: u8) -> u8 {
    let mut lock = BATTERY_TRACKER.lock().unwrap_or_else(|e| e.into_inner());
    let state = lock.get_or_insert_with(|| {
        load_battery_state().unwrap_or_else(|| {
            let now = current_timestamp_secs();
            let initial_dis = percent_from_discharge_voltage(mv);
            PersistentBatteryState {
                last_discharge_percent: initial_dis,
                last_discharge_mv: mv,
                last_discharge_timestamp: now,
                is_charging: charging,
                charge_start_timestamp: if charging { Some(now) } else { None },
                charge_start_percent: if charging {
                    Some(if mv >= 4200 {
                        68
                    } else {
                        percent_from_charging_voltage(mv)
                    })
                } else {
                    None
                },
                last_reported_percent: if charging {
                    if sub == 1 {
                        100
                    } else if mv >= 4200 {
                        68
                    } else {
                        percent_from_charging_voltage(mv)
                    }
                } else {
                    initial_dis
                },
                last_updated_timestamp: now,
            }
        })
    });

    let now = current_timestamp_secs();

    // 1. 硬件回报充满电 (sub == 1)
    if sub == 1 {
        state.is_charging = charging;
        state.last_discharge_percent = 100;
        state.last_discharge_mv = mv;
        state.last_reported_percent = 100;
        state.last_updated_timestamp = now;
        save_battery_state(state);
        return 100;
    }

    // 2. 放电状态 (!charging)
    if !charging {
        let raw_p = percent_from_discharge_voltage(mv);
        // 如果此前是连续放电，防止电压轻微回弹抖动（单调不增，除非跳变 >= 5% 判定为充过电断开）
        let p = if !state.is_charging
            && state.last_reported_percent > 0
            && raw_p > state.last_reported_percent
        {
            if raw_p.saturating_sub(state.last_reported_percent) >= 5 {
                raw_p
            } else {
                state.last_reported_percent
            }
        } else {
            raw_p
        };

        state.is_charging = false;
        state.last_discharge_percent = p;
        state.last_discharge_mv = mv;
        state.last_discharge_timestamp = now;
        state.charge_start_timestamp = None;
        state.charge_start_percent = None;
        state.last_reported_percent = p;
        state.last_updated_timestamp = now;
        save_battery_state(state);
        return p;
    }

    // 3. 充电中 (charging && sub != 1)
    // 如果刚刚从放电转为充电，或者初次记录充电起始
    if !state.is_charging || state.charge_start_percent.is_none() {
        let start_p = if state.last_discharge_timestamp > 0
            && now.saturating_sub(state.last_discharge_timestamp) < 86400
        {
            state.last_discharge_percent
        } else if mv < 4200 {
            percent_from_charging_voltage(mv)
        } else {
            68
        };
        state.is_charging = true;
        state.charge_start_timestamp = Some(now);
        state.charge_start_percent = Some(start_p);
        state.last_reported_percent = start_p.min(99);
    }

    let start_p = state
        .charge_start_percent
        .unwrap_or(state.last_discharge_percent);
    let start_t = state.charge_start_timestamp.unwrap_or(now);
    let elapsed = now.saturating_sub(start_t);

    let result =
        calculate_charging_percent_inner(start_p, elapsed, flags, sub, state.last_reported_percent);
    state.last_reported_percent = result;
    state.last_updated_timestamp = now;
    save_battery_state(state);
    result
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
        let percent = resolve_battery_percent(mv, charging, sub, flags);
        Ok(BatteryInfo {
            percent,
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
    let mut first_error = None;
    for reader in [read_voltage, read_unified, read_status] {
        if let Some(result) = reader(dev) {
            match result {
                Ok(info) => return Ok(info),
                Err(e) => {
                    if first_error.is_none() {
                        first_error = Some(e);
                    }
                }
            }
        }
    }
    Err(first_error.unwrap_or_else(|| {
        HidppError::Invalid("设备不支持任何已知电池 feature (0x1000/0x1001/0x1004)".into())
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_discharge_curve_endpoints_and_interpolation() {
        assert_eq!(percent_from_discharge_voltage(4300), 100);
        assert_eq!(percent_from_discharge_voltage(4186), 100);
        assert_eq!(percent_from_discharge_voltage(3500), 0);
        assert_eq!(percent_from_discharge_voltage(3200), 0);
        assert_eq!(percent_from_discharge_voltage(3811), 50);
        assert_eq!(percent_from_discharge_voltage(3765), 35);
    }

    #[test]
    fn test_charging_curve_endpoints_and_interpolation() {
        assert_eq!(percent_from_charging_voltage(4300), 68);
        assert_eq!(percent_from_charging_voltage(4200), 68);
        assert_eq!(percent_from_charging_voltage(4061), 35);
        assert_eq!(percent_from_charging_voltage(3786), 5);
        assert_eq!(percent_from_charging_voltage(3600), 5);
    }

    #[test]
    fn test_calculate_charging_percent_inner() {
        // 初始 35%，充了 0 秒，不能是 100%
        let p0 = calculate_charging_percent_inner(35, 0, 0x88, 0, 35);
        assert_eq!(p0, 35);

        // 快充 (flags=0x88)，充了 500 秒 (10 个 50s 步进) -> 35 + 10 = 45%
        let p1 = calculate_charging_percent_inner(35, 500, 0x88, 0, 35);
        assert_eq!(p1, 45);

        // 持续充了极长时间 (30000秒)，只要硬件未报告 sub==1，上限严格锁定为 99%
        let p_long = calculate_charging_percent_inner(35, 30000, 0x88, 0, 45);
        assert_eq!(p_long, 99);

        // 当硬件报告 sub==1 (已充满) 时，立刻报告 100%
        let p_full = calculate_charging_percent_inner(35, 30000, 0x88, 1, 99);
        assert_eq!(p_full, 100);
    }
}
