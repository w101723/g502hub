//! 协议探测(只读,不发 SET)。

use crate::device::G502Device;
use crate::hidpp::HidppError;
use anyhow::Result;

const KNOWN: &[(u16, &str)] = &[
    (0x0001, "FeatureSet"),
    (0x0003, "DeviceName"),
    (0x0005, "FirmwareVersion"),
    (0x1000, "BatteryStatus"),
    (0x1001, "BatteryVoltage"),
    (0x1004, "UnifiedBattery"),
    (0x1300, "LedSoftwareControl"),
    (0x2200, "MousePointerBasic"),
    (0x2201, "AdjustableDPI"),
    (0x2202, "MouseButtonSpy"),
    (0x8060, "ReportRate"),
    (0x8070, "ColorLEDEffects"),
    (0x8100, "OnboardProfiles"),
];

fn known_name(id: u16) -> &'static str {
    KNOWN
        .iter()
        .find(|(k, _)| *k == id)
        .map(|(_, n)| *n)
        .unwrap_or("")
}

fn raw_probe(dev: &G502Device, feature_id: u16) {
    let Ok(idx) = dev.feature(feature_id) else {
        println!("{feature_id:#06x}: 不支持");
        return;
    };
    println!("== {feature_id:#06x} (index {idx:#04x}) ==");
    for function in 0u8..=2 {
        for kind in ["short", "long"] {
            let r = if kind == "short" {
                dev.transport
                    .request(dev.dev_index, idx, function, &[], false, 800)
            } else {
                dev.transport
                    .request(dev.dev_index, idx, function, &[], true, 800)
            };
            match r {
                Ok(resp) => println!("  f{function} {kind:5} → {}", hex(&resp)),
                Err(HidppError::Rejected { code, message }) => {
                    println!("  f{function} {kind:5} → ✗ {message} ({code:#04x})")
                }
                Err(HidppError::Timeout(_)) => println!("  f{function} {kind:5} → ⚠ 超时"),
                Err(e) => {
                    println!("  f{function} {kind:5} → ⚠ {e}");
                    return;
                }
            }
        }
    }
}

fn hex(data: &[u8]) -> String {
    data.iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// 0x1300 非 RGB 指示灯能力深探测(只读)。
fn indicator_led_probe(dev: &G502Device) {
    let root = dev.request(0x00, 0x00, &[0x13, 0x00, 0x00]);
    println!(
        "  Root.GetFeature(0x1300) → {}",
        match root {
            Ok(resp) => hex(&resp),
            Err(e) => format!("✗ {e}"),
        }
    );
    let Ok(idx) = dev.feature(0x1300) else {
        return;
    };
    let count_response = dev
        .transport
        .request(dev.dev_index, idx, 0x00, &[], false, 800);
    let count = match count_response {
        Ok(resp) => {
            println!("  0x1300 f0 GetLedCount → {}", hex(&resp));
            resp.get(4).copied().unwrap_or(0)
        }
        Err(e) => {
            println!("  0x1300 f0 GetLedCount → ✗ {e}");
            return;
        }
    };
    for led_index in 0..count {
        let info = dev
            .transport
            .request(dev.dev_index, idx, 0x01, &[led_index], false, 800);
        println!(
            "  0x1300 f1 led{led_index} GetLedInfo → {}",
            match info {
                Ok(resp) => hex(&resp),
                Err(e) => format!("✗ {e}"),
            }
        );
        let state = dev
            .transport
            .request(dev.dev_index, idx, 0x04, &[led_index], true, 800);
        println!(
            "  0x1300 f4 led{led_index} GetLedState → {}",
            match state {
                Ok(resp) => hex(&resp),
                Err(e) => format!("✗ {e}"),
            }
        );
    }
    let sw_ctrl = dev
        .transport
        .request(dev.dev_index, idx, 0x02, &[], false, 800);
    println!(
        "  0x1300 f2 GetSWCtrl → {}",
        match sw_ctrl {
            Ok(resp) => hex(&resp),
            Err(e) => format!("✗ {e}"),
        }
    );
}

/// 0x8070 分区/效果能力深探测(只读):f1 逐分区、f2 逐效果。
fn led_deep_probe(dev: &G502Device) {
    let Ok(idx) = dev.feature(0x8070) else {
        return;
    };
    let zones = dev
        .transport
        .request(dev.dev_index, idx, 0x00, &[], false, 800)
        .ok()
        .and_then(|r| r.get(4).copied())
        .unwrap_or(1)
        .max(1);
    for zone in 0..zones {
        let r = dev
            .transport
            .request(dev.dev_index, idx, 0x01, &[zone, 0x00, 0x00], true, 800);
        println!(
            "  f1 zone{zone} GetZoneInfo → {}",
            match r {
                Ok(resp) => hex(&resp),
                Err(e) => format!("✗ {e}"),
            }
        );
        // f2 按槽位枚举(第 2 参数是槽位下标,非效果 id):0=off 1=solid 2=cycle 3=breathing
        for slot in 0u8..4 {
            let r = dev
                .transport
                .request(dev.dev_index, idx, 0x02, &[zone, slot], true, 800);
            let eid = match &r {
                Ok(resp) if resp.len() >= 8 => ((resp[6] as u16) << 8) | resp[7] as u16,
                _ => 0,
            };
            println!(
                "  f2 zone{zone} slot{slot} → effect {eid:#06x} ({}) | {}",
                crate::features::led::effect_name(eid),
                match r {
                    Ok(resp) => hex(&resp),
                    Err(e) => format!("✗ {e}"),
                }
            );
        }
        let r = dev
            .transport
            .request(dev.dev_index, idx, 0x0E, &[zone, 0x00, 0x00], true, 800);
        println!(
            "  f14 zone{zone} GetZoneEffect → {}",
            match r {
                Ok(resp) => hex(&resp),
                Err(e) => format!("✗ {e}"),
            }
        );
    }
}

pub fn run(what: &str) -> Result<()> {
    let dev = G502Device::open()?;
    println!("设备: {}", crate::device::describe(&dev));
    if what == "all" || what == "features" {
        println!("== feature 列表 ==");
        for fid in dev.list_features()? {
            println!(
                "  {fid:#06x}  index={:#04x}  {}",
                dev.feature(fid)?,
                known_name(fid)
            );
        }
    }
    if what == "all" || what == "battery" {
        raw_probe(&dev, 0x1001);
        raw_probe(&dev, 0x1004);
        raw_probe(&dev, 0x1000);
    }
    if what == "all" || what == "dpi" {
        raw_probe(&dev, 0x2201);
    }
    if what == "all" || what == "led" {
        raw_probe(&dev, 0x8070);
        led_deep_probe(&dev);
        raw_probe(&dev, 0x1300);
        indicator_led_probe(&dev);
        // 未知 feature 中可能与灯光管线相关的,一并探测
        raw_probe(&dev, 0x1EB0);
        raw_probe(&dev, 0x1863);
        raw_probe(&dev, 0x1E22);
        raw_probe(&dev, 0x2121);
        raw_probe(&dev, 0x1DF3);
    }
    if what == "all" || what == "onboard" {
        raw_probe(&dev, 0x8100);
    }
    Ok(())
}
