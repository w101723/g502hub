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
    }
    if what == "all" || what == "onboard" {
        raw_probe(&dev, 0x8100);
    }
    Ok(())
}
