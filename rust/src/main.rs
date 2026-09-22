mod config;
mod controller;
mod device;
mod features;
mod hidpp;
mod macro_engine;
mod menubar;
mod probe;

use anyhow::{bail, Result};
use clap::{Parser, Subcommand};
use config::Config;
use device::{ghub_agent_running, G502Device};
use features::battery::read_battery;
use features::dpi::Dpi;
use features::led::Led;
use std::time::Duration;

/// G502 LIGHTSPEED 轻量管理工具(Rust 版)
#[derive(Parser)]
#[command(name = "g502hub", version, about)]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// 设备/电量/DPI 总览
    Status,
    /// 电量与充电状态
    Battery {
        /// JSON 输出
        #[arg(long)]
        json: bool,
    },
    /// DPI 查询与设置
    Dpi {
        #[arg(default_value = "get")]
        action: String, // get | list | set
        value: Option<u16>,
        /// 把该 DPI 加入配置档位列表
        #[arg(long)]
        save: bool,
    },
    /// RGB 灯效
    Led {
        #[arg(default_value = "get")]
        action: String, // get | set
        /// 分区:primary(主要) / logo(标志) / all
        #[arg(long, default_value = "all")]
        zone: String,
        /// 关闭灯效
        #[arg(long)]
        off: bool,
        /// 颜色 RRGGBB
        #[arg(long)]
        color: Option<String>,
        #[arg(long, default_value = "solid")]
        effect: String,
        #[arg(long, default_value_t = 100)]
        brightness: u8,
        /// 速率:slow/medium/fast 或毫秒数
        #[arg(long)]
        rate: Option<String>,
    },
    /// 配置档
    Profile {
        #[arg(default_value = "list")]
        action: String, // list | apply
        name: Option<String>,
    },
    /// 侧键宏
    Macro {
        #[arg(default_value = "list")]
        action: String, // list | test | run
        name: Option<String>,
    },
    /// 低电量监控
    Monitor {
        #[arg(long, default_value_t = 120)]
        interval: u64,
    },
    /// 板载/主机模式
    Onboard {
        /// get | host | onboard
        #[arg(default_value = "get")]
        action: String,
    },
    /// 协议探测(只读)
    Probe {
        #[arg(default_value = "all")]
        what: String,
    },
    /// 菜单栏常驻应用(默认)
    Menubar,
}

fn notify(text: &str) {
    let _ = std::process::Command::new("osascript")
        .arg("-e")
        .arg(format!(
            "display notification \"{text}\" with title \"g502hub\""
        ))
        .spawn();
}

fn with_device<T>(f: impl FnOnce(&G502Device) -> Result<T>) -> Result<T> {
    let dev = device::get_conn(2)?;
    let out = f(&dev);
    out
}

fn cmd_status() -> Result<()> {
    with_device(|dev| {
        println!("设备   : {}", device::describe(dev));
        let b = read_battery(dev)?;
        let volt = b
            .voltage_mv
            .map(|v| format!(", {v} mV"))
            .unwrap_or_default();
        let bolt = if b.charging { "⚡" } else { "" };
        println!("电池   : {}% {bolt}{}{volt}", b.percent, b.state_text);
        let d = Dpi::new(dev)?;
        let levels = d.dpi_list()?;
        let cur = d.get_dpi()?;
        let desc = if levels.len() > 12 {
            let steps: std::collections::BTreeSet<u16> =
                levels.windows(2).map(|w| w[1] - w[0]).collect();
            if steps.len() == 1 {
                format!(
                    "{}~{}(步进{})",
                    levels[0],
                    levels[levels.len() - 1],
                    steps.iter().next().unwrap()
                )
            } else {
                format!("{} 档", levels.len())
            }
        } else {
            levels
                .iter()
                .map(|v| v.to_string())
                .collect::<Vec<_>>()
                .join("/")
        };
        println!("DPI    : 当前 {cur}   可选 {desc}");
        match features::onboard::get_onboard_mode(dev) {
            Some(Ok(features::onboard::OnboardMode::Onboard)) => {
                println!("模式   : 板载模式(鼠标固件自管,DPI/灯效下发会被覆盖)");
            }
            Some(Ok(features::onboard::OnboardMode::Host)) => println!("模式   : 主机控制模式"),
            Some(Err(e)) => println!("模式   : 读取失败: {e}"),
            None => println!("模式   : 设备不支持 0x8100"),
        }
        Ok(())
    })
}

fn cmd_battery(json: bool) -> Result<()> {
    with_device(|dev| {
        let b = read_battery(dev)?;
        if json {
            println!(
                "{}",
                serde_json::json!({
                    "percent": b.percent, "voltage_mv": b.voltage_mv,
                    "charging": b.charging, "state": b.state_text,
                })
            );
        } else {
            let volt = b
                .voltage_mv
                .map(|v| format!("  ({v} mV)"))
                .unwrap_or_default();
            println!("{}%  {}{volt}", b.percent, b.state_text);
        }
        Ok(())
    })
}

fn cmd_dpi(action: &str, value: Option<u16>, save: bool) -> Result<()> {
    with_device(|dev| {
        let d = Dpi::new(dev)?;
        match action {
            "list" => {
                for v in d.dpi_list()? {
                    print!("{v} ");
                }
                println!();
            }
            "get" => println!("{}", d.get_dpi()?),
            "set" => {
                let Some(v) = value else {
                    bail!("用法: g502hub dpi set <数值>")
                };
                let confirmed = controller::set_dpi_confirmed(dev, v)?;
                let mut cfg = config::load()?;
                cfg.desired_mode = config::DesiredMode::Host;
                cfg.desired_dpi = Some(confirmed);
                if save && !cfg.dpi_levels.contains(&confirmed) {
                    cfg.dpi_levels.push(confirmed);
                    cfg.dpi_levels.sort();
                }
                config::save(&cfg)?;
                println!("DPI 已写入并读回确认 → {confirmed} (已设为唤醒恢复值)");
            }
            other => bail!("未知 dpi 动作: {other} (get/list/set)"),
        }
        Ok(())
    })
}

fn cmd_led(
    action: &str,
    zone: &str,
    off: bool,
    color: Option<String>,
    effect: &str,
    brightness: u8,
    rate: Option<String>,
) -> Result<()> {
    let zones: Vec<u8> = match zone {
        "all" => vec![features::led::ZONE_PRIMARY, features::led::ZONE_LOGO],
        other => vec![features::led::zone_from_key(other)
            .ok_or_else(|| anyhow::anyhow!("未知分区: {other} (primary/logo/all)"))?],
    };
    with_device(|dev| {
        if action == "get" {
            // f14 不回显 volatile 状态(真机校准),这里报告配置的期望状态
            let cfg = config::load().unwrap_or_default();
            for z in 0..2u8 {
                let key = features::led::zone_key(z);
                let spec = cfg.led_zones.get(key).cloned().unwrap_or(config::LedSpec {
                    off: false,
                    effect: "solid".into(),
                    rgb: [255, 255, 255],
                    brightness: 100,
                    rate: Some("medium".into()),
                });
                let period = features::led::rate_period_ms(spec.rate.as_deref()).unwrap_or(0);
                println!(
                    "{}",
                    serde_json::json!({
                        "zone": key,
                        "off": spec.off,
                        "effect": spec.effect,
                        "rgb": format!("#{:02x}{:02x}{:02x}", spec.rgb[0], spec.rgb[1], spec.rgb[2]),
                        "brightness": spec.brightness,
                        "period_ms": if spec.off { 0 } else { period },
                    })
                );
            }
            return Ok(());
        }
        let parsed_rgb: Option<[u8; 3]> = match color {
            Some(c) if c.len() == 6 => Some([
                u8::from_str_radix(&c[0..2], 16)?,
                u8::from_str_radix(&c[2..4], 16)?,
                u8::from_str_radix(&c[4..6], 16)?,
            ]),
            Some(_) => bail!("颜色必须是 6 位十六进制 RRGGBB"),
            None => None,
        };
        for z in zones {
            let key = features::led::zone_key(z).to_string();
            let mut spec = config::load()?
                .led_zones
                .get(&key)
                .cloned()
                .unwrap_or_else(|| {
                    config::LedSpec::new("solid", [255, 255, 255], 100, Some("medium"))
                });
            if off {
                spec.turn_off();
            } else {
                spec.turn_on();
                spec.effect = effect.into();
                if let Some(rgb) = parsed_rgb {
                    spec.rgb = rgb;
                }
                spec.brightness = brightness;
                if let Some(value) = rate.as_deref() {
                    features::led::rate_period_ms(Some(value))?;
                    spec.rate = Some(value.into());
                }
            }
            controller::apply_led_spec(dev, z, &spec)?;
            config::update(|cfg| {
                cfg.led_zones.insert(key, spec.clone());
            })?;
            if spec.off {
                println!("{} 灯效 → 关闭", features::led::zone_label(z));
            } else {
                let period = features::led::rate_period_ms(spec.rate.as_deref())?;
                let rate_text = if period == 0 {
                    "默认".into()
                } else {
                    format!("{period}ms")
                };
                println!(
                    "{} 灯效 → {} #{:02x}{:02x}{:02x} 亮度{}% 速率{rate_text}",
                    features::led::zone_label(z),
                    spec.effect,
                    spec.rgb[0],
                    spec.rgb[1],
                    spec.rgb[2],
                    spec.brightness
                );
            }
        }
        Ok(())
    })
}

fn cmd_profile(action: &str, name: Option<String>) -> Result<()> {
    let cfg = config::load()?;
    if action == "list" {
        for (n, p) in &cfg.profiles {
            let led = p
                .led
                .as_ref()
                .map(|l| format!(", LED {}", l.effect))
                .unwrap_or_default();
            println!("{n}: DPI {:?}{led}", p.dpi_levels);
        }
        return Ok(());
    }
    let Some(name) = name else {
        bail!("用法: g502hub profile apply <名称>")
    };
    let Some(p) = cfg.profiles.get(&name) else {
        bail!(
            "未找到 Profile: {name} (可用: {})",
            cfg.profiles.keys().cloned().collect::<Vec<_>>().join(", ")
        )
    };
    with_device(|dev| {
        let target_dpi = p.active_dpi.or_else(|| p.dpi_levels.last().copied());
        controller::set_mode_confirmed(dev, features::onboard::OnboardMode::Host)?;
        let confirmed = target_dpi
            .map(|v| Dpi::new(dev).and_then(|d| d.set_dpi(v)))
            .transpose()?;
        if let Some(led) = &p.led {
            let l = Led::new(dev)?;
            if led.off {
                l.set_off(0)?;
            } else {
                let period = features::led::rate_period_ms(led.rate.as_deref())?;
                l.set_effect(0, &led.effect, led.rgb, led.brightness, period)?;
            }
        }
        let mut saved = config::load()?;
        saved.desired_mode = config::DesiredMode::Host;
        if let Some(dpi) = confirmed {
            saved.desired_dpi = Some(dpi);
        }
        config::save(&saved)?;
        println!("已应用并读回确认 Profile: {name}");
        Ok(())
    })
}

fn cmd_macro(action: &str, name: Option<String>) -> Result<()> {
    let cfg = config::load()?;
    if action == "list" {
        if cfg.macros.is_empty() {
            println!("(未配置宏,编辑 {})", config::config_path().display());
        }
        for (btn, m) in &cfg.macros {
            println!(
                "{btn}: {}  {}",
                m.name.clone().unwrap_or_default(),
                if m.enabled { "启用" } else { "停用" }
            );
        }
        return Ok(());
    }
    if action == "run" {
        if !macro_engine::accessibility_granted(true) {
            bail!("宏引擎启动失败: 需要辅助功能权限");
        }
        let tap = macro_engine::MacroTap::new(cfg.macros);
        tap.start()?;
        println!("宏拦截引擎运行中 (按绑定的侧键触发，Ctrl-C 退出)...");
        println!("is_running: {}", tap.is_running());
        let mut last_seen = None;
        loop {
            std::thread::sleep(Duration::from_millis(100));
            if let Some(btn) = tap.last_button() {
                if last_seen != Some(btn) {
                    println!("捕获按键: mouse{btn}");
                    last_seen = Some(btn);
                }
            }
        }
    }
    let Some(name) = name else {
        bail!("用法: g502hub macro test <mouse3|mouse4|…> 或 g502hub macro run")
    };
    let Some(m) = cfg.macros.get(&name) else {
        bail!("未找到宏: {name}")
    };
    macro_engine::play(&m.actions)?;
    println!("已回放宏: {}", m.name.clone().unwrap_or(name));
    Ok(())
}

fn cmd_onboard(action: &str) -> Result<()> {
    with_device(|dev| match action {
        "get" => {
            match features::onboard::get_onboard_mode(dev) {
                Some(Ok(m)) => println!("{}", m.name()),
                Some(Err(e)) => bail!("读取失败: {e}"),
                None => println!("设备不支持板载模式 feature"),
            }
            Ok(())
        }
        "host" => {
            let mode = controller::set_mode_confirmed(dev, features::onboard::OnboardMode::Host)?;
            let mut cfg = config::load()?;
            cfg.desired_mode = config::DesiredMode::Host;
            config::save(&cfg)?;
            println!("已写入并读回确认: {}", mode.name());
            Ok(())
        }
        "onboard" => {
            let mode =
                controller::set_mode_confirmed(dev, features::onboard::OnboardMode::Onboard)?;
            let mut cfg = config::load()?;
            cfg.desired_mode = config::DesiredMode::Onboard;
            config::save(&cfg)?;
            println!("已写入并读回确认: {}", mode.name());
            Ok(())
        }
        other => bail!("未知动作: {other} (get/host/onboard)"),
    })
}

fn cmd_monitor(interval: u64) -> Result<()> {
    let cfg: Config = config::load()?;
    let threshold = cfg.low_battery_threshold;
    println!("监控中: 每 {interval}s 查询一次,低于 {threshold}% 弹出系统通知 (Ctrl-C 退出)");
    let mut last_alert = std::time::Instant::now() - Duration::from_secs(3600);
    loop {
        match with_device(|dev| Ok(read_battery(dev)?)) {
            Ok(b) => {
                let ts = chrono_like_now();
                println!("[{ts}] {}% {}", b.percent, b.state_text);
                if !b.charging
                    && b.percent <= threshold
                    && last_alert.elapsed() > Duration::from_secs(1800)
                {
                    notify(&format!("G502 电量仅 {}%,请充电", b.percent));
                    last_alert = std::time::Instant::now();
                }
            }
            Err(e) => {
                let ts = chrono_like_now();
                println!("[{ts}] 设备不可用: {e}");
            }
        }
        std::thread::sleep(Duration::from_secs(interval));
    }
}

fn chrono_like_now() -> String {
    // 简易本地时间:调用系统 date,避免引依赖
    std::process::Command::new("date")
        .arg("+%H:%M:%S")
        .output()
        .ok()
        .and_then(|o| {
            String::from_utf8(o.stdout)
                .ok()
                .map(|s| s.trim().to_string())
        })
        .unwrap_or_default()
}

fn main() -> Result<()> {
    hidpp::init_shared_api()?;
    let cli = Cli::parse();
    if ghub_agent_running() {
        eprintln!("⚠️  G HUB 正在运行,如遇设备无响应请先完全退出 G HUB\n");
    }
    let r: Result<()> = match &cli.cmd {
        None | Some(Cmd::Menubar) => menubar::run(),
        Some(Cmd::Status) => cmd_status(),
        Some(Cmd::Battery { json }) => cmd_battery(*json),
        Some(Cmd::Dpi {
            action,
            value,
            save,
        }) => cmd_dpi(action, *value, *save),
        Some(Cmd::Led {
            action,
            zone,
            off,
            color,
            effect,
            brightness,
            rate,
        }) => cmd_led(
            action,
            zone,
            *off,
            color.clone(),
            effect,
            *brightness,
            rate.clone(),
        ),
        Some(Cmd::Profile { action, name }) => cmd_profile(action, name.clone()),
        Some(Cmd::Macro { action, name }) => cmd_macro(action, name.clone()),
        Some(Cmd::Monitor { interval }) => cmd_monitor(*interval),
        Some(Cmd::Onboard { action }) => cmd_onboard(action),
        Some(Cmd::Probe { what }) => probe::run(what),
    };
    if let Err(e) = r {
        eprintln!("错误: {e:#}");
        std::process::exit(1);
    }
    Ok(())
}
