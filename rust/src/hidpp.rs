//! HID++ 传输层:hidapi 收发 + 响应匹配。
//!
//! 报文: short 0x10 (7B) / long 0x11 (20B)
//! 布局: [report_id, dev_index, feature_index, func<<4|soft_id, params...]

use hidapi::{HidApi, HidDevice};
use std::collections::VecDeque;
use std::ffi::CString;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// 进程级唯一 HidApi。
///
/// macOS 上 hidapi C 库会把 IOHIDManager 调度到创建线程的 RunLoop;
/// 若在无 RunLoop 的线程反复创建/销毁,接收器重新枚举后会留下僵尸管理器,
/// 导致全系统对设备 open 失败。必须在**主线程**(菜单栏有 NSApp run loop)初始化一次。
static SHARED_API: OnceLock<Mutex<HidApi>> = OnceLock::new();

pub fn init_shared_api() -> Result<(), HidppError> {
    let api = HidApi::new().map_err(|e| HidppError::Open(format!("hidapi 初始化失败: {e}")))?;
    let _ = SHARED_API.set(Mutex::new(api));
    Ok(())
}

pub fn shared_api() -> &'static Mutex<HidApi> {
    SHARED_API.get_or_init(|| {
        // CLI 等未显式初始化的场景:就地初始化。菜单栏始终先在主线程调用 init_shared_api。
        Mutex::new(HidApi::new().expect("hidapi init"))
    })
}

pub const VID_LOGITECH: u16 = 0x046D;
pub const RECEIVER_PIDS: &[u16] = &[0xC539, 0xC53A, 0xC547];

pub const SHORT_REPORT: u8 = 0x10;
pub const LONG_REPORT: u8 = 0x11;
pub const SHORT_LEN: usize = 7;
pub const LONG_LEN: usize = 20;

const ERR_FEATURE: u8 = 0xFF;

#[derive(Debug)]
pub enum HidppError {
    /// 设备返回 HID++ 错误报文
    Rejected {
        code: u8,
        message: String,
    },
    Timeout(String),
    Io(String),
    Open(String),
    /// 空参数表等编程错误
    Invalid(String),
}

impl std::fmt::Display for HidppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HidppError::Rejected { code, message } => {
                write!(f, "HID++ 拒绝 (code={code:#04x}) {message}")
            }
            HidppError::Timeout(m) => write!(f, "设备未响应: {m}"),
            HidppError::Io(m) => write!(f, "IO 错误: {m}"),
            HidppError::Open(m) => write!(f, "打开失败: {m}"),
            HidppError::Invalid(m) => write!(f, "无效请求: {m}"),
        }
    }
}
impl std::error::Error for HidppError {}

pub fn error_name(code: u8) -> &'static str {
    match code {
        0x01 => "INVALID_SUB_COMMAND",
        0x02 => "INVALID_ADDRESS",
        0x03 => "INVALID_VALUE",
        0x04 => "CONNECTIVITY_ERROR",
        0x05 => "TOO_MANY_DEVICES",
        0x06 => "HIDPP14_ERROR",
        0x07 => "RESOURCE_ERROR",
        0x08 => "REQUEST_UNSUPPORTED",
        0x09 => "BUSY",
        0x0A => "UNSUPPORTED",
        0x0B => "TIMEOUT",
        _ => "UNKNOWN",
    }
}

pub fn is_transient_error(error: &HidppError) -> bool {
    match error {
        HidppError::Timeout(_) | HidppError::Io(_) => true,
        HidppError::Rejected { code, .. } => matches!(*code, 0x04 | 0x07 | 0x09 | 0x0B),
        HidppError::Open(_) | HidppError::Invalid(_) => false,
    }
}

/// 一个 HID++ vendor 接口的封装(线程安全)。
pub struct Transport {
    dev: Mutex<HidDevice>,
}

pub struct InterfaceInfo {
    pub path: CString,
    pub pid: u16,
    #[allow(dead_code)]
    pub usage_page: u16,
    pub product: String,
}

/// 枚举罗技 HID++ vendor 接口(usage_page 0xFF00)。
pub fn enumerate_vendor_interfaces(api: &HidApi) -> Vec<InterfaceInfo> {
    let mut out = Vec::new();
    for d in api.device_list() {
        if d.vendor_id() != VID_LOGITECH {
            continue;
        }
        // macOS: HID++ 通道是 vendor-defined collection
        if d.usage_page() == 0xFF00 {
            out.push(InterfaceInfo {
                path: d.path().to_owned(),
                pid: d.product_id(),
                usage_page: d.usage_page(),
                product: d.product_string().unwrap_or("Logitech").to_string(),
            });
        }
    }
    out
}

impl Transport {
    pub fn open(api: &HidApi, info: &InterfaceInfo) -> Result<Transport, HidppError> {
        let dev = api.open_path(info.path.as_c_str()).map_err(|e| {
            HidppError::Open(format!(
                "{} ({}): {e}",
                info.product,
                info.path.to_string_lossy()
            ))
        })?;
        Ok(Transport {
            dev: Mutex::new(dev),
        })
    }

    /// 发送 HID++ 2.0 请求并等待匹配响应(含 report_id 的完整报文)。
    pub fn request(
        &self,
        dev_index: u8,
        feature_index: u8,
        function: u8,
        params: &[u8],
        long_request: bool,
        timeout_ms: u64,
    ) -> Result<Vec<u8>, HidppError> {
        if function > 0x0F {
            return Err(HidppError::Invalid(format!(
                "function 编号必须 0-15: {function:#x}"
            )));
        }
        let (report_id, total) = if long_request || params.len() > 3 {
            (LONG_REPORT, LONG_LEN)
        } else {
            (SHORT_REPORT, SHORT_LEN)
        };
        if !long_request && params.len() > 3 {
            return Err(HidppError::Invalid("short report 最多 3 字节参数".into()));
        }
        let soft_id = 5u8;
        let mut buf = vec![0u8; total];
        buf[0] = report_id;
        buf[1] = dev_index;
        buf[2] = feature_index;
        buf[3] = (function << 4) | soft_id;
        buf[4..4 + params.len()].copy_from_slice(params);

        let dev = self.dev.lock().unwrap();
        dev.write(&buf)
            .map_err(|e| HidppError::Io(format!("写入失败: {e} (设备可能被独占)")))?;
        Self::wait_response(
            &dev,
            dev_index,
            feature_index,
            function,
            soft_id,
            timeout_ms,
        )
    }

    fn wait_response(
        dev: &HidDevice,
        dev_index: u8,
        feature_index: u8,
        function: u8,
        soft_id: u8,
        timeout_ms: u64,
    ) -> Result<Vec<u8>, HidppError> {
        let deadline = Instant::now() + Duration::from_millis(timeout_ms);
        let expect = ((function & 0x0F) << 4) | (soft_id & 0x0F);
        let mut pending: VecDeque<Vec<u8>> = VecDeque::new();
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(HidppError::Timeout(format!(
                    "feature={feature_index:#04x} func={function}"
                )));
            }
            let mut buf = [0u8; LONG_LEN];
            let n = dev
                .read_timeout(&mut buf, remaining.as_millis().min(200) as i32)
                .map_err(|e| HidppError::Io(format!("读取失败: {e}")))?;
            if n == 0 {
                continue;
            }
            let data = buf[..n].to_vec();
            if data[0] != SHORT_REPORT && data[0] != LONG_REPORT {
                continue; // 普通鼠标报文
            }
            if data.len() < 4 {
                continue;
            }
            let (r_dev, r_feat, r_sub) = (data[1], data[2], data[3]);
            // HID++ 2.0 错误报文:
            // [report, device, 0xFF, failed_feature, failed_function|soft_id, error_code]
            if r_feat == ERR_FEATURE
                && r_dev == dev_index
                && r_sub == feature_index
                && data.get(4).copied() == Some(expect)
            {
                let code = data.get(5).copied().unwrap_or(0);
                return Err(HidppError::Rejected {
                    code,
                    message: error_name(code).to_string(),
                });
            }
            if r_dev == dev_index && r_feat == feature_index && r_sub == expect {
                return Ok(data);
            }
            // 异步通知先缓存(当前忽略)
            if pending.len() < 16 {
                pending.push_back(data);
            }
        }
    }
}
