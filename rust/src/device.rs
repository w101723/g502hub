//! G502 设备探测:接收器 slot 扫描 + 有线直连 + feature 管理。
//!
//! 在线判定:Root.GetFeature(0x0001) —— 所有 HID++ 2.0 设备必有 FeatureSet。
//! 鼠标深度休眠时接收器返回错误或超时,多轮重试等唤醒。

use crate::hidpp::{
    enumerate_vendor_interfaces, HidppError, InterfaceInfo, Transport, RECEIVER_PIDS,
};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

pub const DEV_INDEX_WIRED: u8 = 0xFF;
const PING_PARAMS: &[u8] = &[0x00, 0x01, 0x00]; // Root.GetFeature(0x0001)
pub const FEATURE_SET: u16 = 0x0001;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Mode {
    Wired,
    Receiver,
}

impl Mode {
    pub fn name(&self) -> &'static str {
        match self {
            Mode::Wired => "USB 有线",
            Mode::Receiver => "接收器",
        }
    }
}

pub struct G502Device {
    pub transport: Transport,
    pub mode: Mode,
    pub dev_index: u8,
    pub pid: u16,
    pub slot: u8,
    features: Mutex<HashMap<u16, u8>>,
}

static CONN: std::sync::Mutex<Option<std::sync::Arc<G502Device>>> = std::sync::Mutex::new(None);

/// 获取(或建立)持久设备连接,跨线程复用,避免每次操作重新枚举。
/// retries>0 时鼠标休眠会重试等待唤醒。
pub fn invalidate_connection() {
    if let Ok(mut conn) = CONN.lock() {
        *conn = None;
    }
}

pub fn get_conn(retries: u32) -> Result<std::sync::Arc<G502Device>, HidppError> {
    let existing = CONN.lock().unwrap().clone();
    if let Some(dev) = existing {
        // 廉价校验:300ms 内 ping 得到即复用
        match dev.request(0x00, 0x00, PING_PARAMS) {
            Ok(resp) if resp.len() >= 5 && resp[4] != 0 => return Ok(dev),
            // Root.GetFeature(0x0001) 对在线 HID++ 2.0 设备必须成功；
            // 接收器在鼠标休眠/离线时会返回 Rejected，旧连接不能继续复用。
            Err(HidppError::Rejected { .. }) => {
                *CONN.lock().unwrap() = None;
            }
            _ => {
                *CONN.lock().unwrap() = None;
            } // 掉线/休眠,重开
        }
    }
    let mut last = HidppError::Open("未尝试".into());
    for i in 0..=retries {
        if i > 0 {
            std::thread::sleep(Duration::from_millis(800));
        }
        match G502Device::open() {
            Ok(d) => {
                let d = std::sync::Arc::new(d);
                *CONN.lock().unwrap() = Some(d.clone());
                return Ok(d);
            }
            Err(e) => last = e,
        }
    }
    Err(last)
}

fn refreshed_interface_pids() -> Result<Vec<u16>, HidppError> {
    let mut api = crate::hidpp::shared_api()
        .lock()
        .map_err(|_| HidppError::Open("HidApi 设备列表锁已损坏".into()))?;
    api.refresh_devices()
        .map_err(|e| HidppError::Open(format!("刷新 USB HID 设备失败: {e}")))?;
    Ok(enumerate_vendor_interfaces(&api)
        .into_iter()
        .map(|d| d.pid)
        .collect())
}

/// 刷新 HID 列表并判断任一 G502 有线接口或 LIGHTSPEED 接收器是否物理存在。
pub fn g502_interface_present() -> Result<bool, HidppError> {
    Ok(refreshed_interface_pids()?
        .iter()
        .any(|pid| *pid == 0xC08D || RECEIVER_PIDS.contains(pid)))
}

/// 判断当前 USB 拓扑是否仍适合复用连接；有线接口出现时优先切换到有线。
pub fn connection_interface_unchanged(dev: &G502Device) -> Result<bool, HidppError> {
    let pids = refreshed_interface_pids()?;
    let current_present = pids.contains(&dev.pid);
    let wired_preempts_receiver = dev.mode == Mode::Receiver && pids.contains(&0xC08D);
    Ok(current_present && !wired_preempts_receiver)
}

pub fn ghub_agent_running() -> bool {
    std::process::Command::new("pgrep")
        .arg("-x")
        .arg("lghub_agent")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

impl G502Device {
    /// 自动探测并打开 G502。鼠标休眠时自动多轮重试。
    pub fn open() -> Result<G502Device, HidppError> {
        let mut api = crate::hidpp::shared_api()
            .lock()
            .map_err(|_| HidppError::Open("HidApi 设备列表锁已损坏".into()))?;
        // hidapi 的 device_list 是缓存；USB 拔插后 path 会改变，打开前必须刷新。
        api.refresh_devices()
            .map_err(|e| HidppError::Open(format!("刷新 USB HID 设备失败: {e}")))?;
        let interfaces = enumerate_vendor_interfaces(&api);
        if interfaces.is_empty() {
            return Err(HidppError::Open(
                "未发现罗技 HID++ vendor 接口:请确认 G502/接收器已插入 USB 口".into(),
            ));
        }
        let receivers: Vec<&InterfaceInfo> = interfaces
            .iter()
            .filter(|d| RECEIVER_PIDS.contains(&d.pid))
            .collect();
        let wired: Vec<&InterfaceInfo> = interfaces
            .iter()
            .filter(|d| !RECEIVER_PIDS.contains(&d.pid))
            .collect();

        let mut notes: Vec<String> = Vec::new();
        for attempt in 0..4 {
            notes.clear();
            // 插线时优先使用有线接口，才能及时读取充电状态并避免继续轮询接收器。
            for info in wired.iter().chain(receivers.iter()) {
                let is_receiver = RECEIVER_PIDS.contains(&info.pid);
                let targets: Vec<u8> = if is_receiver {
                    (1..=5).collect()
                } else {
                    vec![DEV_INDEX_WIRED]
                };
                let t = match Transport::open(&api, info) {
                    Ok(t) => t,
                    Err(e) => {
                        notes.push(format!("打开失败: {e}"));
                        continue;
                    }
                };
                for idx in targets {
                    let (online, note) = probe_index(&t, idx);
                    if online {
                        return Ok(G502Device {
                            transport: t,
                            mode: if is_receiver {
                                Mode::Receiver
                            } else {
                                Mode::Wired
                            },
                            dev_index: idx,
                            pid: info.pid,
                            slot: if is_receiver { idx } else { 0 },
                            features: Mutex::new(HashMap::new()),
                        });
                    }
                    notes.push(note);
                }
                // Transport 在下一轮重新打开(与 python 版一致,短开短关)
            }
            if attempt < 3 {
                std::thread::sleep(Duration::from_millis(800));
            }
        }

        let mut msg = "未找到可应答的 G502(请动一下鼠标唤醒它)。\n  - ".to_string();
        msg.push_str(&notes.join("\n  - "));
        if ghub_agent_running() {
            msg.push_str("\n检测到 G HUB 正在后台运行,如持续失败请先完全退出 G HUB 再试。");
        }
        Err(HidppError::Open(msg))
    }

    pub fn request(
        &self,
        feature_index: u8,
        function: u8,
        params: &[u8],
    ) -> Result<Vec<u8>, HidppError> {
        self.transport
            .request(self.dev_index, feature_index, function, params, false, 800)
    }

    pub fn request_long(
        &self,
        feature_index: u8,
        function: u8,
        params: &[u8],
    ) -> Result<Vec<u8>, HidppError> {
        self.transport
            .request(self.dev_index, feature_index, function, params, true, 800)
    }

    /// 查询 feature id 对应的动态 index;不支持返回 Rejected(UNSUPPORTED)。
    pub fn feature(&self, feature_id: u16) -> Result<u8, HidppError> {
        if let Some(&idx) = self.features.lock().unwrap().get(&feature_id) {
            return Ok(idx);
        }
        let resp = self.request(
            0x00,
            0x00,
            &[(feature_id >> 8) as u8, feature_id as u8, 0x00],
        )?;
        let index = resp[4];
        if index == 0 {
            return Err(HidppError::Rejected {
                code: 0x0A,
                message: format!("feature {feature_id:#06x} 不支持"),
            });
        }
        self.features.lock().unwrap().insert(feature_id, index);
        Ok(index)
    }

    pub fn has_feature(&self, feature_id: u16) -> bool {
        self.feature(feature_id).is_ok()
    }

    /// 枚举设备全部 feature id(FeatureSet f1 = GetFeatureIdByIndex)。
    pub fn list_features(&self) -> Result<Vec<u16>, HidppError> {
        let fs_index = self.feature(FEATURE_SET)?;
        let count = self.request(fs_index, 0x00, &[])?[4] as usize;
        let mut ids = Vec::new();
        for index in 1..=count.max(1) {
            let Ok(resp) = self.request(fs_index, 0x01, &[index as u8, 0x00, 0x00]) else {
                break;
            };
            let fid = ((resp[4] as u16) << 8) | resp[5] as u16;
            if fid != 0 {
                ids.push(fid);
            }
        }
        Ok(ids)
    }
}

/// 探测一个设备索引是否在线。
fn probe_index(t: &Transport, dev_index: u8) -> (bool, String) {
    match t.request(dev_index, 0x00, 0x00, PING_PARAMS, false, 400) {
        Ok(resp) => {
            let online = resp.len() >= 5 && resp[4] != 0;
            (
                online,
                format!("idx{dev_index:#04x}: 应答异常 {}", hex(&resp)),
            )
        }
        Err(HidppError::Rejected { code, message }) => (
            false,
            format!(
                "idx{dev_index:#04x}: 已配对但离线 ({} {message})",
                crate::hidpp::error_name(code)
            ),
        ),
        Err(HidppError::Timeout(_)) => {
            (false, format!("idx{dev_index:#04x}: 无响应(未配对或休眠)"))
        }
        Err(e) => (false, format!("idx{dev_index:#04x}: {e}")),
    }
}

fn hex(data: &[u8]) -> String {
    data.iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn describe(dev: &G502Device) -> String {
    let protocol = match dev.request(0x00, 0x01, &[]) {
        Ok(resp) if resp.len() >= 6 => format!("HID++ {}.{}", resp[4], resp[5]),
        _ => "HID++ ?".to_string(),
    };
    let slot = if dev.mode == Mode::Receiver {
        format!(" slot{}", dev.slot)
    } else {
        String::new()
    };
    format!(
        "G502 ({}{slot}, {:#06x}, {protocol})",
        dev.mode.name(),
        dev.pid
    )
}
