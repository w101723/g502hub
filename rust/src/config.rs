//! 配置:~/Library/Application Support/g502hub/config.json(与 Python 版共用)

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

static CONFIG_OP: Mutex<()> = Mutex::new(());

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LedSpec {
    #[serde(default)]
    pub off: bool,
    #[serde(default = "default_effect")]
    pub effect: String,
    #[serde(default = "default_rgb")]
    pub rgb: [u8; 3],
    #[serde(default = "default_brightness")]
    pub brightness: u8,
    /// 灯效速率:slow/medium/fast;None 表示使用设备默认节奏。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate: Option<String>,
}

impl LedSpec {
    pub fn new(effect: &str, rgb: [u8; 3], brightness: u8, rate: Option<&str>) -> Self {
        LedSpec {
            off: effect == "off",
            effect: effect.into(),
            rgb,
            brightness,
            rate: rate.map(|r| r.into()),
        }
    }

    pub fn turn_off(&mut self) {
        self.off = true;
    }

    pub fn turn_on(&mut self) {
        let was_off = self.off;
        self.off = false;
        if was_off && self.brightness == 0 {
            self.brightness = 100;
        }
    }
}

fn default_effect() -> String {
    "solid".into()
}
fn default_rgb() -> [u8; 3] {
    [255, 255, 255]
}
fn default_brightness() -> u8 {
    100
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DesiredMode {
    Host,
    Onboard,
}

impl Default for DesiredMode {
    fn default() -> Self {
        DesiredMode::Host
    }
}

fn default_desired_mode() -> DesiredMode {
    DesiredMode::Host
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Profile {
    #[serde(default)]
    pub dpi_levels: Vec<u16>,
    #[serde(default)]
    pub active_dpi: Option<u16>,
    #[serde(default)]
    pub led: Option<LedSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Action {
    #[serde(default)]
    pub keys: Option<String>,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub keycode: Option<u32>,
    #[serde(default)]
    pub delay_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_down: Option<bool>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub modifiers: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MacroBinding {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub actions: Vec<Action>,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "default_desired_mode")]
    pub desired_mode: DesiredMode,
    #[serde(default)]
    pub desired_dpi: Option<u16>,
    #[serde(default = "default_dpi_levels")]
    pub dpi_levels: Vec<u16>,
    #[serde(default)]
    pub profiles: BTreeMap<String, Profile>,
    #[serde(default)]
    pub macros: BTreeMap<String, MacroBinding>,
    /// 分区灯效配置:键 primary(主要) / logo(标志)。
    #[serde(default)]
    pub led_zones: BTreeMap<String, LedSpec>,
    #[serde(default = "default_poll")]
    pub battery_poll_seconds: u64,
    #[serde(default = "default_threshold")]
    pub low_battery_threshold: u8,
}

fn default_dpi_levels() -> Vec<u16> {
    vec![400, 800, 1600, 3200]
}
fn default_poll() -> u64 {
    60
}
fn default_threshold() -> u8 {
    15
}

impl Default for Config {
    fn default() -> Self {
        let mut macros = BTreeMap::new();
        macros.insert(
            "mouse3".to_string(),
            MacroBinding {
                name: Some("示例:复制".into()),
                enabled: false,
                actions: vec![Action {
                    keys: Some("cmd+c".into()),
                    text: None,
                    keycode: None,
                    delay_ms: None,
                    key_down: None,
                    modifiers: Vec::new(),
                }],
            },
        );
        let profiles = BTreeMap::new();
        Config {
            desired_mode: DesiredMode::Host,
            desired_dpi: None,
            dpi_levels: default_dpi_levels(),
            profiles,
            macros,
            led_zones: BTreeMap::new(),
            battery_poll_seconds: default_poll(),
            low_battery_threshold: default_threshold(),
        }
    }
}

pub fn config_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    PathBuf::from(home)
        .join("Library/Application Support/g502hub")
        .join("config.json")
}

fn load_unlocked() -> Result<Config> {
    let path = config_path();
    if !path.exists() {
        let cfg = Config::default();
        save_unlocked(&cfg)?;
        return Ok(cfg);
    }
    let text =
        std::fs::read_to_string(&path).with_context(|| format!("读取 {}", path.display()))?;
    let cfg: Config =
        serde_json::from_str(&text).with_context(|| format!("解析 {}", path.display()))?;
    Ok(cfg)
}

fn save_unlocked(cfg: &Config) -> Result<()> {
    let path = config_path();
    let parent = path.parent().unwrap();
    std::fs::create_dir_all(parent)?;
    let temp = parent.join(format!(".config.json.{}.tmp", std::process::id()));
    let mut file =
        std::fs::File::create(&temp).with_context(|| format!("创建临时配置 {}", temp.display()))?;
    file.write_all(serde_json::to_string_pretty(cfg)?.as_bytes())?;
    file.sync_all()?;
    std::fs::rename(&temp, &path).with_context(|| format!("替换配置 {}", path.display()))?;
    Ok(())
}

pub fn load() -> Result<Config> {
    let _guard = CONFIG_OP
        .lock()
        .map_err(|_| anyhow::anyhow!("配置锁已损坏"))?;
    load_unlocked()
}

pub fn save(cfg: &Config) -> Result<()> {
    let _guard = CONFIG_OP
        .lock()
        .map_err(|_| anyhow::anyhow!("配置锁已损坏"))?;
    save_unlocked(cfg)
}

/// 在同一把进程内配置锁下完成读取、修改和原子保存，避免线程间覆盖。
pub fn update<T>(f: impl FnOnce(&mut Config) -> T) -> Result<T> {
    let _guard = CONFIG_OP
        .lock()
        .map_err(|_| anyhow::anyhow!("配置锁已损坏"))?;
    let mut cfg = load_unlocked()?;
    let result = f(&mut cfg);
    save_unlocked(&cfg)?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_backward_compatibility() {
        // 模拟旧版无 desired_mode / desired_dpi / active_dpi 的 JSON
        let old_json = r#"{
            "dpi_levels": [400, 800, 1600],
            "profiles": {
                "test": {
                    "dpi_levels": [800]
                }
            }
        }"#;

        let cfg: Config = serde_json::from_str(old_json).expect("反序列化旧版配置失败");
        assert_eq!(cfg.desired_mode, DesiredMode::Host);
        assert_eq!(cfg.desired_dpi, None);
        assert_eq!(cfg.profiles.get("test").unwrap().active_dpi, None);

        let old_action: Action = serde_json::from_str(r#"{"keys":"cmd+c"}"#).unwrap();
        assert_eq!(old_action.key_down, None);
        assert!(old_action.modifiers.is_empty());

        // 旧版无 led_zones / LedSpec.rate 字段
        let old_json = r#"{
            "profiles": {"办公": {"dpi_levels": [800]}},
            "led_zones": {"primary": {"effect": "solid", "rgb": [255,0,0], "brightness": 80}}
        }"#;
        let cfg: Config = serde_json::from_str(old_json).unwrap();
        let spec = cfg.led_zones.get("primary").unwrap();
        assert_eq!(spec.rate, None);
        assert_eq!(spec.brightness, 80);
    }

    #[test]
    fn test_default_has_no_profiles() {
        assert!(Config::default().profiles.is_empty());
    }

    #[test]
    fn test_led_off_preserves_settings_and_turn_on_repairs_legacy_zero() {
        let mut spec = LedSpec::new("breathing", [10, 20, 30], 75, Some("5000"));
        spec.turn_off();
        assert!(spec.off);
        assert_eq!(spec.effect, "breathing");
        assert_eq!(spec.rgb, [10, 20, 30]);
        assert_eq!(spec.brightness, 75);
        assert_eq!(spec.rate.as_deref(), Some("5000"));

        spec.turn_on();
        assert!(!spec.off);
        assert_eq!(spec.brightness, 75);

        spec.brightness = 0;
        spec.turn_off();
        spec.turn_on();
        assert_eq!(spec.brightness, 100);
    }

    #[test]
    fn test_led_zones_round_trip() {
        let mut cfg = Config::default();
        cfg.led_zones.insert(
            "primary".into(),
            LedSpec::new("breathing", [0, 200, 255], 75, Some("slow")),
        );
        cfg.led_zones
            .insert("logo".into(), LedSpec::new("off", [0, 0, 0], 0, None));
        let serialized = serde_json::to_string_pretty(&cfg).unwrap();
        let decoded: Config = serde_json::from_str(&serialized).unwrap();
        let primary = decoded.led_zones.get("primary").unwrap();
        assert_eq!(primary.effect, "breathing");
        assert_eq!(primary.rate.as_deref(), Some("slow"));
        assert_eq!(decoded.led_zones.get("logo").unwrap().off, true);
    }

    #[test]
    fn test_recorded_action_round_trip() {
        let action = Action {
            keys: None,
            text: None,
            keycode: Some(8),
            delay_ms: None,
            key_down: Some(true),
            modifiers: vec!["cmd".into(), "shift".into()],
        };
        let serialized = serde_json::to_string(&action).unwrap();
        let decoded: Action = serde_json::from_str(&serialized).unwrap();
        assert_eq!(decoded.keycode, Some(8));
        assert_eq!(decoded.key_down, Some(true));
        assert_eq!(decoded.modifiers, vec!["cmd", "shift"]);
    }

    #[test]
    fn test_round_trip() {
        let mut cfg = Config::default();
        cfg.desired_mode = DesiredMode::Onboard;
        cfg.desired_dpi = Some(2400);
        let serialized = serde_json::to_string(&cfg).unwrap();
        let deserialized: Config = serde_json::from_str(&serialized).unwrap();
        assert_eq!(deserialized.desired_mode, DesiredMode::Onboard);
        assert_eq!(deserialized.desired_dpi, Some(2400));
    }
}
