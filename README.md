# g502hub

G502 LIGHTSPEED 的轻量管理工具,替代臃肿的 Logitech G HUB。

**Rust 实现**:单二进制 1.1MB,菜单栏常驻 ~47MB(Python/PyObjC 原型 109MB,G HUB 300MB+;Python 原型已删除)。

## 功能

| 功能 | 说明 | 权限 |
|---|---|---|
| 电量/充电状态 | 原生 macOS 单色圆环按周长表示百分比；放电时圆心留空，充电时显示闪电，离线时显示斜杠 | 无需 |
| DPI 设置 | 菜单快切 / CLI 设置(100–25600,步进 50),睡眠唤醒后自动恢复 | 无需 |
| RGB 灯效 | 主要/标志分区独立设置:固定色/呼吸/循环/关闭,支持速率与亮度;重连自动恢复 | 无需 |
| 配置档 Profile | 多组 DPI+灯效一键切换(默认无配置档,可在 config.json 自行添加) | 无需 |
| 侧键宏 | 拦截 G4/G5 等主机可见侧键,回放任意键序/文本 | 需辅助功能(授权列表里显示为 g502hub) |

连接方式自动适配:LIGHTSPEED 接收器(0xC539)与 USB 有线直连。USB 接口每 2 秒进行一次无设备通信的存在检查，拔出后快速显示离线；重新插入后自动恢复连接、Host 模式和期望 DPI。
在线时每 5 秒刷新一次电池/充电状态，确保仅供电线、不枚举有线 HID 的情况下也能及时显示闪电；接收器仍在但鼠标休眠时按 10 秒退避重试。

## 使用

```bash
# Rust CLI(首选)
rust/target/release/g502hub status                 # 设备/电量/DPI 总览
rust/target/release/g502hub battery [--json]
rust/target/release/g502hub dpi get|list
rust/target/release/g502hub dpi set 1600 [--save]
rust/target/release/g502hub led get                            # 两分区当前配置
rust/target/release/g502hub led set --zone primary --effect solid --color FF8800 --brightness 80
rust/target/release/g502hub led set --zone logo --effect breathing --color 00C8FF --rate fast
rust/target/release/g502hub led set --zone all --effect cycle --rate slow
rust/target/release/g502hub led set --off                      # 关闭两分区
rust/target/release/g502hub profile list|apply 名称
rust/target/release/g502hub macro list|test mouse3
rust/target/release/g502hub monitor --interval 120
rust/target/release/g502hub probe all              # 只读协议探测

# 菜单栏(常驻)
rust/target/release/g502hub menubar
```

## 配置

`~/Library/Application Support/g502hub/config.json`:

```json
{
  "desired_mode": "host",
  "desired_dpi": 3200,
  "dpi_levels": [400, 800, 1600, 3200],
  "led_zones": {
    "primary": {"effect": "breathing", "rgb": [255, 0, 0], "brightness": 100, "rate": "medium"},
    "logo": {"effect": "solid", "rgb": [255, 255, 255], "brightness": 100, "rate": "fast"}
  },
  "macros": {
    "mouse3": {
      "name": "复制",
      "enabled": true,
      "actions": [{"keys": "cmd+c"}, {"delay_ms": 100}, {"text": "hello"}]
    }
  },
  "battery_poll_seconds": 60,
  "low_battery_threshold": 15
}
```

### 菜单栏宏录制

不需要预先知道侧键编号：

1. 点击 `录制快捷键宏…` 或 `录制按键序列宏…`；
2. 直接按鼠标上的目标侧键，应用会识别实际的 `mouseN`；
3. 快捷键模式下按一次组合键，完全松开后自动保存；
4. 序列模式下连续操作键盘，然后从菜单点击 `结束并保存序列`；
5. 录制期间可随时从菜单点击 `取消录制`。

快捷键录制支持 Cmd/Ctrl/Option/Shift 与普通键组合。序列录制保存每个 key down/up、修饰状态和事件间隔，可回放按住、释放及连续组合。录制期间键盘事件会被拦截，不会真的触发当前前台应用；应用自身生成的回放事件带来源标记，不会被再次录入。

宏按键号(CGEvent):`mouse3` = G4/后退(button 3),`mouse4` = G5/前进(button 4)。其他能向 macOS 发出 `otherMouse` 事件的按钮会显示为 `buttonN / mouseN`。若等待侧键超时，通常表示该键被板载固件处理、没有暴露给主机；请切换 Host 模式或先把该键映射为主机按钮。

首次安装后必须在 **系统设置 → 隐私与安全性 → 辅助功能** 中允许最终签名的 `g502hub.app`;旧的 zcode/裸二进制授权不再使用。安装脚本使用稳定的 designated requirement，后续重建不会因 ad-hoc `cdhash` 变化反复丢失授权。

## 安装为开机自启

```bash
./scripts/install.sh      # 构建 Rust → 复制原生 Mach-O 到 .app → 生成 AppIcon → 签名 → LaunchAgent
launchctl bootout gui/$(id -u) ~/Library/LaunchAgents/com.g502hub.menubar.plist   # 卸载
```

## 重新构建

```bash
cd rust && cargo build --release
```

## 与 G HUB 的关系

- 两者**不能同时控制设备**:G HUB 的后台进程(lghub_agent)运行时会干扰 HID++ 通道。
- 工具检测到 G HUB 运行时会在菜单提示"退出它"(走 osascript 正常退出)。
- 不需要卸载 G HUB,退出它的后台即可;彻底卸载更干净。

## 协议实现(工程细节)

- 传输:`hidapi` 打开 vendor collection (usage_page 0xFF00),短报文 0x10/长报文 0x11
- Feature 发现:Root(0x0000) GetFeature;电池 0x1001(电压→百分比曲线);DPI 0x2201
  (f1 列表含 0b111 前缀的范围压缩条目);灯效 0x8070;板载模式 0x8100 f2
- 未匹配的响应/异步包在 `hidpp.rs` 的 `wait_response` 中过滤
- 灯效 0x8070 真机校准:两个分区各固定 4 个效果槽位(f2 按槽位枚举):
  0=off、1=solid、2=cycle(0x0003)、3=breathing(0x000A)。f3 SetZoneEffect
  的 16 字节 payload 按槽位解释——solid 为 `[zone,1,R,G,B,亮度]`(正序 RGB,
  不能带效果 id 字节);cycle/breathing 的参数区以 2 字节效果 id 开头,
  周期为毫秒大端。f14 读回恒为 0,不可用作写入确认。
- 协议探测:`g502hub probe all`(只读,不发送 SET)

## 已知限制

- G HUB 的"板载内存"配置(按键绑定/多 profile 存进鼠标)暂未实现直写(feature 0x8100),
  如需 G6-G9 重绑定需走板载写入,列为二期。
- 宏回放对安全敏感场景(登录界面、需二次确认的系统弹窗)同样生效,请自行斟酌绑定内容。
