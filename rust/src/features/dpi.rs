//! DPI:feature 0x2201 (Adjustable DPI)。
//! 实测函数编号: f0 GetSensorCount  f1 GetSensorDPIList  f2 GetSensorDPI  f3 SetSensorDPI
//! DPI 列表编码: uint16 BE 序列,0x0000 结尾;高 3 位为 0b111 的条目是范围描述
//! (低 13 位=步长,后跟一个 uint16 终点)。

use crate::device::G502Device;
use crate::hidpp::HidppError;

const F_ADJUSTABLE_DPI: u16 = 0x2201;

pub struct Dpi<'a> {
    dev: &'a G502Device,
    index: u8,
    cache: std::cell::RefCell<Option<Vec<u16>>>,
}

impl<'a> Dpi<'a> {
    pub fn new(dev: &'a G502Device) -> Result<Self, HidppError> {
        let index = dev.feature(F_ADJUSTABLE_DPI)?;
        Ok(Dpi {
            dev,
            index,
            cache: std::cell::RefCell::new(None),
        })
    }

    pub fn sensor_count(&self) -> u8 {
        self.dev
            .request(self.index, 0x00, &[])
            .map(|r| r[4])
            .unwrap_or(1)
            .max(1)
    }

    /// 获取支持的 DPI 档位(展开范围条目),按需分页。
    pub fn dpi_list(&self) -> Result<Vec<u16>, HidppError> {
        if let Some(v) = &*self.cache.borrow() {
            return Ok(v.clone());
        }
        let mut raw: Vec<u8> = Vec::new();
        for _ in 0..0x100 {
            let resp = self
                .dev
                .request(self.index, 0x01, &[0x00, 0x00, (raw.len() / 2) as u8])?;
            raw.extend_from_slice(&resp[5..]); // 跳过 sensor 回显字节
            if raw.len() >= 2 && raw[raw.len() - 2..] == [0x00, 0x00] {
                break;
            }
        }
        let mut vals: Vec<u16> = Vec::new();
        let mut i = 0usize;
        while i + 1 < raw.len() {
            let val = ((raw[i] as u16) << 8) | raw[i + 1] as u16;
            if val == 0 {
                break;
            }
            if val >> 13 == 0b111 {
                // 范围条目: 13 位步长 + 终点
                let step = val & 0x1FFF;
                if i + 3 >= raw.len() {
                    break;
                }
                let end = ((raw[i + 2] as u16) << 8) | raw[i + 3] as u16;
                if let Some(&last) = vals.last() {
                    if step > 0 {
                        let mut v = last + step;
                        while v <= end && vals.len() < 4096 {
                            vals.push(v);
                            v += step;
                        }
                    }
                }
                i += 4;
            } else {
                vals.push(val);
                i += 2;
            }
        }
        if vals.is_empty() {
            vals.push(800);
        }
        *self.cache.borrow_mut() = Some(vals.clone());
        Ok(vals)
    }

    pub fn get_dpi(&self) -> Result<u16, HidppError> {
        let resp = self.dev.request(self.index, 0x02, &[0x00, 0x00, 0x00])?;
        let mut dpi = ((resp[5] as u16) << 8) | resp[6] as u16;
        if dpi == 0 {
            // 为 0 时取默认 DPI
            dpi = ((resp[7] as u16) << 8) | resp[8] as u16;
        }
        if !(50..=32000).contains(&dpi) {
            return Err(HidppError::Invalid(format!("DPI 读取异常: {dpi}")));
        }
        Ok(dpi)
    }

    /// 设置 DPI，并读回传感器运行值确认。0x2201 f3 是易失写入，
    /// 睡眠/重连后需要由上层恢复期望值。
    pub fn set_dpi(&self, dpi: u16) -> Result<u16, HidppError> {
        if !(50..=32000).contains(&dpi) {
            return Err(HidppError::Invalid(format!("DPI 超出合理范围: {dpi}")));
        }
        let resp = self
            .dev
            .request(self.index, 0x03, &[0x00, (dpi >> 8) as u8, dpi as u8])?;
        let echoed = ((resp[5] as u16) << 8) | resp[6] as u16;
        if echoed != 0 && echoed != dpi {
            return Err(HidppError::Invalid(format!(
                "DPI 写入回显不一致: 请求 {dpi}, 回显 {echoed}"
            )));
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
        let actual = self.get_dpi()?;
        if actual != dpi {
            return Err(HidppError::Invalid(format!(
                "DPI 读回不一致: 请求 {dpi}, 实际 {actual}"
            )));
        }
        Ok(actual)
    }
}
