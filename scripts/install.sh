#!/bin/bash
# g502hub 安装：构建 Rust、组装并签名 .app、注册开机自启。
set -euo pipefail
cd "$(dirname "$0")/.."
PROJECT_DIR="$(pwd)"
APP="$PROJECT_DIR/g502hub.app"
PLIST_ID="com.g502hub.menubar"
PLIST_SRC="scripts/${PLIST_ID}.plist"
PLIST_DST="$HOME/Library/LaunchAgents/${PLIST_ID}.plist"
BIN="$PROJECT_DIR/rust/target/release/g502hub"

command -v cargo >/dev/null 2>&1 || { echo "错误：未找到 cargo" >&2; exit 1; }

echo "==> [1/4] 构建 Rust release"
cargo build --release --manifest-path rust/Cargo.toml

echo "==> [2/4] 组装原生 .app"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
rm -f "$APP/Contents/Resources/g502_top.png" "$APP/Contents/Resources/g502_side.png"
cp "$BIN" "$APP/Contents/MacOS/g502hub"
chmod 755 "$APP/Contents/MacOS/g502hub"
python3 scripts/generate_app_icon.py >/dev/null
ICONSET="$(mktemp -d)/AppIcon.iconset"
mkdir -p "$ICONSET"
for spec in "16:icon_16x16.png" "32:icon_16x16@2x.png" "32:icon_32x32.png" "64:icon_32x32@2x.png" "128:icon_128x128.png" "256:icon_128x128@2x.png" "256:icon_256x256.png" "512:icon_256x256@2x.png" "512:icon_512x512.png" "1024:icon_512x512@2x.png"; do
    size="${spec%%:*}"; name="${spec#*:}"
    sips -z "$size" "$size" "$APP/Contents/Resources/AppIcon-1024.png" --out "$ICONSET/$name" >/dev/null
 done
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/AppIcon.icns"
rm -rf "$(dirname "$ICONSET")"
# 未安装 Apple Developer 证书时仍使用 ad-hoc 签名，但指定要求必须跨构建稳定。
# 只用默认 ad-hoc 签名会把 cdhash 写入要求，二进制每次变化都会让 TCC 辅助功能授权失效。
codesign --force --deep --sign - --identifier "$PLIST_ID" \
    --requirements "=designated => identifier \"$PLIST_ID\"" "$APP"

echo "==> [3/4] 注册 LaunchAgent"
mkdir -p "$HOME/Library/LaunchAgents"
sed "s|@PROJECT_DIR@|$PROJECT_DIR|g" "$PLIST_SRC" > "$PLIST_DST"
launchctl bootout "gui/$(id -u)" "$PLIST_DST" 2>/dev/null || true
pkill -x g502hub 2>/dev/null || true
launchctl bootstrap "gui/$(id -u)" "$PLIST_DST"
launchctl kickstart -k "gui/$(id -u)/$PLIST_ID"

echo "==> [4/4] 完成"
echo "菜单栏应用：$APP"
echo "CLI：$BIN status"
echo "宏需要在 系统设置 → 隐私与安全性 → 辅助功能 中允许 g502hub.app"
