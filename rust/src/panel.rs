//! G502 LIGHTSPEED 原生控制中心窗口 —— macOS 系统设置风格。
//!
//! 布局参考设计稿 (G502 LIGHTSPEED · macOS 风格设置界面):
//! - 左侧 196pt 侧边栏: 设备徽标 + 分类导航 (DPI / 灯效 / 宏·文字)
//! - 右侧主区: 工具栏 (标题 + 视角切换 + 电量胶囊) → 按键示意图白卡 → 功能抽屉白卡
//! - DPI 页: 大号活动档位数值 + 5 档瓷砖按钮 + 无级滑杆 + 报告率 + 提示条
//! - 灯效页: 分区/模式分段 + 机身灯效预览 + 宝石色板 + 亮度/速率行
//! - 宏页: NSSwitch 引擎开关 + 状态圆点 + 按键绑定行列表 + 录制操作条

use crate::controller::{self, TargetZone};
use crate::features::led;
use crate::mouse_canvas::{CanvasMode, G502MouseCanvas};
use objc2::mutability::MainThreadOnly;
use objc2::rc::{Allocated, Retained};
use objc2::runtime::NSObject;
use objc2::{declare_class, msg_send, msg_send_id, sel, ClassType, DeclaredClass};
use objc2_app_kit::{
    NSAppearance, NSAppearanceCustomization, NSAppearanceNameDarkAqua, NSApplication,
    NSBackingStoreType, NSBezierPath, NSBox, NSBoxType, NSButton, NSButtonType,
    NSCellImagePosition, NSColor, NSCompositingOperation, NSControl, NSCursor, NSEvent,
    NSFloatingWindowLevel, NSFont, NSFontAttributeName, NSFontWeightBold, NSFontWeightMedium,
    NSForegroundColorAttributeName, NSGraphicsContext, NSImage, NSImageView, NSLayoutAttribute,
    NSLayoutConstraintOrientation, NSPanel, NSScreen, NSScrollView, NSSegmentStyle,
    NSSegmentSwitchTracking, NSSegmentedControl, NSSlider, NSStackView, NSStackViewDistribution,
    NSSwitch, NSTextAlignment, NSTextField, NSTrackingArea, NSTrackingAreaOptions,
    NSUserInterfaceLayoutOrientation, NSView, NSWindowButton, NSWindowCollectionBehavior,
    NSWindowStyleMask, NSWindowTitleVisibility,
};
use objc2_foundation::{
    MainThreadMarker, NSArray, NSEdgeInsets, NSNotificationCenter, NSPoint, NSRect, NSSize,
    NSString,
};
use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};

// ---------------------------------------------------------------------- //
// 通用绘制辅助 (供各自定义 NSView 使用)
// ---------------------------------------------------------------------- //

type AttrDict = Retained<NSMutableDictionary<objc2::runtime::AnyObject, objc2::runtime::AnyObject>>;
use objc2_foundation::NSMutableDictionary;

/// 构建文本属性字典 (字体 + 前景色)
unsafe fn make_text_attrs(font: &NSFont, color: &NSColor) -> AttrDict {
    let dict = NSMutableDictionary::<objc2::runtime::AnyObject, objc2::runtime::AnyObject>::new();
    let _: () = msg_send![&dict, setObject: &*font, forKey: NSFontAttributeName];
    let _: () = msg_send![&dict, setObject: &*color, forKey: NSForegroundColorAttributeName];
    dict
}

/// 在指定顶部 Y 位置左对齐绘制文本
unsafe fn draw_text_left(text: &str, x: f64, y: f64, font: &NSFont, color: &NSColor) {
    let s = NSString::from_str(text);
    let dict = make_text_attrs(font, color);
    let rect = NSRect::new(NSPoint::new(x, y), NSSize::new(400.0, 22.0));
    let _: () = msg_send![&*s, drawInRect: rect, withAttributes: &*dict];
}

/// 在指定宽度内水平居中、指定顶部 Y 绘制文本
unsafe fn draw_text_centered(text: &str, w: f64, y: f64, font: &NSFont, color: &NSColor) {
    let s = NSString::from_str(text);
    let dict = make_text_attrs(font, color);
    let size: NSSize = msg_send![&*s, sizeWithAttributes: &*dict];
    let rect = NSRect::new(
        NSPoint::new((w - size.width) * 0.5, y),
        NSSize::new(size.width + 2.0, size.height),
    );
    let _: () = msg_send![&*s, drawInRect: rect, withAttributes: &*dict];
}

/// 在指定右侧 X 结束、指定顶部 Y 绘制文本 (右对齐)
unsafe fn draw_text_right(text: &str, right_x: f64, y: f64, font: &NSFont, color: &NSColor) {
    let s = NSString::from_str(text);
    let dict = make_text_attrs(font, color);
    let size: NSSize = msg_send![&*s, sizeWithAttributes: &*dict];
    let rect = NSRect::new(
        NSPoint::new(right_x - size.width, y),
        NSSize::new(size.width + 2.0, size.height),
    );
    let _: () = msg_send![&*s, drawInRect: rect, withAttributes: &*dict];
}

/// 在指定中心坐标 (cx, cy) 处精准居中绘制文本
unsafe fn draw_text_at_center(text: &str, cx: f64, cy: f64, font: &NSFont, color: &NSColor) {
    let s = NSString::from_str(text);
    let dict = make_text_attrs(font, color);
    let size: NSSize = msg_send![&*s, sizeWithAttributes: &*dict];
    let rect = NSRect::new(
        NSPoint::new(cx - size.width * 0.5, cy - size.height * 0.5),
        NSSize::new(size.width + 2.0, size.height),
    );
    let _: () = msg_send![&*s, drawInRect: rect, withAttributes: &*dict];
}

/// 在指定左侧 X 与指定垂直中心坐标 cy 处精准垂直居中绘制文本
unsafe fn draw_text_left_centered_y(text: &str, x: f64, cy: f64, font: &NSFont, color: &NSColor) {
    let s = NSString::from_str(text);
    let dict = make_text_attrs(font, color);
    let size: NSSize = msg_send![&*s, sizeWithAttributes: &*dict];
    let rect = NSRect::new(
        NSPoint::new(x, cy - size.height * 0.5),
        NSSize::new(size.width + 2.0, size.height),
    );
    let _: () = msg_send![&*s, drawInRect: rect, withAttributes: &*dict];
}

/// 渲染一张 SF Symbol 图标 (按最长边缩放到 point_size 并着色)。
/// 返回的图像尺寸与符号纵横比一致，避免拉伸变形。
#[allow(deprecated)]
unsafe fn tinted_symbol(name: &str, point_size: f64, color: &NSColor) -> Retained<NSImage> {
    let mtm = MainThreadMarker::new().expect("tinted_symbol 需要主线程");
    let sym = NSImage::imageWithSystemSymbolName_accessibilityDescription(
        &NSString::from_str(name),
        None,
    );
    let Some(sym) = sym else {
        return NSImage::initWithSize(mtm.alloc(), NSSize::new(0.0, 0.0));
    };
    let s = sym.size();
    let base = s.width.max(s.height).max(1.0);
    let scale = point_size / base;
    let dw = (s.width * scale).max(1.0);
    let dh = (s.height * scale).max(1.0);
    let canvas = NSImage::initWithSize(mtm.alloc(), NSSize::new(dw, dh));
    canvas.lockFocus();
    let rect = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(dw, dh));
    // drawInRect 会把整张符号图缩放铺满 rect；rect 与符号纵横比一致，因此无形变。
    sym.drawInRect(rect);
    if let Some(ctx) = NSGraphicsContext::currentContext() {
        ctx.setCompositingOperation(NSCompositingOperation::SourceIn);
        color.setFill();
        NSBezierPath::bezierPathWithRect(rect).fill();
        ctx.setCompositingOperation(NSCompositingOperation::SourceOver);
    }
    canvas.unlockFocus();
    canvas
}

// ---------------------------------------------------------------------- //
// 纯格式化辅助 (可单测)
// ---------------------------------------------------------------------- //

/// 千分位格式化 DPI 数值: 3200 → "3,200"
pub fn format_dpi(dpi: u16) -> String {
    let s = dpi.to_string();
    let total = s.len();
    let mut out = String::with_capacity(total + 4);
    for (i, c) in s.chars().enumerate() {
        out.push(c);
        let rem = total - i - 1;
        if rem > 0 && rem % 3 == 0 {
            out.push(',');
        }
    }
    out
}

/// 从菜单栏状态文本解析 (电量百分比, 是否充电)。
/// 输入示例: "🔋 85%" / "⚡️ 71%" / "🔋 未连接"
pub fn battery_meta(text: &str) -> (Option<u8>, bool) {
    let charging = text.contains('⚡');
    let percent = text
        .split('%')
        .next()
        .and_then(|head| {
            head.rsplit(|c: char| !c.is_ascii_digit())
                .next()
                .filter(|digits| !digits.is_empty())
        })
        .and_then(|digits| digits.parse::<u8>().ok());
    (percent, charging)
}

/// 电量 → (SF Symbol 名称, RGB 着色)
pub fn battery_icon_spec(percent: Option<u8>, charging: bool) -> (&'static str, [f64; 3]) {
    const GREEN: [f64; 3] = [0.188, 0.820, 0.345];
    const ORANGE: [f64; 3] = [1.0, 0.584, 0.0];
    const RED: [f64; 3] = [1.0, 0.271, 0.227];
    const GRAY: [f64; 3] = [0.631, 0.631, 0.651];
    if charging {
        return ("bolt.fill", GREEN);
    }
    match percent {
        Some(p) if p >= 60 => ("battery.100", GREEN),
        Some(p) if p >= 35 => ("battery.75", GREEN),
        Some(p) if p >= 15 => ("battery.50", ORANGE),
        Some(p) if p > 0 => ("battery.25", ORANGE),
        Some(_) => ("battery.0", RED),
        None => ("battery.0", GRAY),
    }
}

/// HSV → RGB (用于灯效预览的彩色循环彩虹渐变)
pub fn hsv_to_rgb(h_deg: f64, s: f64, v: f64) -> [u8; 3] {
    let h = ((h_deg % 360.0) + 360.0) % 360.0 / 60.0;
    let c = v * s;
    let x = c * (1.0 - (h % 2.0 - 1.0).abs());
    let (r, g, b) = match h as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    [
        ((r + m) * 255.0).round() as u8,
        ((g + m) * 255.0).round() as u8,
        ((b + m) * 255.0).round() as u8,
    ]
}

// ---------------------------------------------------------------------- //
// 自定义视图 1: 侧边栏导航行 (图标 + 标题, 选中态蓝底白字)
// ---------------------------------------------------------------------- //

pub struct NavRowIvars {
    tag: Cell<usize>,
    title: RefCell<String>,
    icon_normal: RefCell<Retained<NSImage>>,
    icon_selected: RefCell<Retained<NSImage>>,
    selected: Cell<bool>,
    hovered: Cell<bool>,
    a11y_role: Retained<NSString>,
    a11y_label: RefCell<Retained<NSString>>,
    tracking_area: RefCell<Option<Retained<NSTrackingArea>>>,
}

declare_class!(
    pub struct G502NavRow;

    unsafe impl ClassType for G502NavRow {
        type Super = NSView;
        type Mutability = MainThreadOnly;
        const NAME: &'static str = "G502NavRow";
    }

    impl DeclaredClass for G502NavRow {
        type Ivars = NavRowIvars;
    }

    unsafe impl G502NavRow {
        #[method_id(initWithFrame:)]
        fn init_with_frame(this: Allocated<Self>, frame: NSRect) -> Option<Retained<Self>> {
            let mtm = MainThreadMarker::new().expect("nav row 需要主线程");
            let empty = unsafe { NSImage::initWithSize(mtm.alloc(), NSSize::new(0.0, 0.0)) };
            let this = this.set_ivars(NavRowIvars {
                tag: Cell::new(usize::MAX),
                title: RefCell::new(String::new()),
                icon_normal: RefCell::new(empty.clone()),
                icon_selected: RefCell::new(empty),
                selected: Cell::new(false),
                hovered: Cell::new(false),
                a11y_role: NSString::from_str("AXButton"),
                a11y_label: RefCell::new(NSString::from_str("导航")),
                tracking_area: RefCell::new(None),
            });
            unsafe { msg_send_id![super(this), initWithFrame: frame] }
        }

        #[method(updateTrackingAreas)]
        fn update_tracking_areas(&self) {
            let ivars = self.ivars();
            if let Some(old_area) = ivars.tracking_area.borrow_mut().take() {
                unsafe { self.removeTrackingArea(&old_area) };
            }
            let bounds = self.bounds();
            let opts = NSTrackingAreaOptions::NSTrackingMouseEnteredAndExited
                | NSTrackingAreaOptions::NSTrackingActiveAlways
                | NSTrackingAreaOptions::NSTrackingInVisibleRect;
            let alloc = NSTrackingArea::alloc();
            let area = unsafe {
                NSTrackingArea::initWithRect_options_owner_userInfo(
                    alloc,
                    bounds,
                    opts,
                    Some(self.as_ref()),
                    None,
                )
            };
            unsafe { self.addTrackingArea(&area) };
            *ivars.tracking_area.borrow_mut() = Some(area);
        }

        #[method(mouseEntered:)]
        fn mouse_entered(&self, _event: &NSEvent) {
            self.ivars().hovered.set(true);
            unsafe {
                NSCursor::pointingHandCursor().set();
                self.setNeedsDisplay(true);
            }
        }

        #[method(mouseExited:)]
        fn mouse_exited(&self, _event: &NSEvent) {
            self.ivars().hovered.set(false);
            unsafe {
                NSCursor::arrowCursor().set();
                self.setNeedsDisplay(true);
            }
        }

        #[method(mouseDown:)]
        fn mouse_down(&self, _event: &NSEvent) {
            let tag = self.ivars().tag.get();
            PopoverPanel::save_active_editor();
            PopoverPanel::switch_main_tab(tag);
        }

        #[method(drawRect:)]
        fn draw_rect(&self, _dirty_rect: NSRect) {
            let bounds = self.bounds();
            let w = bounds.size.width;
            let h = bounds.size.height;
            if w < 10.0 || h < 10.0 {
                return;
            }
            let sel = self.ivars().selected.get();
            let hov = self.ivars().hovered.get();
            unsafe {
                let cy = h * 0.5;

                if sel {
                    let path =
                        NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(bounds, 8.0, 8.0);
                    NSColor::colorWithSRGBRed_green_blue_alpha(0.047, 0.169, 0.231, 0.95).setFill();
                    path.fill();
                    NSColor::colorWithSRGBRed_green_blue_alpha(0.0, 0.808, 1.0, 0.65).setStroke();
                    path.setLineWidth(1.0);
                    path.stroke();

                    // 左侧罗技青活力指示条 (强化选中感，垂直居中)
                    let bar_h = 24.0;
                    let bar_rect = NSRect::new(NSPoint::new(3.5, cy - bar_h * 0.5), NSSize::new(3.5, bar_h));
                    let bar = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(bar_rect, 1.75, 1.75);
                    PopoverPanel::color_accent().setFill();
                    bar.fill();
                } else if hov {
                    let path =
                        NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(bounds, 8.0, 8.0);
                    NSColor::colorWithSRGBRed_green_blue_alpha(1.0, 1.0, 1.0, 0.05).setFill();
                    path.fill();
                    NSColor::colorWithSRGBRed_green_blue_alpha(1.0, 1.0, 1.0, 0.08).setStroke();
                    path.setLineWidth(1.0);
                    path.stroke();
                }

                // 统一图标列水平中心线 (icon_cx = 21.0)，确保不同长宽比图标几何中心严格对齐
                let icon_cx = 21.0;
                let icon = if sel {
                    self.ivars().icon_selected.borrow().clone()
                } else {
                    self.ivars().icon_normal.borrow().clone()
                };
                let icon_size = icon.size();
                if icon_size.width > 0.5 {
                    let icon_rect = NSRect::new(
                        NSPoint::new(icon_cx - icon_size.width * 0.5, cy - icon_size.height * 0.5),
                        NSSize::new(icon_size.width, icon_size.height),
                    );
                    icon.drawInRect(icon_rect);
                }

                let font = NSFont::systemFontOfSize_weight(13.5, NSFontWeightMedium);
                let color = if sel {
                    NSColor::colorWithSRGBRed_green_blue_alpha(0.96, 0.98, 1.0, 1.0)
                } else if hov {
                    NSColor::colorWithSRGBRed_green_blue_alpha(0.88, 0.92, 0.96, 1.0)
                } else {
                    PopoverPanel::color_primary_text()
                };
                let title = self.ivars().title.borrow().clone();
                // 文字起始 X = 38.0，垂直中心与图标中心 cy 严格对齐
                draw_text_left_centered_y(&title, 38.0, cy, &font, &color);
            }
        }

        // ---- NSAccessibility: 让导航行可作为按钮被系统辅助功能与自动化脚本触发 ----
        #[method(isAccessibilityElement)]
        fn is_accessibility_element(&self) -> bool {
            true
        }

        #[method(accessibilityRole)]
        fn accessibility_role(&self) -> Option<&NSString> {
            Some(&*self.ivars().a11y_role)
        }

        #[method(accessibilityLabel)]
        fn accessibility_label(&self) -> Option<&NSString> {
            let label = self.ivars().a11y_label.borrow();
            // Ref 借用无法作为返回值,故经裸指针延长生命周期;
            // 字符串本体存于 ivar,与对象同生命周期,调用期间始终有效。
            let ptr: *const NSString = std::ptr::from_ref(&**label);
            Some(unsafe { &*ptr })
        }

        #[method(accessibilityPerformPress)]
        fn accessibility_perform_press(&self) -> bool {
            let tag = self.ivars().tag.get();
            if tag == usize::MAX {
                return false.into();
            }
            PopoverPanel::save_active_editor();
            PopoverPanel::switch_main_tab(tag);
            true.into()
        }
    }
);

impl G502NavRow {
    pub fn new(
        frame: NSRect,
        tag: usize,
        title: &str,
        icon_normal: Retained<NSImage>,
        icon_selected: Retained<NSImage>,
    ) -> Retained<Self> {
        let mtm = MainThreadMarker::new().expect("nav row 需要主线程");
        unsafe {
            let alloc = mtm.alloc::<Self>();
            let this: Option<Retained<Self>> = msg_send_id![alloc, initWithFrame: frame];
            let this = this.expect("初始化 G502NavRow 失败");
            this.ivars().tag.set(tag);
            *this.ivars().title.borrow_mut() = title.to_string();
            *this.ivars().icon_normal.borrow_mut() = icon_normal;
            *this.ivars().icon_selected.borrow_mut() = icon_selected;
            *this.ivars().a11y_label.borrow_mut() = NSString::from_str(&format!("{title} 导航"));
            this
        }
    }

    pub fn set_selected(&self, selected: bool) {
        if self.ivars().selected.get() != selected {
            self.ivars().selected.set(selected);
            unsafe { self.setNeedsDisplay(true) };
        }
    }
}

// ---------------------------------------------------------------------- //
// 自定义视图 2: DPI 预设瓷砖按钮 (两行: 数值 + 档位说明)
// ---------------------------------------------------------------------- //

pub struct DpiTileIvars {
    dpi: Cell<u16>,
    value_text: RefCell<String>,
    sub_text: RefCell<String>,
    selected: Cell<bool>,
    hovered: Cell<bool>,
    a11y_role: Retained<NSString>,
    a11y_label: RefCell<Retained<NSString>>,
    tracking_area: RefCell<Option<Retained<NSTrackingArea>>>,
}

declare_class!(
    pub struct G502DpiTile;

    unsafe impl ClassType for G502DpiTile {
        type Super = NSView;
        type Mutability = MainThreadOnly;
        const NAME: &'static str = "G502DpiTile";
    }

    impl DeclaredClass for G502DpiTile {
        type Ivars = DpiTileIvars;
    }

    unsafe impl G502DpiTile {
        #[method_id(initWithFrame:)]
        fn init_with_frame(this: Allocated<Self>, frame: NSRect) -> Option<Retained<Self>> {
            let this = this.set_ivars(DpiTileIvars {
                dpi: Cell::new(0),
                value_text: RefCell::new(String::new()),
                sub_text: RefCell::new(String::new()),
                selected: Cell::new(false),
                hovered: Cell::new(false),
                a11y_role: NSString::from_str("AXButton"),
                a11y_label: RefCell::new(NSString::from_str("DPI 档位")),
                tracking_area: RefCell::new(None),
            });
            unsafe { msg_send_id![super(this), initWithFrame: frame] }
        }

        #[method(updateTrackingAreas)]
        fn update_tracking_areas(&self) {
            let ivars = self.ivars();
            if let Some(old_area) = ivars.tracking_area.borrow_mut().take() {
                unsafe { self.removeTrackingArea(&old_area) };
            }
            let bounds = self.bounds();
            let opts = NSTrackingAreaOptions::NSTrackingMouseEnteredAndExited
                | NSTrackingAreaOptions::NSTrackingActiveAlways
                | NSTrackingAreaOptions::NSTrackingInVisibleRect;
            let alloc = NSTrackingArea::alloc();
            let area = unsafe {
                NSTrackingArea::initWithRect_options_owner_userInfo(
                    alloc,
                    bounds,
                    opts,
                    Some(self.as_ref()),
                    None,
                )
            };
            unsafe { self.addTrackingArea(&area) };
            *ivars.tracking_area.borrow_mut() = Some(area);
        }

        #[method(mouseEntered:)]
        fn mouse_entered(&self, _event: &NSEvent) {
            self.ivars().hovered.set(true);
            unsafe {
                NSCursor::pointingHandCursor().set();
                self.setNeedsDisplay(true);
            }
        }

        #[method(mouseExited:)]
        fn mouse_exited(&self, _event: &NSEvent) {
            self.ivars().hovered.set(false);
            unsafe {
                NSCursor::arrowCursor().set();
                self.setNeedsDisplay(true);
            }
        }

        #[method(mouseDown:)]
        fn mouse_down(&self, _event: &NSEvent) {
            let dpi = self.ivars().dpi.get();
            PopoverPanel::dispatch_dpi_preset(dpi);
        }

        #[method(drawRect:)]
        fn draw_rect(&self, _dirty_rect: NSRect) {
            let bounds = self.bounds();
            let w = bounds.size.width;
            let h = bounds.size.height;
            if w < 10.0 || h < 10.0 {
                return;
            }
            let sel = self.ivars().selected.get();
            let hov = self.ivars().hovered.get();
            unsafe {
                let path = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(bounds, 10.0, 10.0);
                if sel {
                    NSColor::colorWithSRGBRed_green_blue_alpha(0.04, 0.28, 0.40, 0.90).setFill();
                } else if hov {
                    NSColor::colorWithSRGBRed_green_blue_alpha(0.18, 0.20, 0.24, 1.0).setFill();
                } else {
                    NSColor::colorWithSRGBRed_green_blue_alpha(0.10, 0.11, 0.13, 1.0).setFill();
                }
                path.fill();

                let border = if sel {
                    PopoverPanel::color_accent()
                } else if hov {
                    NSColor::colorWithSRGBRed_green_blue_alpha(0.30, 0.33, 0.38, 1.0)
                } else {
                    NSColor::colorWithSRGBRed_green_blue_alpha(0.18, 0.20, 0.23, 1.0)
                };
                border.setStroke();
                path.setLineWidth(if sel { 1.5 } else { 1.0 });
                path.stroke();

                let value_font = NSFont::monospacedDigitSystemFontOfSize_weight(14.5, NSFontWeightBold);
                let value_color = if sel {
                    NSColor::colorWithSRGBRed_green_blue_alpha(0.92, 0.98, 1.0, 1.0)
                } else {
                    PopoverPanel::color_primary_text()
                };
                let value_text = self.ivars().value_text.borrow().clone();
                draw_text_centered(&value_text, w, h * 0.48, &value_font, &value_color);

                let sub_font = NSFont::systemFontOfSize(10.5);
                let sub_color = if sel {
                    PopoverPanel::color_accent()
                } else {
                    PopoverPanel::color_secondary_text()
                };
                let sub_text = self.ivars().sub_text.borrow().clone();
                draw_text_centered(&sub_text, w, h * 0.14, &sub_font, &sub_color);
            }
        }

        // ---- NSAccessibility: 瓷砖可作为按钮触发档位切换 ----
        #[method(isAccessibilityElement)]
        fn is_accessibility_element(&self) -> bool {
            true
        }

        #[method(accessibilityRole)]
        fn accessibility_role(&self) -> Option<&NSString> {
            Some(&*self.ivars().a11y_role)
        }

        #[method(accessibilityLabel)]
        fn accessibility_label(&self) -> Option<&NSString> {
            let label = self.ivars().a11y_label.borrow();
            // Ref 借用无法作为返回值,故经裸指针延长生命周期;
            // 字符串本体存于 ivar,与对象同生命周期,调用期间始终有效。
            let ptr: *const NSString = std::ptr::from_ref(&**label);
            Some(unsafe { &*ptr })
        }

        #[method(accessibilityPerformPress)]
        fn accessibility_perform_press(&self) -> bool {
            let dpi = self.ivars().dpi.get();
            if dpi == 0 {
                return false.into();
            }
            PopoverPanel::dispatch_dpi_preset(dpi);
            true.into()
        }
    }
);

impl G502DpiTile {
    pub fn new(frame: NSRect, dpi: u16, value_text: &str, sub_text: &str) -> Retained<Self> {
        let mtm = MainThreadMarker::new().expect("dpi tile 需要主线程");
        unsafe {
            let alloc = mtm.alloc::<Self>();
            let this: Option<Retained<Self>> = msg_send_id![alloc, initWithFrame: frame];
            let this = this.expect("初始化 G502DpiTile 失败");
            this.ivars().dpi.set(dpi);
            *this.ivars().value_text.borrow_mut() = value_text.to_string();
            *this.ivars().sub_text.borrow_mut() = sub_text.to_string();
            *this.ivars().a11y_label.borrow_mut() =
                NSString::from_str(&format!("{value_text} DPI {sub_text}"));
            this
        }
    }

    pub fn dpi_value(&self) -> u16 {
        self.ivars().dpi.get()
    }

    pub fn set_selected(&self, selected: bool) {
        if self.ivars().selected.get() != selected {
            self.ivars().selected.set(selected);
            unsafe { self.setNeedsDisplay(true) };
        }
    }
}

// ---------------------------------------------------------------------- //
// 自定义视图 3: 灯效实时预览 (深色机身底板 + 灯带 / G 标发光)
// ---------------------------------------------------------------------- //

pub struct LedPreviewIvars {
    mode_idx: Cell<usize>,
    rgb: Cell<[u8; 3]>,
    brightness: Cell<u8>,
    period_ms: Cell<u16>,
    zone_idx: Cell<usize>,
    tracking_area: RefCell<Option<Retained<NSTrackingArea>>>,
}

declare_class!(
    pub struct LedPreviewView;

    unsafe impl ClassType for LedPreviewView {
        type Super = NSView;
        type Mutability = MainThreadOnly;
        const NAME: &'static str = "LedPreviewView";
    }

    impl DeclaredClass for LedPreviewView {
        type Ivars = LedPreviewIvars;
    }

    unsafe impl LedPreviewView {
        #[method_id(initWithFrame:)]
        fn init_with_frame(this: Allocated<Self>, frame: NSRect) -> Option<Retained<Self>> {
            let this = this.set_ivars(LedPreviewIvars {
                mode_idx: Cell::new(3),
                rgb: Cell::new([0, 200, 255]),
                brightness: Cell::new(100),
                period_ms: Cell::new(2000),
                zone_idx: Cell::new(0),
                tracking_area: RefCell::new(None),
            });
            unsafe { msg_send_id![super(this), initWithFrame: frame] }
        }

        #[method(updateTrackingAreas)]
        fn update_tracking_areas(&self) {
            let ivars = self.ivars();
            if let Some(old_area) = ivars.tracking_area.borrow_mut().take() {
                unsafe { self.removeTrackingArea(&old_area) };
            }
            let bounds = self.bounds();
            let opts = NSTrackingAreaOptions::NSTrackingMouseEnteredAndExited
                | NSTrackingAreaOptions::NSTrackingActiveAlways
                | NSTrackingAreaOptions::NSTrackingInVisibleRect;
            let alloc = NSTrackingArea::alloc();
            let area = unsafe {
                NSTrackingArea::initWithRect_options_owner_userInfo(
                    alloc,
                    bounds,
                    opts,
                    Some(self.as_ref()),
                    None,
                )
            };
            unsafe { self.addTrackingArea(&area) };
            *ivars.tracking_area.borrow_mut() = Some(area);
        }

        #[method(mouseEntered:)]
        fn mouse_entered(&self, _event: &NSEvent) {
            unsafe { NSCursor::pointingHandCursor().set() };
        }

        #[method(mouseExited:)]
        fn mouse_exited(&self, _event: &NSEvent) {
            unsafe { NSCursor::arrowCursor().set() };
        }

        #[method(mouseDown:)]
        fn mouse_down(&self, event: &NSEvent) {
            let win_pt = unsafe { event.locationInWindow() };
            let local_pt = self.convertPoint_fromView(win_pt, None);
            let w = self.bounds().size.width;
            let current = self.ivars().zone_idx.get();
            let clicked_zone = if local_pt.x < w * 0.5 { 1 } else { 2 };
            if current == clicked_zone {
                PopoverPanel::dispatch_zone(0);
            } else {
                PopoverPanel::dispatch_zone(clicked_zone);
            }
        }

        #[method(drawRect:)]
        fn draw_rect(&self, _dirty_rect: NSRect) {
            let bounds = self.bounds();
            let w = bounds.size.width;
            let h = bounds.size.height;
            if w < 40.0 || h < 30.0 {
                return;
            }
            let mode = self.ivars().mode_idx.get();
            let rgb = self.ivars().rgb.get();
            let bright = (self.ivars().brightness.get() as f64 / 100.0).clamp(0.15, 1.0);
            let zone = self.ivars().zone_idx.get();

            unsafe {
                let ctx = NSGraphicsContext::currentContext();

                // 整体外框剪裁 (防溢出)
                let outer_path = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(bounds, 10.0, 10.0);
                if let Some(ctx) = ctx.as_ref() {
                    ctx.saveGraphicsState();
                    outer_path.addClip();

                    // 背景底色
                    NSColor::colorWithSRGBRed_green_blue_alpha(0.063, 0.067, 0.078, 1.0).setFill();
                    outer_path.fill();

                    let card_gap = 12.0;
                    let pad_x = 10.0;
                    let pad_y = 8.0;
                    let card_w = (w - pad_x * 2.0 - card_gap) * 0.5;
                    let card_h = h - pad_y * 2.0;

                    let r_norm = rgb[0] as f64 / 255.0;
                    let g_norm = rgb[1] as f64 / 255.0;
                    let b_norm = rgb[2] as f64 / 255.0;
                    let breath_dim = if mode == 3 { 0.65 } else { 1.0 };

                    // ========================================== //
                    // 分区 1: 主要灯带 (左子卡片)
                    // ========================================== //
                    let left_active = zone == 0 || zone == 1;
                    let left_rect = NSRect::new(NSPoint::new(pad_x, pad_y), NSSize::new(card_w, card_h));
                    let left_path = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(left_rect, 8.0, 8.0);

                    if left_active {
                        NSColor::colorWithSRGBRed_green_blue_alpha(0.045, 0.160, 0.220, 0.95).setFill();
                        left_path.fill();
                        NSColor::colorWithSRGBRed_green_blue_alpha(0.0, 0.808, 1.0, 0.85).setStroke();
                        left_path.setLineWidth(1.5);
                        left_path.stroke();
                    } else {
                        NSColor::colorWithSRGBRed_green_blue_alpha(0.082, 0.086, 0.102, 0.95).setFill();
                        left_path.fill();
                        NSColor::colorWithSRGBRed_green_blue_alpha(0.18, 0.20, 0.24, 0.80).setStroke();
                        left_path.setLineWidth(1.0);
                        left_path.stroke();
                    }

                    // 标题与标签
                    let title_font = NSFont::boldSystemFontOfSize(12.5);
                    let tag_font = NSFont::monospacedDigitSystemFontOfSize_weight(10.5, NSFontWeightMedium);
                    let title_color = if left_active {
                        NSColor::colorWithSRGBRed_green_blue_alpha(0.95, 0.97, 1.0, 1.0)
                    } else {
                        NSColor::colorWithSRGBRed_green_blue_alpha(0.55, 0.58, 0.65, 1.0)
                    };
                    let tag_color = if left_active {
                        PopoverPanel::color_accent()
                    } else {
                        NSColor::colorWithSRGBRed_green_blue_alpha(0.42, 0.45, 0.52, 1.0)
                    };
                    draw_text_left("主要灯带", pad_x + 12.0, pad_y + card_h - 22.0, &title_font, &title_color);
                    let left_tag = if zone == 0 {
                        "全部同步"
                    } else if left_active {
                        "正在配置"
                    } else {
                        "未选中"
                    };
                    draw_text_right(left_tag, pad_x + card_w - 12.0, pad_y + card_h - 22.0, &tag_font, &tag_color);

                    // 主要灯带 3 段式光条模拟
                    let strip_alpha = if left_active { 1.0 } else { 0.18 };
                    let bars_cx = pad_x + card_w * 0.5;
                    let bars_cy = pad_y + card_h * 0.38;
                    let bar_w = 26.0;
                    let bar_h = 7.0;
                    let bar_spacing = 4.0;
                    let total_bars_w = bar_w * 3.0 + bar_spacing * 2.0;
                    let start_bx = bars_cx - total_bars_w * 0.5;

                    for b in 0..3 {
                        let bx = start_bx + b as f64 * (bar_w + bar_spacing);
                        let by = bars_cy - bar_h * 0.5;
                        let bar_rect = NSRect::new(NSPoint::new(bx, by), NSSize::new(bar_w, bar_h));
                        let bar_path = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(bar_rect, 3.5, 3.5);

                        if mode == 0 {
                            NSColor::colorWithSRGBRed_green_blue_alpha(0.20, 0.22, 0.26, strip_alpha).setFill();
                            bar_path.fill();
                        } else if mode == 2 {
                            let hue = (b as f64 * 45.0 + 120.0) % 360.0;
                            let c = hsv_to_rgb(hue, 1.0, 1.0);
                            let col = NSColor::colorWithSRGBRed_green_blue_alpha(
                                c[0] as f64 / 255.0,
                                c[1] as f64 / 255.0,
                                c[2] as f64 / 255.0,
                                strip_alpha * (0.60 + bright * 0.40),
                            );
                            col.setFill();
                            bar_path.fill();
                        } else {
                            if left_active {
                                let glow_c = NSColor::colorWithSRGBRed_green_blue_alpha(
                                    r_norm, g_norm, b_norm, 0.25 * bright * breath_dim * strip_alpha,
                                );
                                glow_c.setFill();
                                let expand_rect = NSRect::new(
                                    NSPoint::new(bx - 4.0, by - 4.0),
                                    NSSize::new(bar_w + 8.0, bar_h + 8.0),
                                );
                                NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(expand_rect, 5.5, 5.5).fill();
                            }
                            let core = NSColor::colorWithSRGBRed_green_blue_alpha(
                                r_norm, g_norm, b_norm, strip_alpha * (0.55 + 0.45 * bright) * breath_dim,
                            );
                            core.setFill();
                            bar_path.fill();
                        }
                    }

                    // ========================================== //
                    // 分区 2: G 标志 (右子卡片)
                    // ========================================== //
                    let right_active = zone == 0 || zone == 2;
                    let right_x = pad_x + card_w + card_gap;
                    let right_rect = NSRect::new(NSPoint::new(right_x, pad_y), NSSize::new(card_w, card_h));
                    let right_path = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(right_rect, 8.0, 8.0);

                    if right_active {
                        NSColor::colorWithSRGBRed_green_blue_alpha(0.045, 0.160, 0.220, 0.95).setFill();
                        right_path.fill();
                        NSColor::colorWithSRGBRed_green_blue_alpha(0.0, 0.808, 1.0, 0.85).setStroke();
                        right_path.setLineWidth(1.5);
                        right_path.stroke();
                    } else {
                        NSColor::colorWithSRGBRed_green_blue_alpha(0.082, 0.086, 0.102, 0.95).setFill();
                        right_path.fill();
                        NSColor::colorWithSRGBRed_green_blue_alpha(0.18, 0.20, 0.24, 0.80).setStroke();
                        right_path.setLineWidth(1.0);
                        right_path.stroke();
                    }

                    // 标题与标签
                    let title_color_r = if right_active {
                        NSColor::colorWithSRGBRed_green_blue_alpha(0.95, 0.97, 1.0, 1.0)
                    } else {
                        NSColor::colorWithSRGBRed_green_blue_alpha(0.55, 0.58, 0.65, 1.0)
                    };
                    let tag_color_r = if right_active {
                        PopoverPanel::color_accent()
                    } else {
                        NSColor::colorWithSRGBRed_green_blue_alpha(0.42, 0.45, 0.52, 1.0)
                    };
                    draw_text_left("G 标志", right_x + 12.0, pad_y + card_h - 22.0, &title_font, &title_color_r);
                    let right_tag = if zone == 0 {
                        "全部同步"
                    } else if right_active {
                        "正在配置"
                    } else {
                        "未选中"
                    };
                    draw_text_right(right_tag, right_x + card_w - 12.0, pad_y + card_h - 22.0, &tag_font, &tag_color_r);

                    // G 标发光光圈与文字模拟
                    let logo_alpha = if right_active { 1.0 } else { 0.18 };
                    let logo_cx = right_x + card_w * 0.5;
                    let logo_cy = pad_y + card_h * 0.38;
                    let r = 16.0;

                    if mode == 0 {
                        let dim = NSColor::colorWithSRGBRed_green_blue_alpha(0.20, 0.22, 0.26, logo_alpha);
                        dim.setFill();
                        NSBezierPath::bezierPathWithOvalInRect(NSRect::new(
                            NSPoint::new(logo_cx - r, logo_cy - r),
                            NSSize::new(r * 2.0, r * 2.0),
                        ))
                        .fill();
                    } else {
                        if right_active {
                            for (expand, factor) in [(10.0, 0.08), (5.0, 0.16)] {
                                let glow_c = NSColor::colorWithSRGBRed_green_blue_alpha(
                                    r_norm, g_norm, b_norm, factor * bright * breath_dim * logo_alpha,
                                );
                                glow_c.setFill();
                                let er = r + expand;
                                NSBezierPath::bezierPathWithOvalInRect(NSRect::new(
                                    NSPoint::new(logo_cx - er, logo_cy - er),
                                    NSSize::new(er * 2.0, er * 2.0),
                                ))
                                .fill();
                            }
                        }
                        let core = NSColor::colorWithSRGBRed_green_blue_alpha(
                            r_norm, g_norm, b_norm, logo_alpha * (0.55 + 0.45 * bright) * breath_dim,
                        );
                        core.setFill();
                        NSBezierPath::bezierPathWithOvalInRect(NSRect::new(
                            NSPoint::new(logo_cx - r, logo_cy - r),
                            NSSize::new(r * 2.0, r * 2.0),
                        ))
                        .fill();
                    }

                    // 居中绘制 G 字母
                    let g_font = NSFont::boldSystemFontOfSize(r * 1.05);
                    let g_color = NSColor::colorWithSRGBRed_green_blue_alpha(0.04, 0.04, 0.05, 0.95);
                    draw_text_at_center("G", logo_cx, logo_cy, &g_font, &g_color);

                    ctx.restoreGraphicsState();
                }

                outer_path.setLineWidth(1.0);
                PopoverPanel::color_card_line().setStroke();
                outer_path.stroke();
            }
        }
    }
);

impl LedPreviewView {
    pub fn new(frame: NSRect) -> Retained<Self> {
        let mtm = MainThreadMarker::new().expect("led preview 需要主线程");
        unsafe {
            let alloc = mtm.alloc::<Self>();
            let this: Option<Retained<Self>> = msg_send_id![alloc, initWithFrame: frame];
            this.expect("初始化 LedPreviewView 失败")
        }
    }

    pub fn set_state(
        &self,
        mode_idx: usize,
        rgb: [u8; 3],
        brightness: u8,
        period_ms: u16,
        zone_idx: usize,
    ) {
        let ivars = self.ivars();
        let mut changed = false;
        if ivars.mode_idx.get() != mode_idx {
            ivars.mode_idx.set(mode_idx);
            changed = true;
        }
        if ivars.rgb.get() != rgb {
            ivars.rgb.set(rgb);
            changed = true;
        }
        if ivars.brightness.get() != brightness {
            ivars.brightness.set(brightness);
            changed = true;
        }
        if ivars.period_ms.get() != period_ms {
            ivars.period_ms.set(period_ms);
            changed = true;
        }
        if ivars.zone_idx.get() != zone_idx {
            ivars.zone_idx.set(zone_idx);
            changed = true;
        }
        if changed {
            unsafe { self.setNeedsDisplay(true) };
        }
    }
}

// ---------------------------------------------------------------------- //
// 事件分发器
// ---------------------------------------------------------------------- //

declare_class!(
    pub struct PanelDispatcher;

    unsafe impl ClassType for PanelDispatcher {
        type Super = NSObject;
        type Mutability = MainThreadOnly;
        const NAME: &'static str = "G502PanelDispatcher";
    }

    impl DeclaredClass for PanelDispatcher {}

    unsafe impl PanelDispatcher {
        #[method(onSidebarTabClicked:)]
        fn on_sidebar_tab_clicked(&self, sender: Option<&NSButton>) {
            PopoverPanel::save_active_editor();
            if let Some(btn) = sender {
                let tag = unsafe { btn.tag() };
                PopoverPanel::switch_main_tab(tag as usize);
            }
        }

        #[method(onPollingRateChanged:)]
        fn on_polling_rate_changed(&self, sender: Option<&NSSegmentedControl>) {
            if let Some(ctrl) = sender {
                let seg = unsafe { ctrl.selectedSegment() };
                let rate_str = match seg {
                    0 => "125Hz (8ms)",
                    1 => "250Hz (4ms)",
                    2 => "500Hz (2ms)",
                    _ => "1000Hz (1ms)",
                };
                eprintln!("G502 报告率已设定: {rate_str}");
            }
        }

        #[method(onMouseViewChanged:)]
        fn on_mouse_view_changed(&self, sender: Option<&NSSegmentedControl>) {
            PopoverPanel::save_active_editor();
            if let Some(ctrl) = sender {
                let seg = unsafe { ctrl.selectedSegment() } as usize;
                PopoverPanel::switch_mouse_view(seg);
            }
        }

        #[method(onDpiSliderChanged:)]
        fn on_dpi_slider_changed(&self, sender: Option<&NSSlider>) {
            if let Some(slider) = sender {
                let val = unsafe { slider.doubleValue() };
                PopoverPanel::dispatch_dpi_slider(val);
            }
        }

        #[method(onZoneChanged:)]
        fn on_zone_changed(&self, sender: Option<&NSSegmentedControl>) {
            if let Some(ctrl) = sender {
                let seg = unsafe { ctrl.selectedSegment() };
                PopoverPanel::dispatch_zone(seg as usize);
            }
        }

        #[method(onEffectChanged:)]
        fn on_effect_changed(&self, sender: Option<&NSSegmentedControl>) {
            if let Some(ctrl) = sender {
                let seg = unsafe { ctrl.selectedSegment() };
                PopoverPanel::dispatch_effect(seg as usize);
            }
        }

        #[method(onColorClicked:)]
        fn on_color_clicked(&self, sender: Option<&NSControl>) {
            if let Some(btn) = sender {
                let tag = unsafe { btn.tag() };
                PopoverPanel::dispatch_color(tag as usize);
            }
        }

        #[method(onBrightnessChanged:)]
        fn on_brightness_changed(&self, sender: Option<&NSSlider>) {
            if let Some(slider) = sender {
                let val = unsafe { slider.doubleValue() } as u8;
                PopoverPanel::dispatch_brightness(val);
            }
        }

        #[method(onRateChanged:)]
        fn on_rate_changed(&self, sender: Option<&NSSlider>) {
            if let Some(slider) = sender {
                let val = unsafe { slider.doubleValue() } as u16;
                PopoverPanel::dispatch_rate(val);
            }
        }

        #[method(onToggleMacroEngine:)]
        fn on_toggle_macro_engine(&self, _sender: Option<&NSSwitch>) {
            crate::menubar::dispatch_menu_action("macro:toggle");
            PopoverPanel::sync_macro_ui();
        }

        #[method(onTextInputEnded:)]
        fn on_text_input_ended(&self, sender: Option<&NSTextField>) {
            if let Some(tf) = sender {
                let tag = unsafe { tf.tag() } as usize;
                if let Some(gk) = crate::macro_engine::G_KEYS.get(tag) {
                    let binding = crate::macro_engine::get_binding_for_key(gk.id);
                    if !crate::macro_engine::can_save_text_binding(
                        unsafe { tf.isEditable() },
                        binding.as_ref(),
                    ) {
                        return;
                    }
                    PopoverPanel::select_gkey(gk.id, false);
                    let text = if let Some(editor) = PopoverPanel::active_editor(tf) {
                        unsafe { editor.string() }.to_string()
                    } else {
                        unsafe { tf.stringValue() }.to_string()
                    };
                    if let Err(e) = crate::macro_engine::save_text_binding(gk.id, &text) {
                        eprintln!("保存 {} 文字宏失败: {e}", gk.id);
                    }
                    PopoverPanel::sync_macro_ui();
                }
            }
        }

        #[method(onRecordGKey:)]
        fn on_record_g_key(&self, sender: Option<&NSButton>) {
            if let Some(btn) = sender {
                let tag = unsafe { btn.tag() } as usize;
                if let Some(gk) = crate::macro_engine::G_KEYS.get(tag) {
                    PopoverPanel::select_gkey(gk.id, false);
                    crate::menubar::dispatch_menu_action(&format!("macro:record-{}", gk.id));
                    PopoverPanel::sync_macro_ui();
                }
            }
        }

        #[method(onClearGKey:)]
        fn on_clear_g_key(&self, sender: Option<&NSButton>) {
            if let Some(btn) = sender {
                let tag = unsafe { btn.tag() } as usize;
                if let Some(gk) = crate::macro_engine::G_KEYS.get(tag) {
                    PopoverPanel::select_gkey(gk.id, false);
                    crate::menubar::dispatch_menu_action(&format!("macro:clear-{}", gk.id));
                    PopoverPanel::sync_macro_ui();
                }
            }
        }

        #[method(onDefaultBatteryG9:)]
        fn on_default_battery_g9(&self, _sender: Option<&NSButton>) {
            PopoverPanel::select_gkey("mouse8", false);
            crate::menubar::dispatch_menu_action("macro:default-battery");
            PopoverPanel::sync_macro_ui();
        }

        #[method(onRecordSequence:)]
        fn on_record_sequence(&self, _sender: Option<&NSButton>) {
            crate::menubar::dispatch_menu_action("macro:record-sequence");
            PopoverPanel::sync_macro_ui();
        }

        #[method(onFinishRecording:)]
        fn on_finish_recording(&self, _sender: Option<&NSButton>) {
            crate::menubar::dispatch_menu_action("macro:finish-recording");
            PopoverPanel::sync_macro_ui();
        }

        #[method(onCancelRecording:)]
        fn on_cancel_recording(&self, _sender: Option<&NSButton>) {
            crate::menubar::dispatch_menu_action("macro:cancel-recording");
            PopoverPanel::sync_macro_ui();
        }

        #[method(onWindowWillClose:)]
        fn on_window_will_close(&self, _notification: Option<&objc2_foundation::NSNotification>) {
            PopoverPanel::save_active_editor();
        }

        #[method(onControlTextDidChange:)]
        fn on_control_text_did_change(&self, notification: Option<&objc2_foundation::NSNotification>) {
            if let Some(notif) = notification {
                if let Some(obj) = unsafe { notif.object() } {
                    PopoverPanel::on_text_field_changed(&obj);
                }
            }
        }
    }
);

// ---------------------------------------------------------------------- //
// 面板持有结构与主实现
// ---------------------------------------------------------------------- //

struct GKeyRow {
    key_id: &'static str,
    summary_label: Retained<NSTextField>,
    text_input: Retained<NSTextField>,
    record_btn: Retained<NSButton>,
    clear_btn: Retained<NSButton>,
    _battery_btn: Option<Retained<NSButton>>,
}

struct PanelHolder {
    panel: Retained<G502KeyPanel>,
    battery_icon: Retained<NSImageView>,
    battery_label: Retained<NSTextField>,
    mode_label: Retained<NSTextField>,
    nav_rows: Vec<Retained<G502NavRow>>,
    current_tab: Cell<usize>,
    dpi_stack: Retained<NSStackView>,
    rgb_stack: Retained<NSStackView>,
    macro_stack: Retained<NSStackView>,

    // DPI
    dpi_value_label: Retained<NSTextField>,
    dpi_slider: Retained<NSSlider>,
    dpi_tiles: Vec<Retained<G502DpiTile>>,
    active_dpi: Cell<u16>,

    // RGB 灯效
    led_preview: Retained<LedPreviewView>,
    zone_control: Retained<NSSegmentedControl>,
    effect_control: Retained<NSSegmentedControl>,
    color_label: Retained<NSTextField>,
    color_buttons: Vec<Retained<NSButton>>,
    brightness_slider: Retained<NSSlider>,
    brightness_label: Retained<NSTextField>,
    rate_slider: Retained<NSSlider>,
    rate_label: Retained<NSTextField>,
    active_rgb: Cell<[u8; 3]>,

    // 侧键宏与画布
    macro_switch: Retained<NSSwitch>,
    macro_status_dot: Retained<NSBox>,
    mouse_view_control: Retained<NSSegmentedControl>,
    mouse_canvas: Retained<G502MouseCanvas>,
    gkey_rows: Vec<GKeyRow>,
    macro_status_label: Retained<NSTextField>,
    macro_rec_seq_btn: Retained<NSButton>,
    macro_rec_finish_btn: Retained<NSButton>,
    macro_rec_cancel_btn: Retained<NSButton>,
    _dispatcher: Retained<PanelDispatcher>,
}

thread_local! {
    static HOLDER: RefCell<Option<PanelHolder>> = const { RefCell::new(None) };
}

static OPEN_REQUEST: AtomicBool = AtomicBool::new(false);

// ---- 调试快照 (G502HUB_SNAPSHOT=<目录> 时, 打开面板后自动对三个标签页截图并退出) ----
// G502HUB_SNAPSHOT_TAB=0|1|2 可只截指定单页: 连续多页截屏时 layer-backed 视图
// 的 cacheDisplayInRect 可能返回陈旧画面, 单页单进程截取可规避。
static SNAPSHOT_PATH: std::sync::OnceLock<String> = std::sync::OnceLock::new();
static SNAPSHOT_TAB: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
static SNAPSHOT_STAGE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static SNAPSHOT_AT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// DPI 预设瓷砖的档位说明
pub const DPI_TILE_SUBS: [&str; 5] = ["低速", "标准", "中速", "高速", "极速"];

pub struct PopoverPanel;

declare_class!(
    pub struct G502KeyPanel;

    unsafe impl ClassType for G502KeyPanel {
        type Super = NSPanel;
        type Mutability = MainThreadOnly;
        const NAME: &'static str = "G502KeyPanel";
    }

    impl DeclaredClass for G502KeyPanel {}

    unsafe impl G502KeyPanel {
        #[method(canBecomeKeyWindow)]
        fn can_become_key_window(&self) -> bool {
            true
        }

        #[method(canBecomeMainWindow)]
        fn can_become_main_window(&self) -> bool {
            true
        }
    }
);

impl PopoverPanel {
    pub const DPI_PRESETS: &'static [u16] = &[400, 800, 1600, 3200, 6400];
    pub const PANEL_WIDTH: f64 = 1000.0;
    pub const PANEL_HEIGHT: f64 = 660.0;
    pub const SIDEBAR_WIDTH: f64 = 176.0;

    pub fn slider_to_dpi(t: f64) -> u16 {
        let t = t.clamp(0.0, 1.0);
        let raw = 100.0 * (256.0_f64).powf(t);
        let rounded = ((raw + 25.0) / 50.0).floor() as u32 * 50;
        rounded.clamp(100, 25600) as u16
    }

    pub fn dpi_to_slider(dpi: u16) -> f64 {
        let dpi = (dpi as f64).clamp(100.0, 25600.0);
        ((dpi / 100.0).ln() / (256.0_f64).ln()).clamp(0.0, 1.0)
    }

    // ---- 调色板 (对齐设计稿 CSS 变量) ----

    /// 主文字 #1D1D1F
    pub fn color_primary_text() -> Retained<NSColor> {
        unsafe { NSColor::colorWithSRGBRed_green_blue_alpha(0.941, 0.957, 0.973, 1.0) }
    }

    /// 辅助文字 #6E6E73
    pub fn color_secondary_text() -> Retained<NSColor> {
        unsafe { NSColor::colorWithSRGBRed_green_blue_alpha(0.643, 0.690, 0.737, 1.0) }
    }

    /// 强调蓝 #007AFF
    pub fn color_accent() -> Retained<NSColor> {
        unsafe { NSColor::colorWithSRGBRed_green_blue_alpha(0.0, 0.808, 1.0, 1.0) }
    }

    /// 罗技 / macOS 活力蓝 (兼容旧命名)
    #[allow(dead_code)]
    pub fn color_accent_cyan() -> Retained<NSColor> {
        Self::color_accent()
    }

    /// 翡翠薄荷绿 #30D158
    pub fn color_success_green() -> Retained<NSColor> {
        unsafe { NSColor::colorWithSRGBRed_green_blue_alpha(0.188, 0.820, 0.345, 1.0) }
    }

    /// 战术亮橙色 #FF9500
    #[allow(dead_code)]
    pub fn color_warning_orange() -> Retained<NSColor> {
        unsafe { NSColor::colorWithSRGBRed_green_blue_alpha(1.0, 0.584, 0.0, 1.0) }
    }

    /// 录制警示红 #FF453A
    pub fn color_recording_red() -> Retained<NSColor> {
        unsafe { NSColor::colorWithSRGBRed_green_blue_alpha(1.0, 0.271, 0.227, 1.0) }
    }

    /// 空闲灰 #A1A1A6
    pub fn color_idle_gray() -> Retained<NSColor> {
        unsafe { NSColor::colorWithSRGBRed_green_blue_alpha(0.510, 0.557, 0.608, 1.0) }
    }

    /// 卡片描边 #26292E
    fn color_card_line() -> Retained<NSColor> {
        unsafe { NSColor::colorWithSRGBRed_green_blue_alpha(0.149, 0.161, 0.180, 1.0) }
    }

    /// 主窗口底色 #111216
    fn color_window_bg() -> Retained<NSColor> {
        unsafe { NSColor::colorWithSRGBRed_green_blue_alpha(0.067, 0.071, 0.086, 1.0) }
    }

    /// 侧边栏底色 #0B0C0E
    fn color_sidebar_bg() -> Retained<NSColor> {
        unsafe { NSColor::colorWithSRGBRed_green_blue_alpha(0.043, 0.047, 0.055, 1.0) }
    }

    pub fn init(mtm: MainThreadMarker) {
        let panel_width = Self::PANEL_WIDTH;
        let panel_height = Self::PANEL_HEIGHT;
        let sidebar_width = Self::SIDEBAR_WIDTH;
        let right_width = panel_width - sidebar_width;
        let content_lr = 20.0;
        let right_content_width = right_width - content_lr * 2.0;
        let card_lr = 22.0;
        let drawer_inner_width = right_content_width - card_lr * 2.0;

        let frame = NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new(panel_width, panel_height),
        );
        let style = NSWindowStyleMask::Titled
            | NSWindowStyleMask::Closable
            | NSWindowStyleMask::FullSizeContentView;

        let panel: Retained<G502KeyPanel> = unsafe {
            msg_send_id![
                mtm.alloc::<G502KeyPanel>(),
                initWithContentRect: frame,
                styleMask: style,
                backing: NSBackingStoreType::NSBackingStoreBuffered,
                defer: false,
            ]
        };

        panel.setOpaque(false);
        unsafe {
            panel.setTitle(&NSString::from_str("G502 LIGHTSPEED · 设置"));
            panel.setTitlebarAppearsTransparent(true);
            panel.setTitleVisibility(NSWindowTitleVisibility::NSWindowTitleHidden);
            panel.setAppearance(NSAppearance::appearanceNamed(NSAppearanceNameDarkAqua).as_deref());
            panel.setReleasedWhenClosed(false);
            panel.setBackgroundColor(Some(&NSColor::clearColor()));
            panel.setHasShadow(true);
            panel.setLevel(NSFloatingWindowLevel);
            panel.setBecomesKeyOnlyIfNeeded(false);
            panel.setHidesOnDeactivate(false);
            panel.setMovableByWindowBackground(true);
            panel.setCollectionBehavior(NSWindowCollectionBehavior::MoveToActiveSpace);
            if let Some(btn) = panel.standardWindowButton(NSWindowButton::NSWindowMiniaturizeButton)
            {
                btn.setHidden(true);
            }
            if let Some(btn) = panel.standardWindowButton(NSWindowButton::NSWindowZoomButton) {
                btn.setHidden(true);
            }
        }

        let dispatcher: Retained<PanelDispatcher> = unsafe { msg_send_id![mtm.alloc(), init] };

        // 主窗口底板 #F6F6F8 + 12pt 圆角 + 细边框
        let main_box = unsafe { NSBox::new(mtm) };
        unsafe {
            main_box.setBoxType(NSBoxType::NSBoxCustom);
            main_box.setCornerRadius(12.0);
            main_box.setBorderWidth(1.0);
            main_box.setBorderColor(&Self::color_card_line());
            main_box.setFillColor(&Self::color_window_bg());
            main_box.setContentViewMargins(NSSize::new(0.0, 0.0));
            panel.setContentView(Some(&main_box));
        }

        // 水平根容器: 左侧边栏 (196pt) + 右侧主区 (804pt)
        let root_h_stack = unsafe { NSStackView::new(mtm) };
        unsafe {
            root_h_stack.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
            root_h_stack.setAlignment(NSLayoutAttribute::Top);
            root_h_stack.setSpacing(0.0);
            root_h_stack.setFrame(NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(panel_width, panel_height),
            ));
            root_h_stack
                .widthAnchor()
                .constraintEqualToConstant(panel_width)
                .setActive(true);
            root_h_stack
                .heightAnchor()
                .constraintEqualToConstant(panel_height)
                .setActive(true);
            main_box.setContentView(Some(&root_h_stack));
        }

        // ==================================================================== //
        // 1. 左侧边栏 (176pt, 经典电竞深色导航控制台)
        // ==================================================================== //
        let sidebar_inner_width = sidebar_width - 24.0; // 152.0 pt, 左右各留 12pt 边距确保绝对对称
        let sidebar_box = unsafe { NSBox::new(mtm) };
        unsafe {
            sidebar_box.setBoxType(NSBoxType::NSBoxCustom);
            sidebar_box.setCornerRadius(12.0);
            sidebar_box.setBorderWidth(1.0);
            sidebar_box.setBorderColor(&Self::color_card_line());
            sidebar_box.setFillColor(&Self::color_sidebar_bg());
            sidebar_box.setContentViewMargins(NSSize::new(12.0, 14.0));
            sidebar_box
                .widthAnchor()
                .constraintEqualToConstant(sidebar_width)
                .setActive(true);
            sidebar_box
                .heightAnchor()
                .constraintEqualToConstant(panel_height)
                .setActive(true);
            root_h_stack.addArrangedSubview(&sidebar_box);
        }

        let sidebar_stack = unsafe { NSStackView::new(mtm) };
        unsafe {
            sidebar_stack.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
            sidebar_stack.setAlignment(NSLayoutAttribute::CenterX);
            sidebar_stack.setSpacing(0.0);
            sidebar_box.setContentView(Some(&sidebar_stack));
        }

        // 顶端避让系统红点 (Traffic Light)
        let close_spacer = unsafe { NSView::new(mtm) };
        unsafe {
            close_spacer
                .widthAnchor()
                .constraintEqualToConstant(sidebar_inner_width)
                .setActive(true);
            close_spacer
                .heightAnchor()
                .constraintEqualToConstant(26.0)
                .setActive(true);
            sidebar_stack.addArrangedSubview(&close_spacer);
        }

        // 设备块: 深色圆角 G 徽标 + G502 / LIGHTSPEED (水平居中对齐)
        let device_row = unsafe { NSStackView::new(mtm) };
        unsafe {
            device_row.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
            device_row.setAlignment(NSLayoutAttribute::CenterY);
            device_row.setSpacing(10.0);
            device_row
                .widthAnchor()
                .constraintEqualToConstant(sidebar_inner_width)
                .setActive(true);
            device_row
                .heightAnchor()
                .constraintEqualToConstant(44.0)
                .setActive(true);
        }

        let logo_box = unsafe { NSBox::new(mtm) };
        unsafe {
            logo_box.setBoxType(NSBoxType::NSBoxCustom);
            logo_box.setCornerRadius(8.0);
            logo_box.setBorderWidth(1.0);
            let border_c = NSColor::colorWithSRGBRed_green_blue_alpha(0.0, 0.808, 1.0, 0.45);
            logo_box.setBorderColor(&border_c);
            let bg_c = NSColor::colorWithSRGBRed_green_blue_alpha(0.063, 0.075, 0.095, 1.0);
            logo_box.setFillColor(&bg_c);
            logo_box.setContentViewMargins(NSSize::new(2.0, 2.0));
            logo_box
                .widthAnchor()
                .constraintEqualToConstant(36.0)
                .setActive(true);
            logo_box
                .heightAnchor()
                .constraintEqualToConstant(36.0)
                .setActive(true);
        }
        let logo_label = unsafe { NSTextField::labelWithString(&NSString::from_str("G"), mtm) };
        unsafe {
            logo_label.setFont(Some(&NSFont::boldSystemFontOfSize(18.0)));
            let cyan = PopoverPanel::color_accent();
            logo_label.setTextColor(Some(&cyan));
            logo_label.setAlignment(NSTextAlignment::Center);
            logo_box.setContentView(Some(&logo_label));
        }

        let device_col = unsafe { NSStackView::new(mtm) };
        unsafe {
            device_col.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
            device_col.setAlignment(NSLayoutAttribute::Leading);
            device_col.setSpacing(1.0);
        }
        let device_name = unsafe { NSTextField::labelWithString(&NSString::from_str("G502"), mtm) };
        unsafe {
            device_name.setFont(Some(&NSFont::boldSystemFontOfSize(14.0)));
            device_name.setTextColor(Some(&Self::color_primary_text()));
            device_col.addArrangedSubview(&device_name);
        }
        let device_model =
            unsafe { NSTextField::labelWithString(&NSString::from_str("LIGHTSPEED"), mtm) };
        unsafe {
            device_model.setFont(Some(&NSFont::systemFontOfSize(10.5)));
            device_model.setTextColor(Some(&Self::color_secondary_text()));
            device_col.addArrangedSubview(&device_model);
        }

        let dev_center_stack = unsafe { NSStackView::new(mtm) };
        unsafe {
            dev_center_stack.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
            dev_center_stack.setAlignment(NSLayoutAttribute::CenterY);
            dev_center_stack.setSpacing(10.0);
            dev_center_stack.addArrangedSubview(&logo_box);
            dev_center_stack.addArrangedSubview(&device_col);
            device_row.setAlignment(NSLayoutAttribute::CenterX);
            device_row.addArrangedSubview(&dev_center_stack);
            sidebar_stack.addArrangedSubview(&device_row);
        }

        // 顶部分隔线
        let top_div = unsafe { Self::create_separator(mtm, sidebar_inner_width) };
        unsafe {
            sidebar_stack.addArrangedSubview(&top_div);
        }

        let nav_gap = unsafe { NSView::new(mtm) };
        unsafe {
            nav_gap
                .widthAnchor()
                .constraintEqualToConstant(sidebar_inner_width)
                .setActive(true);
            nav_gap
                .heightAnchor()
                .constraintEqualToConstant(18.0)
                .setActive(true);
            sidebar_stack.addArrangedSubview(&nav_gap);
        }

        let nav_label_row = unsafe { NSStackView::new(mtm) };
        unsafe {
            nav_label_row.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
            nav_label_row.setAlignment(NSLayoutAttribute::CenterY);
            nav_label_row.setSpacing(0.0);
            nav_label_row
                .widthAnchor()
                .constraintEqualToConstant(sidebar_inner_width)
                .setActive(true);
            nav_label_row
                .heightAnchor()
                .constraintEqualToConstant(18.0)
                .setActive(true);
        }
        let nav_label_spacer = unsafe { NSView::new(mtm) };
        unsafe {
            nav_label_spacer
                .widthAnchor()
                .constraintEqualToConstant(12.0)
                .setActive(true);
            nav_label_row.addArrangedSubview(&nav_label_spacer);
        }
        let nav_label = unsafe { NSTextField::labelWithString(&NSString::from_str("功能设置"), mtm) };
        unsafe {
            nav_label.setFont(Some(&NSFont::boldSystemFontOfSize(11.0)));
            nav_label.setTextColor(Some(&Self::color_idle_gray()));
            nav_label_row.addArrangedSubview(&nav_label);
        }
        let nav_label_tail = unsafe { Self::create_horizontal_spacer(mtm) };
        unsafe {
            nav_label_row.addArrangedSubview(&nav_label_tail);
            sidebar_stack.addArrangedSubview(&nav_label_row);
        }

        let nav_gap2 = unsafe { NSView::new(mtm) };
        unsafe {
            nav_gap2
                .widthAnchor()
                .constraintEqualToConstant(sidebar_inner_width)
                .setActive(true);
            nav_gap2
                .heightAnchor()
                .constraintEqualToConstant(8.0)
                .setActive(true);
            sidebar_stack.addArrangedSubview(&nav_gap2);
        }

        // 3 个导航行: DPI / 灯效 / 宏·文字 (44pt 舒适电竞导航条，8pt 间距)
        let nav_specs: [(&str, &str, usize); 3] = [
            ("speedometer", "DPI 灵敏度", 0),
            ("lightbulb", "LIGHTSYNC 灯效", 1),
            ("keyboard", "按键指派与宏", 2),
        ];
        let mut nav_rows = Vec::new();
        for (i, (symbol, title, tag)) in nav_specs.iter().enumerate() {
            let icon_normal = unsafe { tinted_symbol(symbol, 18.0, &Self::color_accent()) };
            let icon_selected = unsafe { tinted_symbol(symbol, 18.0, &NSColor::whiteColor()) };
            let row = G502NavRow::new(
                NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(sidebar_inner_width, 44.0)),
                *tag,
                title,
                icon_normal,
                icon_selected,
            );
            unsafe {
                row.widthAnchor()
                    .constraintEqualToConstant(sidebar_inner_width)
                    .setActive(true);
                row.heightAnchor()
                    .constraintEqualToConstant(44.0)
                    .setActive(true);
                sidebar_stack.addArrangedSubview(&row);
            }
            if i < 2 {
                let row_gap = unsafe { NSView::new(mtm) };
                unsafe {
                    row_gap
                        .widthAnchor()
                        .constraintEqualToConstant(sidebar_inner_width)
                        .setActive(true);
                    row_gap
                        .heightAnchor()
                        .constraintEqualToConstant(8.0)
                        .setActive(true);
                    sidebar_stack.addArrangedSubview(&row_gap);
                }
            }
            nav_rows.push(row);
        }
        nav_rows[0].set_selected(true);

        // 下部弹性占位 (与上部弹性占位对应，完美平衡垂直空间)
        let sidebar_bottom_spacer = unsafe { NSView::new(mtm) };
        unsafe {
            sidebar_bottom_spacer.setContentHuggingPriority_forOrientation(
                1.0,
                NSLayoutConstraintOrientation::Vertical,
            );
            sidebar_stack.addArrangedSubview(&sidebar_bottom_spacer);
        }

        // 底部硬件状态卡片 (彻底消除下半部分大面积空白不对称)
        let bottom_div = unsafe { Self::create_separator(mtm, sidebar_inner_width) };
        unsafe {
            sidebar_stack.addArrangedSubview(&bottom_div);
        }

        let bottom_gap = unsafe { NSView::new(mtm) };
        unsafe {
            bottom_gap
                .widthAnchor()
                .constraintEqualToConstant(sidebar_inner_width)
                .setActive(true);
            bottom_gap
                .heightAnchor()
                .constraintEqualToConstant(12.0)
                .setActive(true);
            sidebar_stack.addArrangedSubview(&bottom_gap);
        }

        let info_box = unsafe { NSBox::new(mtm) };
        unsafe {
            info_box.setBoxType(NSBoxType::NSBoxCustom);
            info_box.setCornerRadius(8.0);
            info_box.setBorderWidth(1.0);
            let border_c = NSColor::colorWithSRGBRed_green_blue_alpha(0.149, 0.161, 0.180, 1.0);
            info_box.setBorderColor(&border_c);
            let bg_c = NSColor::colorWithSRGBRed_green_blue_alpha(0.055, 0.059, 0.071, 1.0);
            info_box.setFillColor(&bg_c);
            info_box.setContentViewMargins(NSSize::new(10.0, 8.0));
            info_box
                .widthAnchor()
                .constraintEqualToConstant(sidebar_inner_width)
                .setActive(true);
            info_box
                .heightAnchor()
                .constraintEqualToConstant(58.0)
                .setActive(true);
            sidebar_stack.addArrangedSubview(&info_box);
        }

        let info_stack = unsafe { NSStackView::new(mtm) };
        unsafe {
            info_stack.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
            info_stack.setAlignment(NSLayoutAttribute::Leading);
            info_stack.setSpacing(2.0);
            info_box.setContentView(Some(&info_stack));
        }

        let status_row = unsafe { NSStackView::new(mtm) };
        unsafe {
            status_row.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
            status_row.setAlignment(NSLayoutAttribute::CenterY);
            status_row.setSpacing(6.0);
            info_stack.addArrangedSubview(&status_row);
        }
        let dot_view = unsafe { NSBox::new(mtm) };
        unsafe {
            dot_view.setBoxType(NSBoxType::NSBoxCustom);
            dot_view.setCornerRadius(3.5);
            dot_view.setBorderWidth(0.0);
            dot_view.setFillColor(&Self::color_success_green());
            dot_view
                .widthAnchor()
                .constraintEqualToConstant(7.0)
                .setActive(true);
            dot_view
                .heightAnchor()
                .constraintEqualToConstant(7.0)
                .setActive(true);
            status_row.addArrangedSubview(&dot_view);
        }
        let status_label = unsafe { NSTextField::labelWithString(&NSString::from_str("LIGHTSPEED 无线"), mtm) };
        unsafe {
            status_label.setFont(Some(&NSFont::boldSystemFontOfSize(11.0)));
            status_label.setTextColor(Some(&Self::color_primary_text()));
            status_row.addArrangedSubview(&status_label);
        }

        let sensor_label = unsafe {
            NSTextField::labelWithString(&NSString::from_str("HERO 25K 传感器 · 1ms"), mtm)
        };
        unsafe {
            sensor_label.setFont(Some(&NSFont::systemFontOfSize(9.5)));
            sensor_label.setTextColor(Some(&Self::color_secondary_text()));
            info_stack.addArrangedSubview(&sensor_label);
        }

        let mem_label = unsafe {
            NSTextField::labelWithString(&NSString::from_str("板载模式支持 · 1000Hz"), mtm)
        };
        unsafe {
            mem_label.setFont(Some(&NSFont::systemFontOfSize(9.0)));
            mem_label.setTextColor(Some(&Self::color_idle_gray()));
            info_stack.addArrangedSubview(&mem_label);
        }

        // ==================================================================== //
        // 2. 右侧主工作区 (804pt)
        // ==================================================================== //
        let content_stack = unsafe { NSStackView::new(mtm) };
        unsafe {
            content_stack.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
            content_stack.setAlignment(NSLayoutAttribute::CenterX);
            content_stack.setSpacing(8.0);
            content_stack.setEdgeInsets(NSEdgeInsets {
                top: 14.0,
                left: content_lr,
                bottom: 16.0,
                right: content_lr,
            });
            content_stack
                .widthAnchor()
                .constraintEqualToConstant(right_width)
                .setActive(true);
            content_stack
                .heightAnchor()
                .constraintEqualToConstant(panel_height)
                .setActive(true);
            root_h_stack.addArrangedSubview(&content_stack);
        }

        // 2.1 工具栏: 标题 + 视角切换 + 电量胶囊
        let header_row = unsafe { NSStackView::new(mtm) };
        unsafe {
            header_row.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
            header_row.setAlignment(NSLayoutAttribute::CenterY);
            header_row.setSpacing(10.0);
            header_row
                .widthAnchor()
                .constraintEqualToConstant(right_content_width)
                .setActive(true);
            header_row
                .heightAnchor()
                .constraintEqualToConstant(40.0)
                .setActive(true);
        }

        let title_col = unsafe { NSStackView::new(mtm) };
        unsafe {
            title_col.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
            title_col.setAlignment(NSLayoutAttribute::Leading);
            title_col.setSpacing(1.0);
        }
        let title_label =
            unsafe { NSTextField::labelWithString(&NSString::from_str("G502 LIGHTSPEED"), mtm) };
        unsafe {
            title_label.setFont(Some(&NSFont::boldSystemFontOfSize(16.0)));
            title_label.setTextColor(Some(&Self::color_primary_text()));
            title_col.addArrangedSubview(&title_label);
        }
        let subtitle_label = unsafe {
            NSTextField::labelWithString(
                &NSString::from_str("HERO 25K 传感器 · 最高 25,600 DPI"),
                mtm,
            )
        };
        unsafe {
            subtitle_label.setFont(Some(&NSFont::systemFontOfSize(11.0)));
            subtitle_label.setTextColor(Some(&Self::color_secondary_text()));
            title_col.addArrangedSubview(&subtitle_label);
            header_row.addArrangedSubview(&title_col);
        }

        let header_spacer = unsafe { Self::create_horizontal_spacer(mtm) };
        unsafe {
            header_row.addArrangedSubview(&header_spacer);
        }

        // 视角切换: 顶部视角 (G7–G11) / 侧面视角 (G4–G6)
        let view_labels = NSArray::from_vec(vec![
            NSString::from_str("顶部视角 (G7–G11)"),
            NSString::from_str("侧面视角 (G4–G6)"),
        ]);
        let mouse_view_control = unsafe {
            NSSegmentedControl::segmentedControlWithLabels_trackingMode_target_action(
                &view_labels,
                NSSegmentSwitchTracking::SelectOne,
                Some(&dispatcher),
                Some(sel!(onMouseViewChanged:)),
                mtm,
            )
        };
        unsafe {
            mouse_view_control.setSelectedSegment(0);
            mouse_view_control.setSegmentStyle(NSSegmentStyle::Rounded);
            header_row.addArrangedSubview(&mouse_view_control);
        }

        // 电量胶囊: 图标 + 百分比 + 模式
        let battery_pill_stack = unsafe { NSStackView::new(mtm) };
        unsafe {
            battery_pill_stack.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
            battery_pill_stack.setAlignment(NSLayoutAttribute::CenterY);
            battery_pill_stack.setSpacing(7.0);
        }
        let battery_icon = unsafe {
            NSImageView::initWithFrame(
                mtm.alloc(),
                NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(26.0, 14.0)),
            )
        };
        unsafe {
            battery_icon
                .widthAnchor()
                .constraintEqualToConstant(26.0)
                .setActive(true);
            battery_icon
                .heightAnchor()
                .constraintEqualToConstant(14.0)
                .setActive(true);
            battery_pill_stack.addArrangedSubview(&battery_icon);
        }
        let battery_label = unsafe { NSTextField::labelWithString(&NSString::from_str("--"), mtm) };
        unsafe {
            battery_label.setFont(Some(&NSFont::boldSystemFontOfSize(12.5)));
            battery_label.setTextColor(Some(&Self::color_primary_text()));
            battery_pill_stack.addArrangedSubview(&battery_label);
        }
        let mode_label =
            unsafe { NSTextField::labelWithString(&NSString::from_str("主机控制"), mtm) };
        unsafe {
            mode_label.setFont(Some(&NSFont::systemFontOfSize(11.0)));
            mode_label.setTextColor(Some(&Self::color_secondary_text()));
            battery_pill_stack.addArrangedSubview(&mode_label);
        }
        let battery_pill = unsafe { NSBox::new(mtm) };
        unsafe {
            battery_pill.setBoxType(NSBoxType::NSBoxCustom);
            battery_pill.setCornerRadius(13.0);
            battery_pill.setBorderWidth(1.0);
            battery_pill.setBorderColor(&Self::color_card_line());
            battery_pill.setFillColor(&NSColor::colorWithSRGBRed_green_blue_alpha(
                0.12, 0.13, 0.16, 1.0,
            ));
            battery_pill.setContentViewMargins(NSSize::new(10.0, 4.0));
            battery_pill.setContentView(Some(&battery_pill_stack));
            header_row.addArrangedSubview(&battery_pill);
            content_stack.addArrangedSubview(&header_row);
        }

        // 2.2 按键示意图画布卡片 (内含深色实机画布)
        let diagram_box = unsafe { NSBox::new(mtm) };
        unsafe {
            diagram_box.setBoxType(NSBoxType::NSBoxCustom);
            diagram_box.setCornerRadius(12.0);
            diagram_box.setBorderWidth(1.0);
            diagram_box.setBorderColor(&Self::color_card_line());
            diagram_box.setFillColor(&NSColor::colorWithSRGBRed_green_blue_alpha(
                0.075, 0.078, 0.090, 1.0,
            ));
            diagram_box.setContentViewMargins(NSSize::new(8.0, 8.0));
            diagram_box
                .widthAnchor()
                .constraintEqualToConstant(right_content_width)
                .setActive(true);
            diagram_box
                .heightAnchor()
                .constraintEqualToConstant(256.0)
                .setActive(true);
            content_stack.addArrangedSubview(&diagram_box);
        }

        let diagram_inner_width = right_content_width - 16.0;
        let mouse_canvas = G502MouseCanvas::new(
            NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(diagram_inner_width, 240.0),
            ),
            mtm,
        );
        unsafe {
            mouse_canvas
                .widthAnchor()
                .constraintEqualToConstant(diagram_inner_width)
                .setActive(true);
            mouse_canvas
                .heightAnchor()
                .constraintEqualToConstant(240.0)
                .setActive(true);
            diagram_box.setContentView(Some(&mouse_canvas));
        }

        // 2.3 功能抽屉深色卡片
        let drawer_box = unsafe { NSBox::new(mtm) };
        unsafe {
            drawer_box.setBoxType(NSBoxType::NSBoxCustom);
            drawer_box.setCornerRadius(12.0);
            drawer_box.setBorderWidth(1.0);
            drawer_box.setBorderColor(&Self::color_card_line());
            drawer_box.setFillColor(&NSColor::colorWithSRGBRed_green_blue_alpha(
                0.094, 0.098, 0.114, 1.0,
            ));
            drawer_box.setContentViewMargins(NSSize::new(card_lr, 14.0));
            drawer_box
                .widthAnchor()
                .constraintEqualToConstant(right_content_width)
                .setActive(true);
            drawer_box
                .heightAnchor()
                .constraintEqualToConstant(314.0)
                .setActive(true);
            content_stack.addArrangedSubview(&drawer_box);
        }

        let drawer_container = unsafe { NSStackView::new(mtm) };
        unsafe {
            drawer_container.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
            drawer_container.setAlignment(NSLayoutAttribute::CenterX);
            drawer_container.setSpacing(0.0);
            drawer_box.setContentView(Some(&drawer_container));
        }

        // ==================================================================== //
        // 抽屉 0: DPI 灵敏度与报告率
        // ==================================================================== //
        let dpi_stack = unsafe { NSStackView::new(mtm) };
        unsafe {
            dpi_stack.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
            dpi_stack.setAlignment(NSLayoutAttribute::CenterX);
            dpi_stack.setSpacing(10.0);
            dpi_stack
                .widthAnchor()
                .constraintEqualToConstant(drawer_inner_width)
                .setActive(true);
            drawer_container.addArrangedSubview(&dpi_stack);
        }

        // 顶栏: 标题 + 当前活动档位大数值
        let dpi_header_row = unsafe { NSStackView::new(mtm) };
        unsafe {
            dpi_header_row.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
            dpi_header_row.setAlignment(NSLayoutAttribute::CenterY);
            dpi_header_row.setSpacing(8.0);
            dpi_header_row
                .widthAnchor()
                .constraintEqualToConstant(drawer_inner_width)
                .setActive(true);
        }
        let dpi_title_col = unsafe { NSStackView::new(mtm) };
        unsafe {
            dpi_title_col.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
            dpi_title_col.setAlignment(NSLayoutAttribute::Leading);
            dpi_title_col.setSpacing(1.0);
            dpi_header_row.addArrangedSubview(&dpi_title_col);
        }
        let dpi_title =
            unsafe { NSTextField::labelWithString(&NSString::from_str("灵敏度 (DPI)"), mtm) };
        unsafe {
            dpi_title.setFont(Some(&NSFont::boldSystemFontOfSize(14.5)));
            dpi_title.setTextColor(Some(&Self::color_primary_text()));
            dpi_title_col.addArrangedSubview(&dpi_title);
        }
        let dpi_hint =
            unsafe { NSTextField::labelWithString(&NSString::from_str("当前活动档位"), mtm) };
        unsafe {
            dpi_hint.setFont(Some(&NSFont::systemFontOfSize(11.0)));
            dpi_hint.setTextColor(Some(&Self::color_secondary_text()));
            dpi_title_col.addArrangedSubview(&dpi_hint);
        }
        let dpi_header_spacer = unsafe { Self::create_horizontal_spacer(mtm) };
        unsafe {
            dpi_header_row.addArrangedSubview(&dpi_header_spacer);
        }
        let dpi_value_label =
            unsafe { NSTextField::labelWithString(&NSString::from_str("1,600 DPI"), mtm) };
        unsafe {
            let font = NSFont::monospacedDigitSystemFontOfSize_weight(24.0, NSFontWeightBold);
            dpi_value_label.setFont(Some(&font));
            dpi_value_label.setTextColor(Some(&Self::color_accent()));
            dpi_value_label.setAlignment(NSTextAlignment::Right);
            dpi_header_row.addArrangedSubview(&dpi_value_label);
            dpi_stack.addArrangedSubview(&dpi_header_row);
        }

        // 5 档预设瓷砖 (400 / 800 / 1600 / 3200 / 6400)
        let tile_row = unsafe { NSStackView::new(mtm) };
        unsafe {
            tile_row.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
            tile_row.setAlignment(NSLayoutAttribute::CenterY);
            tile_row.setSpacing(10.0);
            tile_row
                .widthAnchor()
                .constraintEqualToConstant(drawer_inner_width)
                .setActive(true);
            tile_row
                .heightAnchor()
                .constraintEqualToConstant(56.0)
                .setActive(true);
            dpi_stack.addArrangedSubview(&tile_row);
        }
        let tile_width = (drawer_inner_width - 4.0 * 10.0) / 5.0;
        let mut dpi_tiles = Vec::new();
        for (i, &preset) in Self::DPI_PRESETS.iter().enumerate() {
            let tile = G502DpiTile::new(
                NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(tile_width, 56.0)),
                preset,
                &format_dpi(preset),
                DPI_TILE_SUBS[i],
            );
            unsafe {
                tile.widthAnchor()
                    .constraintEqualToConstant(tile_width)
                    .setActive(true);
                tile.heightAnchor()
                    .constraintEqualToConstant(56.0)
                    .setActive(true);
                tile_row.addArrangedSubview(&tile);
            }
            dpi_tiles.push(tile);
        }

        // 无级平滑微调滑杆 (100 ~ 25600)
        let dpi_slider_row = unsafe { NSStackView::new(mtm) };
        unsafe {
            dpi_slider_row.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
            dpi_slider_row.setAlignment(NSLayoutAttribute::CenterY);
            dpi_slider_row.setSpacing(8.0);
            dpi_slider_row
                .widthAnchor()
                .constraintEqualToConstant(drawer_inner_width)
                .setActive(true);
        }
        let dpi_min_label =
            unsafe { NSTextField::labelWithString(&NSString::from_str("100"), mtm) };
        unsafe {
            dpi_min_label.setFont(Some(&NSFont::systemFontOfSize(10.5)));
            dpi_min_label.setTextColor(Some(&Self::color_secondary_text()));
            dpi_slider_row.addArrangedSubview(&dpi_min_label);
        }
        let dpi_slider = unsafe {
            NSSlider::sliderWithValue_minValue_maxValue_target_action(
                Self::dpi_to_slider(1600),
                0.0,
                1.0,
                Some(&dispatcher),
                Some(sel!(onDpiSliderChanged:)),
                mtm,
            )
        };
        unsafe {
            dpi_slider.setContinuous(true);
            dpi_slider.setContentHuggingPriority_forOrientation(
                1.0,
                NSLayoutConstraintOrientation::Horizontal,
            );
            dpi_slider_row.addArrangedSubview(&dpi_slider);
        }
        let dpi_max_label =
            unsafe { NSTextField::labelWithString(&NSString::from_str("25,600"), mtm) };
        unsafe {
            dpi_max_label.setFont(Some(&NSFont::systemFontOfSize(10.5)));
            dpi_max_label.setTextColor(Some(&Self::color_secondary_text()));
            dpi_slider_row.addArrangedSubview(&dpi_max_label);
            dpi_stack.addArrangedSubview(&dpi_slider_row);
        }

        let dpi_sep = unsafe { Self::create_separator(mtm, drawer_inner_width) };
        unsafe {
            dpi_stack.addArrangedSubview(&dpi_sep);
        }

        // 报告率控制行
        let rate_control_row = unsafe { NSStackView::new(mtm) };
        unsafe {
            rate_control_row.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
            rate_control_row.setAlignment(NSLayoutAttribute::CenterY);
            rate_control_row.setSpacing(12.0);
            rate_control_row
                .widthAnchor()
                .constraintEqualToConstant(drawer_inner_width)
                .setActive(true);
        }
        let rate_title_col = unsafe { NSStackView::new(mtm) };
        unsafe {
            rate_title_col.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
            rate_title_col.setAlignment(NSLayoutAttribute::Leading);
            rate_title_col.setSpacing(1.0);
            rate_control_row.addArrangedSubview(&rate_title_col);
        }
        let rate_heading =
            unsafe { NSTextField::labelWithString(&NSString::from_str("报告率"), mtm) };
        unsafe {
            rate_heading.setFont(Some(&NSFont::boldSystemFontOfSize(13.0)));
            rate_heading.setTextColor(Some(&Self::color_primary_text()));
            rate_title_col.addArrangedSubview(&rate_heading);
        }
        let rate_sub = unsafe {
            NSTextField::labelWithString(&NSString::from_str("每秒向电脑报告位置的次数"), mtm)
        };
        unsafe {
            rate_sub.setFont(Some(&NSFont::systemFontOfSize(10.5)));
            rate_sub.setTextColor(Some(&Self::color_secondary_text()));
            rate_title_col.addArrangedSubview(&rate_sub);
        }
        let rate_h_spacer = unsafe { Self::create_horizontal_spacer(mtm) };
        unsafe {
            rate_control_row.addArrangedSubview(&rate_h_spacer);
        }
        let rate_labels = NSArray::from_vec(vec![
            NSString::from_str("125 Hz"),
            NSString::from_str("250 Hz"),
            NSString::from_str("500 Hz"),
            NSString::from_str("1000 Hz"),
        ]);
        let polling_rate_control = unsafe {
            NSSegmentedControl::segmentedControlWithLabels_trackingMode_target_action(
                &rate_labels,
                NSSegmentSwitchTracking::SelectOne,
                Some(&dispatcher),
                Some(sel!(onPollingRateChanged:)),
                mtm,
            )
        };
        unsafe {
            polling_rate_control.setSelectedSegment(3); // 默认 1000Hz 电竞标准
            polling_rate_control.setSegmentStyle(NSSegmentStyle::Rounded);
            rate_control_row.addArrangedSubview(&polling_rate_control);
            dpi_stack.addArrangedSubview(&rate_control_row);
        }

        // 快捷提示条 (深青底)
        let tip_box = unsafe { NSBox::new(mtm) };
        unsafe {
            tip_box.setBoxType(NSBoxType::NSBoxCustom);
            tip_box.setCornerRadius(9.0);
            tip_box.setBorderWidth(1.0);
            let border_c = NSColor::colorWithSRGBRed_green_blue_alpha(0.0, 0.808, 1.0, 0.20);
            tip_box.setBorderColor(&border_c);
            let tip_bg = NSColor::colorWithSRGBRed_green_blue_alpha(0.0, 0.808, 1.0, 0.08);
            tip_box.setFillColor(&tip_bg);
            tip_box.setContentViewMargins(NSSize::new(12.0, 8.0));
            tip_box
                .widthAnchor()
                .constraintEqualToConstant(drawer_inner_width)
                .setActive(true);
            tip_box
                .heightAnchor()
                .constraintEqualToConstant(44.0)
                .setActive(true);
            dpi_stack.addArrangedSubview(&tip_box);
        }
        let tip_stack = unsafe { NSStackView::new(mtm) };
        unsafe {
            tip_stack.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
            tip_stack.setAlignment(NSLayoutAttribute::CenterY);
            tip_stack.setSpacing(8.0);
            tip_box.setContentView(Some(&tip_stack));
        }
        let tip_icon = unsafe {
            NSImageView::initWithFrame(
                mtm.alloc(),
                NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(15.0, 15.0)),
            )
        };
        unsafe {
            tip_icon.setImage(Some(&tinted_symbol(
                "info.circle",
                15.0,
                &Self::color_accent(),
            )));
            tip_stack.addArrangedSubview(&tip_icon);
        }
        let tip_label = unsafe {
            NSTextField::labelWithString(
                &NSString::from_str(
                    "使用鼠标左侧的 G8 / G7 键可在档位间即时加减切换；点击上方档位也可直接选定。",
                ),
                mtm,
            )
        };
        unsafe {
            tip_label.setFont(Some(&NSFont::systemFontOfSize(11.5)));
            tip_label.setTextColor(Some(&Self::color_secondary_text()));
            tip_stack.addArrangedSubview(&tip_label);
        }

        // ==================================================================== //
        // 抽屉 1: LIGHTSYNC RGB 灯效
        // ==================================================================== //
        let rgb_stack = unsafe { NSStackView::new(mtm) };
        unsafe {
            rgb_stack.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
            rgb_stack.setAlignment(NSLayoutAttribute::CenterX);
            rgb_stack.setSpacing(8.0);
            rgb_stack
                .widthAnchor()
                .constraintEqualToConstant(drawer_inner_width)
                .setActive(true);
            rgb_stack.setHidden(true);
            drawer_container.addArrangedSubview(&rgb_stack);
        }

        // 分区与效果模式并排顶栏
        let rgb_top_row = unsafe { NSStackView::new(mtm) };
        unsafe {
            rgb_top_row.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
            rgb_top_row.setAlignment(NSLayoutAttribute::Top);
            rgb_top_row.setSpacing(32.0);
            rgb_top_row
                .widthAnchor()
                .constraintEqualToConstant(drawer_inner_width)
                .setActive(true);
        }

        let zone_col = unsafe { NSStackView::new(mtm) };
        unsafe {
            zone_col.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
            zone_col.setAlignment(NSLayoutAttribute::Leading);
            zone_col.setSpacing(5.0);
            rgb_top_row.addArrangedSubview(&zone_col);
        }
        let zone_title =
            unsafe { NSTextField::labelWithString(&NSString::from_str("灯效分区"), mtm) };
        unsafe {
            zone_title.setFont(Some(&NSFont::boldSystemFontOfSize(13.0)));
            zone_title.setTextColor(Some(&Self::color_primary_text()));
            zone_col.addArrangedSubview(&zone_title);
        }
        let zone_labels = NSArray::from_vec(vec![
            NSString::from_str("全部联动"),
            NSString::from_str("主要灯带"),
            NSString::from_str("G 标志"),
        ]);
        let zone_control = unsafe {
            NSSegmentedControl::segmentedControlWithLabels_trackingMode_target_action(
                &zone_labels,
                NSSegmentSwitchTracking::SelectOne,
                Some(&dispatcher),
                Some(sel!(onZoneChanged:)),
                mtm,
            )
        };
        unsafe {
            zone_control.setSelectedSegment(0);
            zone_control.setSegmentStyle(NSSegmentStyle::TexturedRounded);
            zone_control
                .heightAnchor()
                .constraintEqualToConstant(28.0)
                .setActive(true);
            for i in 0..3 {
                zone_control.setWidth_forSegment(88.0, i);
            }
            zone_col.addArrangedSubview(&zone_control);
        }

        let effect_col = unsafe { NSStackView::new(mtm) };
        unsafe {
            effect_col.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
            effect_col.setAlignment(NSLayoutAttribute::Leading);
            effect_col.setSpacing(5.0);
            rgb_top_row.addArrangedSubview(&effect_col);
        }
        let effect_title =
            unsafe { NSTextField::labelWithString(&NSString::from_str("效果模式"), mtm) };
        unsafe {
            effect_title.setFont(Some(&NSFont::boldSystemFontOfSize(13.0)));
            effect_title.setTextColor(Some(&Self::color_primary_text()));
            effect_col.addArrangedSubview(&effect_title);
        }
        let effect_labels = NSArray::from_vec(vec![
            NSString::from_str("关闭"),
            NSString::from_str("固定色"),
            NSString::from_str("彩色循环"),
            NSString::from_str("呼吸"),
        ]);
        let effect_control = unsafe {
            NSSegmentedControl::segmentedControlWithLabels_trackingMode_target_action(
                &effect_labels,
                NSSegmentSwitchTracking::SelectOne,
                Some(&dispatcher),
                Some(sel!(onEffectChanged:)),
                mtm,
            )
        };
        unsafe {
            effect_control.setSelectedSegment(3);
            effect_control.setSegmentStyle(NSSegmentStyle::TexturedRounded);
            effect_control
                .heightAnchor()
                .constraintEqualToConstant(28.0)
                .setActive(true);
            for i in 0..4 {
                effect_control.setWidth_forSegment(76.0, i);
            }
            effect_col.addArrangedSubview(&effect_control);
            rgb_stack.addArrangedSubview(&rgb_top_row);
        }

        // 机身灯效预览 (深色底板 + 灯带/G 标双区独立发光指示)
        let led_preview = LedPreviewView::new(NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new(drawer_inner_width, 88.0),
        ));
        unsafe {
            led_preview
                .widthAnchor()
                .constraintEqualToConstant(drawer_inner_width)
                .setActive(true);
            led_preview
                .heightAnchor()
                .constraintEqualToConstant(88.0)
                .setActive(true);
            rgb_stack.addArrangedSubview(&led_preview);
        }

        // 预设色彩
        let color_header = unsafe { NSStackView::new(mtm) };
        unsafe {
            color_header.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
            color_header.setAlignment(NSLayoutAttribute::CenterY);
            color_header.setSpacing(6.0);
            color_header
                .widthAnchor()
                .constraintEqualToConstant(drawer_inner_width)
                .setActive(true);
        }
        let color_title =
            unsafe { NSTextField::labelWithString(&NSString::from_str("预设色彩"), mtm) };
        unsafe {
            color_title.setFont(Some(&NSFont::boldSystemFontOfSize(13.0)));
            color_title.setTextColor(Some(&Self::color_primary_text()));
            color_header.addArrangedSubview(&color_title);
        }
        let color_spacer = unsafe { Self::create_horizontal_spacer(mtm) };
        unsafe {
            color_header.addArrangedSubview(&color_spacer);
        }
        let color_label =
            unsafe { NSTextField::labelWithString(&NSString::from_str("选定: --"), mtm) };
        unsafe {
            let font = NSFont::systemFontOfSize(11.5);
            color_label.setFont(Some(&font));
            color_label.setTextColor(Some(&Self::color_secondary_text()));
            color_header.addArrangedSubview(&color_label);
            rgb_stack.addArrangedSubview(&color_header);
        }

        let color_row = unsafe { NSStackView::new(mtm) };
        unsafe {
            color_row.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
            color_row.setDistribution(NSStackViewDistribution::EqualSpacing);
            color_row.setAlignment(NSLayoutAttribute::CenterY);
            color_row
                .widthAnchor()
                .constraintEqualToConstant(drawer_inner_width)
                .setActive(true);
        }
        let mut color_buttons = Vec::new();
        for (i, (c_name, c_hex)) in crate::menubar::LED_COLORS.iter().enumerate() {
            let btn = unsafe {
                NSButton::buttonWithTitle_target_action(
                    &NSString::from_str(""),
                    Some(&dispatcher),
                    Some(sel!(onColorClicked:)),
                    mtm,
                )
            };
            unsafe {
                btn.setButtonType(NSButtonType::MomentaryChange);
                btn.setBordered(false);
                btn.setTag(i as isize);
                let rgb = crate::menubar::hex_rgb(c_hex);
                let swatch = Self::create_swatch_image(rgb, false, mtm);
                btn.setImage(Some(&swatch));
                btn.setImagePosition(NSCellImagePosition::NSImageOnly);
                btn.setToolTip(Some(&NSString::from_str(c_name)));
                btn.widthAnchor()
                    .constraintEqualToConstant(26.0)
                    .setActive(true);
                btn.heightAnchor()
                    .constraintEqualToConstant(26.0)
                    .setActive(true);
                color_row.addArrangedSubview(&btn);
            }
            color_buttons.push(btn);
        }
        unsafe {
            rgb_stack.addArrangedSubview(&color_row);
            rgb_stack.addArrangedSubview(&Self::create_separator(mtm, drawer_inner_width));
        }

        // 亮度行
        let brightness_row = unsafe { NSStackView::new(mtm) };
        unsafe {
            brightness_row.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
            brightness_row.setAlignment(NSLayoutAttribute::CenterY);
            brightness_row.setSpacing(10.0);
            brightness_row
                .widthAnchor()
                .constraintEqualToConstant(drawer_inner_width)
                .setActive(true);
        }
        let bright_title =
            unsafe { NSTextField::labelWithString(&NSString::from_str("亮度"), mtm) };
        unsafe {
            bright_title.setFont(Some(&NSFont::systemFontOfSize(12.5)));
            bright_title.setTextColor(Some(&Self::color_primary_text()));
            bright_title
                .widthAnchor()
                .constraintEqualToConstant(44.0)
                .setActive(true);
            brightness_row.addArrangedSubview(&bright_title);
        }
        let brightness_slider = unsafe {
            NSSlider::sliderWithValue_minValue_maxValue_target_action(
                100.0,
                0.0,
                100.0,
                Some(&dispatcher),
                Some(sel!(onBrightnessChanged:)),
                mtm,
            )
        };
        unsafe {
            brightness_slider.setContinuous(true);
            brightness_slider.setContentHuggingPriority_forOrientation(
                1.0,
                NSLayoutConstraintOrientation::Horizontal,
            );
            brightness_row.addArrangedSubview(&brightness_slider);
        }
        let brightness_label =
            unsafe { NSTextField::labelWithString(&NSString::from_str("100%"), mtm) };
        unsafe {
            let font = NSFont::monospacedDigitSystemFontOfSize_weight(11.5, NSFontWeightMedium);
            brightness_label.setFont(Some(&font));
            brightness_label.setTextColor(Some(&Self::color_accent()));
            brightness_label.setAlignment(NSTextAlignment::Right);
            brightness_label
                .widthAnchor()
                .constraintEqualToConstant(84.0)
                .setActive(true);
            brightness_row.addArrangedSubview(&brightness_label);
            rgb_stack.addArrangedSubview(&brightness_row);
        }

        // 速率行
        let rate_col2 = unsafe { NSStackView::new(mtm) };
        unsafe {
            rate_col2.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
            rate_col2.setAlignment(NSLayoutAttribute::CenterY);
            rate_col2.setSpacing(10.0);
            rate_col2
                .widthAnchor()
                .constraintEqualToConstant(drawer_inner_width)
                .setActive(true);
        }
        let rate_title2 =
            unsafe { NSTextField::labelWithString(&NSString::from_str("速率"), mtm) };
        unsafe {
            rate_title2.setFont(Some(&NSFont::systemFontOfSize(12.5)));
            rate_title2.setTextColor(Some(&Self::color_primary_text()));
            rate_title2
                .widthAnchor()
                .constraintEqualToConstant(44.0)
                .setActive(true);
            rate_col2.addArrangedSubview(&rate_title2);
        }
        let rate_slider = unsafe {
            NSSlider::sliderWithValue_minValue_maxValue_target_action(
                2000.0,
                1000.0,
                10000.0,
                Some(&dispatcher),
                Some(sel!(onRateChanged:)),
                mtm,
            )
        };
        unsafe {
            rate_slider.setContinuous(true);
            rate_slider.setContentHuggingPriority_forOrientation(
                1.0,
                NSLayoutConstraintOrientation::Horizontal,
            );
            rate_col2.addArrangedSubview(&rate_slider);
        }
        let rate_label =
            unsafe { NSTextField::labelWithString(&NSString::from_str("2.0s (适中)"), mtm) };
        unsafe {
            let font = NSFont::monospacedDigitSystemFontOfSize_weight(11.5, NSFontWeightMedium);
            rate_label.setFont(Some(&font));
            rate_label.setTextColor(Some(&Self::color_accent()));
            rate_label.setAlignment(NSTextAlignment::Right);
            rate_label
                .widthAnchor()
                .constraintEqualToConstant(84.0)
                .setActive(true);
            rate_col2.addArrangedSubview(&rate_label);
            rgb_stack.addArrangedSubview(&rate_col2);
        }

        // ==================================================================== //
        // 抽屉 2: 按键指派与宏
        // ==================================================================== //
        let macro_stack = unsafe { NSStackView::new(mtm) };
        unsafe {
            macro_stack.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
            macro_stack.setAlignment(NSLayoutAttribute::CenterX);
            macro_stack.setSpacing(8.0);
            macro_stack
                .widthAnchor()
                .constraintEqualToConstant(drawer_inner_width)
                .setActive(true);
            macro_stack.setHidden(true);
            drawer_container.addArrangedSubview(&macro_stack);
        }

        // 顶栏: NSSwitch 引擎开关 + 状态圆点 + 状态文本
        let macro_header_row = unsafe { NSStackView::new(mtm) };
        unsafe {
            macro_header_row.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
            macro_header_row.setAlignment(NSLayoutAttribute::CenterY);
            macro_header_row.setSpacing(8.0);
            macro_header_row
                .widthAnchor()
                .constraintEqualToConstant(drawer_inner_width)
                .setActive(true);
        }
        let macro_switch = unsafe { NSSwitch::new(mtm) };
        unsafe {
            macro_switch.setTarget(Some(&dispatcher));
            macro_switch.setAction(Some(sel!(onToggleMacroEngine:)));
            macro_switch.setState(objc2_app_kit::NSControlStateValueOn);
            macro_header_row.addArrangedSubview(&macro_switch);
        }
        let macro_title_label =
            unsafe { NSTextField::labelWithString(&NSString::from_str("宏引擎"), mtm) };
        unsafe {
            macro_title_label.setFont(Some(&NSFont::boldSystemFontOfSize(14.5)));
            macro_title_label.setTextColor(Some(&Self::color_primary_text()));
            macro_header_row.addArrangedSubview(&macro_title_label);
        }
        let macro_header_spacer = unsafe { Self::create_horizontal_spacer(mtm) };
        unsafe {
            macro_header_row.addArrangedSubview(&macro_header_spacer);
        }
        let macro_status_dot = unsafe { NSBox::new(mtm) };
        unsafe {
            macro_status_dot.setBoxType(NSBoxType::NSBoxCustom);
            macro_status_dot.setCornerRadius(4.0);
            macro_status_dot.setBorderWidth(0.0);
            macro_status_dot.setFillColor(&Self::color_success_green());
            macro_status_dot
                .widthAnchor()
                .constraintEqualToConstant(8.0)
                .setActive(true);
            macro_status_dot
                .heightAnchor()
                .constraintEqualToConstant(8.0)
                .setActive(true);
            macro_header_row.addArrangedSubview(&macro_status_dot);
        }
        let macro_status_label = unsafe {
            NSTextField::labelWithString(
                &NSString::from_str("宏引擎就绪 (支持组合键与指定文字)"),
                mtm,
            )
        };
        unsafe {
            macro_status_label.setFont(Some(&NSFont::systemFontOfSize(11.0)));
            macro_status_label.setTextColor(Some(&Self::color_secondary_text()));
            macro_header_row.addArrangedSubview(&macro_status_label);
            macro_stack.addArrangedSubview(&macro_header_row);
        }

        // 滚动列表 (G4..G11 按键指派)
        let macro_rows_stack = unsafe { NSStackView::new(mtm) };
        unsafe {
            macro_rows_stack.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
            macro_rows_stack.setAlignment(NSLayoutAttribute::Leading);
            macro_rows_stack.setSpacing(6.0);
            macro_rows_stack.setEdgeInsets(NSEdgeInsets {
                top: 4.0,
                left: 2.0,
                bottom: 4.0,
                right: 2.0,
            });
        }
        let macro_scroll = unsafe {
            NSScrollView::initWithFrame(
                mtm.alloc(),
                NSRect::new(
                    NSPoint::new(0.0, 0.0),
                    NSSize::new(drawer_inner_width, 190.0),
                ),
            )
        };
        unsafe {
            macro_scroll.setHasVerticalScroller(true);
            macro_scroll.setDrawsBackground(false);
            // 文档视图改用 autolayout 锚定: 宽度固定 + 左/上钉住, 高度由内容撑开,
            // 否则 autoresizing 会把文档视图压成 0×0 (行宽 0 导致整列表不可见)。
            macro_rows_stack.setTranslatesAutoresizingMaskIntoConstraints(false);
            macro_scroll.setDocumentView(Some(&macro_rows_stack));
            macro_rows_stack
                .widthAnchor()
                .constraintEqualToConstant(drawer_inner_width - 16.0)
                .setActive(true);
            macro_rows_stack
                .leadingAnchor()
                .constraintEqualToAnchor(&macro_scroll.leadingAnchor())
                .setActive(true);
            macro_rows_stack
                .topAnchor()
                .constraintEqualToAnchor(&macro_scroll.topAnchor())
                .setActive(true);
            macro_scroll
                .widthAnchor()
                .constraintEqualToConstant(drawer_inner_width)
                .setActive(true);
            macro_scroll
                .heightAnchor()
                .constraintEqualToConstant(216.0)
                .setActive(true);
            macro_stack.addArrangedSubview(&macro_scroll);
        }

        let mut gkey_rows = Vec::new();
        // 2 列网格布局: 8 个按键排成 4 行全部可见, 无需滚动 (之前单列 8 行超出抽屉
        // 高度被裁剪, 判定为列表缺失)。滚动视图保留兜底: 内容超高时仍可滚动。
        let grid_width = drawer_inner_width - 16.0; // 避开滚动条
        let grid_gutter = 14.0;
        let cell_width = (grid_width - grid_gutter) / 2.0;
        let mut grid_rows: Vec<Retained<NSStackView>> = Vec::new();
        for _ in 0..(crate::macro_engine::G_KEYS.len() + 1) / 2 {
            let grid_row = unsafe { NSStackView::new(mtm) };
            unsafe {
                grid_row.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
                grid_row.setAlignment(NSLayoutAttribute::CenterY);
                grid_row.setSpacing(grid_gutter);
                grid_row
                    .widthAnchor()
                    .constraintEqualToConstant(grid_width)
                    .setActive(true);
                macro_rows_stack.addArrangedSubview(&grid_row);
            }
            grid_rows.push(grid_row);
        }
        let row_width = cell_width;
        for (i, gk) in crate::macro_engine::G_KEYS.iter().enumerate() {
            let row = unsafe { NSStackView::new(mtm) };
            unsafe {
                row.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
                row.setAlignment(NSLayoutAttribute::CenterY);
                // Fill 分布: 低拥抱优先级的编辑列自动横向扩展, 按钮靠右排布
                row.setDistribution(NSStackViewDistribution::Fill);
                row.setSpacing(6.0);
                row.widthAnchor()
                    .constraintEqualToConstant(row_width)
                    .setActive(true);
            }

            // G-Key 硬件键帽徽章
            let badge_box = unsafe { NSBox::new(mtm) };
            unsafe {
                badge_box.setBoxType(NSBoxType::NSBoxCustom);
                badge_box.setCornerRadius(6.0);
                badge_box.setBorderWidth(1.0);
                let border_c = NSColor::colorWithSRGBRed_green_blue_alpha(0.20, 0.23, 0.28, 1.0);
                badge_box.setBorderColor(&border_c);
                let bg_c = NSColor::colorWithSRGBRed_green_blue_alpha(0.12, 0.14, 0.17, 1.0);
                badge_box.setFillColor(&bg_c);
                badge_box.setContentViewMargins(NSSize::new(2.0, 2.0));
                badge_box
                    .widthAnchor()
                    .constraintEqualToConstant(82.0)
                    .setActive(true);
            }
            let badge_title = match gk.name {
                "G4" => "G4 后退",
                "G5" => "G5 前进",
                "G6" => "G6 瞄准",
                "G7" => "G7 DPI-",
                "G8" => "G8 DPI+",
                "G9" => "G9 电量",
                "G10" => "G10 滚轮左",
                "G11" => "G11 滚轮右",
                _ => gk.name,
            };
            let name_label =
                unsafe { NSTextField::labelWithString(&NSString::from_str(badge_title), mtm) };
            unsafe {
                name_label.setFont(Some(&NSFont::boldSystemFontOfSize(10.0)));
                name_label.setTextColor(Some(&Self::color_accent()));
                name_label.setAlignment(NSTextAlignment::Center);
                badge_box.setContentView(Some(&name_label));
                row.addArrangedSubview(&badge_box);
            }

            let editor_stack = unsafe { NSStackView::new(mtm) };
            unsafe {
                editor_stack.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
                // Fill 分布让文字输入框横向铺满编辑列 (避免 placeholder 被压缩)
                editor_stack.setDistribution(NSStackViewDistribution::Fill);
                editor_stack.setSpacing(1.0);
                editor_stack.setContentHuggingPriority_forOrientation(
                    1.0,
                    NSLayoutConstraintOrientation::Horizontal,
                );
            }

            let summary_label =
                unsafe { NSTextField::labelWithString(&NSString::from_str("未绑定"), mtm) };
            unsafe {
                summary_label.setFont(Some(&NSFont::systemFontOfSize(9.0)));
                summary_label.setTextColor(Some(&Self::color_secondary_text()));
                editor_stack.addArrangedSubview(&summary_label);
            }

            let text_input = unsafe { NSTextField::new(mtm) };
            unsafe {
                text_input.setEditable(true);
                text_input.setSelectable(true);
                text_input.setBezeled(true);
                text_input.setFont(Some(&NSFont::systemFontOfSize(11.0)));
                text_input.setPlaceholderString(Some(&NSString::from_str(
                    "在此输入自动打字文字 (输入即生效)...",
                )));
                text_input.setTextColor(Some(&Self::color_primary_text()));
                let input_bg = NSColor::colorWithSRGBRed_green_blue_alpha(0.12, 0.13, 0.16, 1.0);
                text_input.setBackgroundColor(Some(&input_bg));
                text_input.setDrawsBackground(true);
                text_input
                    .heightAnchor()
                    .constraintEqualToConstant(24.0)
                    .setActive(true);
                text_input.setTarget(Some(&dispatcher));
                text_input.setAction(Some(sel!(onTextInputEnded:)));
                if let Some(cell) = text_input.cell() {
                    cell.setSendsActionOnEndEditing(true);
                }
                text_input.setTag(i as isize);
                editor_stack.addArrangedSubview(&text_input);
                row.addArrangedSubview(&editor_stack);
            }

            let record_btn = unsafe {
                NSButton::buttonWithTitle_target_action(
                    &NSString::from_str("录制"),
                    Some(&dispatcher),
                    Some(sel!(onRecordGKey:)),
                    mtm,
                )
            };
            unsafe {
                record_btn.setFont(Some(&NSFont::systemFontOfSize(10.0)));
                record_btn.setTag(i as isize);
                row.addArrangedSubview(&record_btn);
            }

            let mut battery_btn = None;
            if gk.is_battery_default {
                let btn = unsafe {
                    NSButton::buttonWithTitle_target_action(
                        &NSString::from_str("恢复电量"),
                        Some(&dispatcher),
                        Some(sel!(onDefaultBatteryG9:)),
                        mtm,
                    )
                };
                unsafe {
                    btn.setFont(Some(&NSFont::systemFontOfSize(10.0)));
                    row.addArrangedSubview(&btn);
                }
                battery_btn = Some(btn);
            }

            let clear_btn = unsafe {
                NSButton::buttonWithTitle_target_action(
                    &NSString::from_str("清空"),
                    Some(&dispatcher),
                    Some(sel!(onClearGKey:)),
                    mtm,
                )
            };
            unsafe {
                clear_btn.setFont(Some(&NSFont::systemFontOfSize(10.0)));
                clear_btn.setTag(i as isize);
                row.addArrangedSubview(&clear_btn);
                grid_rows[i / 2].addArrangedSubview(&row);
            }

            gkey_rows.push(GKeyRow {
                key_id: gk.id,
                summary_label,
                text_input,
                record_btn,
                clear_btn,
                _battery_btn: battery_btn,
            });
        }

        // 底部快捷操作条
        let macro_action_row = unsafe { NSStackView::new(mtm) };
        unsafe {
            macro_action_row.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
            macro_action_row.setAlignment(NSLayoutAttribute::CenterY);
            macro_action_row.setSpacing(10.0);
            macro_action_row
                .widthAnchor()
                .constraintEqualToConstant(drawer_inner_width)
                .setActive(true);
        }
        let macro_rec_seq_btn = unsafe {
            NSButton::buttonWithTitle_target_action(
                &NSString::from_str("按键序列"),
                Some(&dispatcher),
                Some(sel!(onRecordSequence:)),
                mtm,
            )
        };
        unsafe {
            macro_rec_seq_btn.setFont(Some(&NSFont::systemFontOfSize(11.0)));
            macro_action_row.addArrangedSubview(&macro_rec_seq_btn);
        }
        let macro_action_spacer = unsafe { Self::create_horizontal_spacer(mtm) };
        unsafe {
            macro_action_row.addArrangedSubview(&macro_action_spacer);
        }
        let macro_rec_cancel_btn = unsafe {
            NSButton::buttonWithTitle_target_action(
                &NSString::from_str("取消录制"),
                Some(&dispatcher),
                Some(sel!(onCancelRecording:)),
                mtm,
            )
        };
        unsafe {
            macro_rec_cancel_btn.setFont(Some(&NSFont::systemFontOfSize(11.0)));
            macro_action_row.addArrangedSubview(&macro_rec_cancel_btn);
        }
        let macro_rec_finish_btn = unsafe {
            NSButton::buttonWithTitle_target_action(
                &NSString::from_str("完成保存"),
                Some(&dispatcher),
                Some(sel!(onFinishRecording:)),
                mtm,
            )
        };
        unsafe {
            macro_rec_finish_btn.setFont(Some(&NSFont::boldSystemFontOfSize(11.0)));
            macro_action_row.addArrangedSubview(&macro_rec_finish_btn);
            macro_stack.addArrangedSubview(&macro_action_row);
        }

        unsafe {
            let center = NSNotificationCenter::defaultCenter();
            center.addObserver_selector_name_object(
                &dispatcher,
                sel!(onWindowWillClose:),
                Some(objc2_app_kit::NSWindowWillCloseNotification),
                Some(&panel),
            );
            center.addObserver_selector_name_object(
                &dispatcher,
                sel!(onControlTextDidChange:),
                Some(objc2_app_kit::NSControlTextDidChangeNotification),
                None,
            );
        }

        let holder = PanelHolder {
            panel,
            battery_icon,
            battery_label,
            mode_label,
            nav_rows,
            current_tab: Cell::new(0),
            dpi_stack,
            rgb_stack,
            macro_stack,
            dpi_value_label,
            dpi_slider,
            dpi_tiles,
            active_dpi: Cell::new(1600),
            led_preview,
            zone_control,
            effect_control,
            color_label,
            color_buttons,
            brightness_slider,
            brightness_label,
            rate_slider,
            rate_label,
            active_rgb: Cell::new([0, 200, 255]),
            macro_switch,
            macro_status_dot,
            mouse_view_control,
            mouse_canvas,
            gkey_rows,
            macro_status_label,
            macro_rec_seq_btn,
            macro_rec_finish_btn,
            macro_rec_cancel_btn,
            _dispatcher: dispatcher,
        };

        HOLDER.with(|cell| {
            *cell.borrow_mut() = Some(holder);
        });

        // 初始化预览与瓷砖状态
        Self::sync_ui_from_config();

        // 调试快照目录 (自动化视觉验证)
        if let Ok(dir) = std::env::var("G502HUB_SNAPSHOT") {
            let _ = SNAPSHOT_PATH.set(dir);
            Self::request_show();
        }
        if let Ok(tab) = std::env::var("G502HUB_SNAPSHOT_TAB") {
            if let Ok(tab) = tab.parse::<usize>() {
                let _ = SNAPSHOT_TAB.set(tab.min(2));
            }
        }
    }

    /// 渲染面板内容为 PNG (仅用于调试快照, 无需屏幕录制权限)
    fn take_snapshot(path: &str) {
        use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRepPropertyKey};
        use objc2_foundation::NSDictionary;
        HOLDER.with(|cell| {
            if let Some(h) = cell.borrow().as_ref() {
                unsafe {
                    if let Some(content) = h.panel.contentView() {
                        let bounds = content.bounds();
                        if let Some(rep) = content.bitmapImageRepForCachingDisplayInRect(bounds) {
                            content.cacheDisplayInRect_toBitmapImageRep(bounds, &rep);
                            let props = NSDictionary::<
                                NSBitmapImageRepPropertyKey,
                                objc2::runtime::AnyObject,
                            >::new();
                            if let Some(data) = rep.representationUsingType_properties(
                                NSBitmapImageFileType::PNG,
                                &props,
                            ) {
                                let _ =
                                    data.writeToFile_atomically(&NSString::from_str(path), true);
                            }
                        }
                    }
                }
            }
        });
    }

    /// 快照状态机: 依次截取 DPI / 灯效 / 宏 三页后退出进程
    fn poll_snapshot() {
        let Some(dir) = SNAPSHOT_PATH.get() else {
            return;
        };
        let shown_at = SNAPSHOT_AT.load(Ordering::SeqCst);
        if shown_at == 0 {
            return;
        }
        let stage = SNAPSHOT_STAGE.load(Ordering::SeqCst);

        // 单页快照模式: 显示稳定后切到目标页,稍等布局/绘制完成再截取
        if let Some(tab) = SNAPSHOT_TAB.get() {
            match stage {
                0 => {
                    if now_ms().saturating_sub(shown_at) < 2000 {
                        return;
                    }
                    SNAPSHOT_STAGE.store(1, Ordering::SeqCst);
                    SNAPSHOT_AT.store(now_ms(), Ordering::SeqCst);
                    Self::switch_main_tab(*tab);
                }
                _ => {
                    if now_ms().saturating_sub(shown_at) < 800 {
                        return;
                    }
                    let file = format!("{dir}/tab{tab}.png");
                    Self::take_snapshot(&file);
                    eprintln!("snapshot saved: {file}");
                    std::process::exit(0);
                }
            }
            return;
        }

        if stage >= 3 {
            return;
        }
        let delay_ms = if stage == 0 { 2000 } else { 1000 };
        if now_ms().saturating_sub(shown_at) < delay_ms {
            return;
        }
        SNAPSHOT_STAGE.store(stage + 1, Ordering::SeqCst);
        SNAPSHOT_AT.store(now_ms(), Ordering::SeqCst);
        let file = format!("{dir}/tab{stage}.png");
        Self::take_snapshot(&file);
        eprintln!("snapshot saved: {file}");
        match stage {
            0 => Self::switch_main_tab(1),
            1 => Self::switch_main_tab(2),
            _ => std::process::exit(0),
        }
    }

    unsafe fn create_separator(mtm: MainThreadMarker, width: f64) -> Retained<NSBox> {
        let sep = NSBox::new(mtm);
        sep.setBoxType(NSBoxType::NSBoxSeparator);
        sep.widthAnchor()
            .constraintEqualToConstant(width)
            .setActive(true);
        sep
    }

    unsafe fn create_horizontal_spacer(mtm: MainThreadMarker) -> Retained<NSView> {
        let spacer = NSView::new(mtm);
        spacer.setContentHuggingPriority_forOrientation(
            1.0,
            NSLayoutConstraintOrientation::Horizontal,
        );
        spacer
    }

    /// 生成宝石级圆形颜色点图标 (用于预设色彩矩阵，支持选中发光外环)
    #[allow(deprecated)]
    unsafe fn create_swatch_image(
        rgb: [u8; 3],
        is_selected: bool,
        mtm: MainThreadMarker,
    ) -> Retained<NSImage> {
        let size = NSSize::new(26.0, 26.0);
        let img = NSImage::initWithSize(mtm.alloc(), size);
        img.lockFocus();

        let color = NSColor::colorWithSRGBRed_green_blue_alpha(
            rgb[0] as f64 / 255.0,
            rgb[1] as f64 / 255.0,
            rgb[2] as f64 / 255.0,
            1.0,
        );

        if is_selected {
            // 外层选中指示对焦环 (电竞罗技青)
            let ring_rect = NSRect::new(NSPoint::new(1.0, 1.0), NSSize::new(24.0, 24.0));
            let ring = NSBezierPath::bezierPathWithOvalInRect(ring_rect);
            let ring_color = NSColor::colorWithSRGBRed_green_blue_alpha(0.0, 0.808, 1.0, 1.0);
            ring_color.setStroke();
            ring.setLineWidth(2.0);
            ring.stroke();

            // 内层核心色彩珠
            let inner_rect = NSRect::new(NSPoint::new(4.5, 4.5), NSSize::new(17.0, 17.0));
            let inner = NSBezierPath::bezierPathWithOvalInRect(inner_rect);
            color.setFill();
            inner.fill();

            let inner_rim = NSColor::colorWithWhite_alpha(1.0, 0.40);
            inner_rim.setStroke();
            inner.setLineWidth(0.75);
            inner.stroke();
        } else {
            // 常态微缩饱满圆形色彩珠
            let disc_rect = NSRect::new(NSPoint::new(3.0, 3.0), NSSize::new(20.0, 20.0));
            let disc = NSBezierPath::bezierPathWithOvalInRect(disc_rect);
            color.setFill();
            disc.fill();

            let rim = NSColor::colorWithWhite_alpha(1.0, 0.35);
            rim.setStroke();
            disc.setLineWidth(0.75);
            disc.stroke();
        }

        img.unlockFocus();
        img
    }

    pub fn switch_main_tab(tab_idx: usize) {
        HOLDER.with(|cell| {
            if let Some(h) = cell.borrow().as_ref() {
                let tab_idx = tab_idx.min(2);
                h.current_tab.set(tab_idx);

                h.dpi_stack.setHidden(tab_idx != 0);
                h.rgb_stack.setHidden(tab_idx != 1);
                h.macro_stack.setHidden(tab_idx != 2);

                for (i, row) in h.nav_rows.iter().enumerate() {
                    row.set_selected(i == tab_idx);
                }

                h.mouse_canvas.set_tab(tab_idx);
                if tab_idx == 1 {
                    unsafe {
                        h.mouse_view_control.setSelectedSegment(0);
                    }
                    h.mouse_canvas.set_mode(CanvasMode::Top);
                    let seg = unsafe { h.zone_control.selectedSegment() } as usize;
                    let key = match seg {
                        1 => Some("主要灯带".to_string()),
                        2 => Some("G 标志".to_string()),
                        _ => None,
                    };
                    h.mouse_canvas.set_selected_key(key);
                }
            }
        });
        if tab_idx == 2 {
            Self::sync_macro_ui();
        }
    }

    pub fn switch_mouse_view(view_idx: usize) {
        HOLDER.with(|cell| {
            if let Some(h) = cell.borrow().as_ref() {
                h.mouse_canvas.set_mode(if view_idx == 0 {
                    CanvasMode::Top
                } else {
                    CanvasMode::Side
                });
            }
        });
    }

    fn gkey_name_for_id(key_id: &str) -> Option<&'static str> {
        crate::macro_engine::get_gkey_by_id(key_id).map(|gk| gk.name)
    }

    pub(crate) fn select_gkey(key_id: &str, focus_editor: bool) {
        HOLDER.with(|cell| {
            if let Some(h) = cell.borrow().as_ref() {
                let Some(gk) = crate::macro_engine::get_gkey_by_id(key_id) else {
                    return;
                };
                let top = matches!(gk.name, "G7" | "G8" | "G9" | "G10" | "G11");
                let segment = if top { 0 } else { 1 };
                unsafe {
                    h.mouse_view_control.setSelectedSegment(segment);
                }
                h.mouse_canvas.set_mode(if top {
                    CanvasMode::Top
                } else {
                    CanvasMode::Side
                });
                h.mouse_canvas.set_selected_key(Some(gk.name.to_string()));

                if focus_editor {
                    PopoverPanel::switch_main_tab(2);
                    if let Some(row) = h.gkey_rows.iter().find(|row| row.key_id == gk.id) {
                        unsafe {
                            row.text_input.scrollRectToVisible(row.text_input.bounds());
                        }
                        h.panel.makeFirstResponder(Some(&row.text_input));
                    }
                }
            }
        });
    }

    /// 供后台线程发起请求，在主线程 pump 周期中弹出浮窗
    pub fn request_show() {
        OPEN_REQUEST.store(true, Ordering::SeqCst);
    }

    /// 在主线程 pump 周期消费弹窗请求
    pub fn poll_open_request() {
        if OPEN_REQUEST.swap(false, Ordering::SeqCst) {
            Self::show_at(None);
        }
        Self::poll_snapshot();
    }

    pub fn toggle_at(tray_rect: Option<tray_icon::Rect>) {
        let is_visible = HOLDER.with(|cell| {
            cell.borrow()
                .as_ref()
                .map(|h| h.panel.isVisible())
                .unwrap_or(false)
        });

        if !is_visible {
            Self::show_at(tray_rect);
        } else {
            HOLDER.with(|cell| {
                if let Some(h) = cell.borrow().as_ref() {
                    h.panel.orderOut(None);
                }
            });
        }
    }

    pub fn show_at(_tray_rect: Option<tray_icon::Rect>) {
        Self::sync_ui_from_config();
        HOLDER.with(|cell| {
            if let Some(h) = cell.borrow().as_ref() {
                let mtm = MainThreadMarker::from(&*h.panel);
                let panel_frame = h.panel.frame();

                if h.panel.isVisible() {
                    let app = NSApplication::sharedApplication(mtm);
                    #[allow(deprecated)]
                    app.activateIgnoringOtherApps(true);
                    h.panel.makeKeyAndOrderFront(None);
                    return;
                }

                // 计算屏幕中央黄金展示位置 (自适应可见工作区与视网膜缩放)
                let screen_frame = NSScreen::mainScreen(mtm)
                    .map(|s| s.visibleFrame())
                    .unwrap_or(NSRect::new(
                        NSPoint::new(0.0, 0.0),
                        NSSize::new(1440.0, 900.0),
                    ));

                let ideal_x = screen_frame.origin.x
                    + (screen_frame.size.width - panel_frame.size.width) * 0.5;
                let ideal_y = screen_frame.origin.y
                    + (screen_frame.size.height - panel_frame.size.height) * 0.52;

                let min_x = screen_frame.origin.x + 16.0;
                let max_x = (screen_frame.origin.x + screen_frame.size.width
                    - panel_frame.size.width
                    - 16.0)
                    .max(min_x);
                let min_y = screen_frame.origin.y + 16.0;
                let max_y = (screen_frame.origin.y + screen_frame.size.height
                    - panel_frame.size.height
                    - 16.0)
                    .max(min_y);

                let final_x = ideal_x.clamp(min_x, max_x);
                let final_y = ideal_y.clamp(min_y, max_y);

                unsafe {
                    h.panel.setFrameOrigin(NSPoint::new(final_x, final_y));
                    let app = NSApplication::sharedApplication(mtm);
                    #[allow(deprecated)]
                    app.activateIgnoringOtherApps(true);

                    h.panel.makeKeyAndOrderFront(None);
                }

                // 快照状态机: 从本次显示开始计时
                SNAPSHOT_AT.store(now_ms(), Ordering::SeqCst);
                SNAPSHOT_STAGE.store(0, Ordering::SeqCst);
            }
        });
    }

    pub(crate) fn on_text_field_changed(obj: &objc2::runtime::AnyObject) {
        HOLDER.with(|cell| {
            if let Some(h) = cell.borrow().as_ref() {
                for row in &h.gkey_rows {
                    let tf_ptr =
                        Retained::as_ptr(&row.text_input) as *const objc2::runtime::AnyObject;
                    let obj_ptr = obj as *const objc2::runtime::AnyObject;
                    if tf_ptr == obj_ptr {
                        let binding = crate::macro_engine::get_binding_for_key(row.key_id);
                        if !crate::macro_engine::can_save_text_binding(
                            unsafe { row.text_input.isEditable() },
                            binding.as_ref(),
                        ) {
                            break;
                        }
                        let text = if let Some(editor) = Self::active_editor(&row.text_input) {
                            unsafe { editor.string() }.to_string()
                        } else {
                            unsafe { row.text_input.stringValue() }.to_string()
                        };
                        if text.is_empty() {
                            let _ = crate::macro_engine::clear_binding(row.key_id);
                            unsafe {
                                row.summary_label
                                    .setStringValue(&NSString::from_str("未绑定"));
                                row.summary_label
                                    .setTextColor(Some(&Self::color_secondary_text()));
                                row.clear_btn.setEnabled(false);
                            }
                        } else {
                            // 保存原始文本, 保留用户输入的前后及所有空格
                            let _ = crate::macro_engine::save_text_binding(row.key_id, &text);
                            unsafe {
                                row.summary_label
                                    .setStringValue(&NSString::from_str("文字 · 自动输入"));
                                row.summary_label.setTextColor(Some(&Self::color_accent()));
                                row.clear_btn.setEnabled(true);
                            }
                        }
                        break;
                    }
                }
            }
        });
    }

    pub(crate) fn active_editor(field: &NSTextField) -> Option<Retained<objc2_app_kit::NSText>> {
        unsafe { msg_send_id![field, currentEditor] }
    }

    pub(crate) fn save_active_editor() {
        HOLDER.with(|cell| {
            if let Some(h) = cell.borrow().as_ref() {
                for row in &h.gkey_rows {
                    let binding = crate::macro_engine::get_binding_for_key(row.key_id);
                    if !crate::macro_engine::can_save_text_binding(
                        unsafe { row.text_input.isEditable() },
                        binding.as_ref(),
                    ) {
                        continue;
                    }
                    let Some(editor) = Self::active_editor(&row.text_input) else {
                        continue;
                    };
                    let text = unsafe { editor.string() }.to_string();
                    if !text.is_empty() {
                        let _ = crate::macro_engine::save_text_binding(row.key_id, &text);
                    } else {
                        let _ = crate::macro_engine::clear_binding(row.key_id);
                    }
                }
            }
        });
    }

    // ---- 颜色比对与 UI 回显格式化 ----

    fn find_matching_color_idx(rgb: [u8; 3]) -> Option<usize> {
        for (i, (_name, hex)) in crate::menubar::LED_COLORS.iter().enumerate() {
            if crate::menubar::hex_rgb(hex) == rgb {
                return Some(i);
            }
        }
        None
    }

    fn update_color_ui(
        color_buttons: &[Retained<NSButton>],
        color_label: &NSTextField,
        rgb: [u8; 3],
        effect_idx: usize,
    ) {
        let matching_idx = Self::find_matching_color_idx(rgb);
        let color_enabled = effect_idx != 0 && effect_idx != 2;

        for (i, btn) in color_buttons.iter().enumerate() {
            let mtm = MainThreadMarker::from(&**btn);
            let btn_rgb = crate::menubar::hex_rgb(crate::menubar::LED_COLORS[i].1);
            let is_selected = Some(i) == matching_idx && color_enabled;
            let swatch = unsafe { Self::create_swatch_image(btn_rgb, is_selected, mtm) };
            unsafe {
                btn.setImage(Some(&swatch));
                btn.setEnabled(color_enabled);
            }
        }

        let text = if effect_idx == 0 {
            "灯效已关闭".to_string()
        } else if effect_idx == 2 {
            "彩色循环 (色彩自动过渡)".to_string()
        } else {
            match matching_idx {
                Some(i) => {
                    let (name, hex) = crate::menubar::LED_COLORS[i];
                    format!("选定: {name} · #{hex}")
                }
                None => {
                    format!("选定: 自定义 · #{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2])
                }
            }
        };
        unsafe {
            color_label.setStringValue(&NSString::from_str(&text));
            color_label.setTextColor(Some(&Self::color_secondary_text()));
        }
    }

    fn format_rate_label(period_ms: u16, effect_idx: usize) -> String {
        if effect_idx == 0 || effect_idx == 1 {
            return "不适用".into();
        }
        let sec = period_ms as f32 / 1000.0;
        let desc = if period_ms <= 1500 {
            "快"
        } else if period_ms <= 3000 {
            "适中"
        } else if period_ms <= 6000 {
            "舒缓"
        } else {
            "慢速"
        };
        format!("{sec:.1}s ({desc})")
    }

    // ---- 状态提取与合流下发核心逻辑 ----

    fn collect_and_schedule(h: &PanelHolder) {
        let zone_seg = unsafe { h.zone_control.selectedSegment() } as usize;
        let target = Self::current_target_zone(zone_seg);
        let effect_seg = unsafe { h.effect_control.selectedSegment() } as usize;
        let brightness = unsafe { h.brightness_slider.doubleValue() } as u8;
        let rate_raw = unsafe { h.rate_slider.doubleValue() } as u16;
        let rate_ms = if rate_raw == 0 {
            2000
        } else {
            let rounded = ((rate_raw + 50) / 100) * 100;
            rounded.clamp(1000, 10000)
        };
        let rgb = h.active_rgb.get();

        let mut spec =
            crate::config::LedSpec::new("breathing", rgb, brightness, Some(&rate_ms.to_string()));

        match effect_seg {
            0 => spec.turn_off(),
            1 => {
                spec.turn_on();
                spec.effect = "solid".into();
            }
            2 => {
                spec.turn_on();
                spec.effect = "cycle".into();
            }
            3 => {
                spec.turn_on();
                spec.effect = "breathing".into();
            }
            _ => {}
        }

        controller::schedule_live_led(target, spec);
    }

    /// 依据当前控件状态刷新灯效预览视图
    fn update_led_preview(h: &PanelHolder) {
        let mode_idx = unsafe { h.effect_control.selectedSegment() } as usize;
        let zone_idx = unsafe { h.zone_control.selectedSegment() } as usize;
        let rgb = h.active_rgb.get();
        let brightness = unsafe { h.brightness_slider.doubleValue() } as u8;
        let rate_raw = unsafe { h.rate_slider.doubleValue() } as u16;
        let period = if rate_raw == 0 {
            2000
        } else {
            rate_raw.clamp(1000, 10000)
        };
        h.led_preview
            .set_state(mode_idx, rgb, brightness, period, zone_idx);
    }

    /// 同步 DPI 瓷砖选中态
    fn update_dpi_tiles(h: &PanelHolder, active_dpi: u16) {
        for tile in &h.dpi_tiles {
            tile.set_selected(tile.dpi_value() == active_dpi);
        }
    }

    /// 同步电量胶囊 (图标 + 百分比)
    fn sync_battery_ui(h: &PanelHolder, battery_text: &str) {
        let (percent, charging) = battery_meta(battery_text);
        let label_text = match percent {
            Some(p) => format!("{p}%"),
            None => "未连接".to_string(),
        };
        let (symbol, tint_rgb) = battery_icon_spec(percent, charging);
        unsafe {
            let tint = NSColor::colorWithSRGBRed_green_blue_alpha(
                tint_rgb[0],
                tint_rgb[1],
                tint_rgb[2],
                1.0,
            );
            h.battery_label
                .setStringValue(&NSString::from_str(&label_text));
            h.battery_label
                .setTextColor(Some(&Self::color_primary_text()));
            h.battery_icon
                .setImage(Some(&tinted_symbol(symbol, 22.0, &tint)));
        }
    }

    // ---- 事件分发逻辑 ----

    fn dispatch_dpi_slider(t: f64) {
        HOLDER.with(|cell| {
            if let Some(h) = cell.borrow().as_ref() {
                let dpi = Self::slider_to_dpi(t);
                h.active_dpi.set(dpi);
                unsafe {
                    h.dpi_value_label
                        .setStringValue(&NSString::from_str(&format!("{} DPI", format_dpi(dpi))));
                }
                Self::update_dpi_tiles(h, dpi);
                controller::schedule_live_dpi(dpi);
            }
        });
    }

    pub(crate) fn dispatch_dpi_preset(dpi: u16) {
        HOLDER.with(|cell| {
            if let Some(h) = cell.borrow().as_ref() {
                h.active_dpi.set(dpi);
                unsafe {
                    h.dpi_value_label
                        .setStringValue(&NSString::from_str(&format!("{} DPI", format_dpi(dpi))));
                    h.dpi_slider.setDoubleValue(Self::dpi_to_slider(dpi));
                }
                Self::update_dpi_tiles(h, dpi);
                controller::schedule_live_dpi(dpi);
            }
        });
    }

    fn current_target_zone(zone_seg: usize) -> TargetZone {
        match zone_seg {
            1 => TargetZone::Single(led::ZONE_PRIMARY),
            2 => TargetZone::Single(led::ZONE_LOGO),
            _ => TargetZone::All,
        }
    }

    fn current_spec(zone_seg: usize) -> crate::config::LedSpec {
        let zone_key = match zone_seg {
            2 => "logo",
            _ => "primary",
        };
        if let Some(spec) = controller::get_live_spec_for_zone(zone_key) {
            return spec;
        }
        let cfg = crate::config::load().unwrap_or_default();
        match zone_seg {
            2 => cfg.led_zones.get("logo").cloned().unwrap_or_else(|| {
                crate::config::LedSpec::new("breathing", [0, 200, 255], 100, Some("2000"))
            }),
            _ => cfg.led_zones.get("primary").cloned().unwrap_or_else(|| {
                crate::config::LedSpec::new("breathing", [255, 204, 0], 100, Some("2000"))
            }),
        }
    }

    pub fn dispatch_zone(zone_idx: usize) {
        HOLDER.with(|cell| {
            if let Some(h) = cell.borrow().as_ref() {
                unsafe {
                    h.zone_control.setSelectedSegment(zone_idx as isize);
                }
                let key = match zone_idx {
                    1 => Some("主要灯带".to_string()),
                    2 => Some("G 标志".to_string()),
                    _ => None,
                };
                h.mouse_canvas.set_selected_key(key);
            }
        });
        Self::sync_ui_from_config_for_zone(zone_idx);
    }

    fn dispatch_effect(effect_seg: usize) {
        HOLDER.with(|cell| {
            if let Some(h) = cell.borrow().as_ref() {
                let rgb = h.active_rgb.get();
                Self::update_color_ui(&h.color_buttons, &h.color_label, rgb, effect_seg);

                let rate_raw = unsafe { h.rate_slider.doubleValue() } as u16;
                let rate_ms = if rate_raw == 0 {
                    2000
                } else {
                    rate_raw.clamp(1000, 10000)
                };
                unsafe {
                    h.rate_slider.setEnabled(effect_seg == 2 || effect_seg == 3);
                    h.rate_label
                        .setStringValue(&NSString::from_str(&Self::format_rate_label(
                            rate_ms, effect_seg,
                        )));
                }

                Self::update_led_preview(h);
                Self::collect_and_schedule(h);
            }
        });
    }

    fn dispatch_color(color_idx: usize) {
        if let Some((_name, hex)) = crate::menubar::LED_COLORS.get(color_idx) {
            let rgb = crate::menubar::hex_rgb(hex);
            HOLDER.with(|cell| {
                if let Some(h) = cell.borrow().as_ref() {
                    h.active_rgb.set(rgb);

                    let mut effect_seg = unsafe { h.effect_control.selectedSegment() } as usize;
                    if effect_seg == 0 || effect_seg == 2 {
                        effect_seg = 3; // 切换到呼吸
                        unsafe {
                            h.effect_control.setSelectedSegment(3);
                        }
                    }

                    Self::update_color_ui(&h.color_buttons, &h.color_label, rgb, effect_seg);

                    let rate_raw = unsafe { h.rate_slider.doubleValue() } as u16;
                    let rate_ms = if rate_raw == 0 {
                        2000
                    } else {
                        rate_raw.clamp(1000, 10000)
                    };
                    unsafe {
                        h.rate_slider.setEnabled(effect_seg == 2 || effect_seg == 3);
                        h.rate_label
                            .setStringValue(&NSString::from_str(&Self::format_rate_label(
                                rate_ms, effect_seg,
                            )));
                    }

                    Self::update_led_preview(h);
                    Self::collect_and_schedule(h);
                }
            });
        }
    }

    fn dispatch_brightness(val: u8) {
        HOLDER.with(|cell| {
            if let Some(h) = cell.borrow().as_ref() {
                unsafe {
                    h.brightness_label
                        .setStringValue(&NSString::from_str(&format!("{val}%")));
                    h.brightness_label.setTextColor(Some(&Self::color_accent()));
                }

                Self::update_led_preview(h);
                Self::collect_and_schedule(h);
            }
        });
    }

    fn dispatch_rate(val: u16) {
        HOLDER.with(|cell| {
            if let Some(h) = cell.borrow().as_ref() {
                let rounded = ((val + 50) / 100) * 100;
                let rate_ms = rounded.clamp(1000, 10000);
                let effect_seg = unsafe { h.effect_control.selectedSegment() } as usize;
                unsafe {
                    h.rate_label
                        .setStringValue(&NSString::from_str(&Self::format_rate_label(
                            rate_ms, effect_seg,
                        )));
                    h.rate_label.setTextColor(Some(&Self::color_accent()));
                }

                Self::update_led_preview(h);
                Self::collect_and_schedule(h);
            }
        });
    }

    pub fn sync_status(battery_text: &str, mode_text: &str, dpi: Option<u16>) {
        HOLDER.with(|cell| {
            if let Some(h) = cell.borrow().as_ref() {
                unsafe {
                    Self::sync_battery_ui(h, battery_text);
                    h.mode_label.setStringValue(&NSString::from_str(mode_text));
                    h.mode_label
                        .setTextColor(Some(&Self::color_secondary_text()));
                    if let Some(d) = dpi {
                        let clamped = d.clamp(100, 25600);
                        h.active_dpi.set(clamped);
                        h.dpi_value_label
                            .setStringValue(&NSString::from_str(&format!(
                                "{} DPI",
                                format_dpi(clamped)
                            )));
                        h.dpi_value_label.setTextColor(Some(&Self::color_accent()));
                        h.dpi_slider.setDoubleValue(Self::dpi_to_slider(clamped));
                        Self::update_dpi_tiles(h, clamped);
                    }
                }
            }
        });
        Self::sync_macro_ui();
    }

    pub fn sync_ui_from_config() {
        let zone_seg = HOLDER.with(|cell| {
            cell.borrow()
                .as_ref()
                .map(|h| unsafe { h.zone_control.selectedSegment() } as usize)
                .unwrap_or(0)
        });
        Self::sync_ui_from_config_for_zone(zone_seg);
    }

    pub fn sync_ui_from_config_for_zone(zone_seg: usize) {
        HOLDER.with(|cell| {
            if let Some(h) = cell.borrow().as_ref() {
                // 同步 DPI 状态
                let live_dpi = controller::get_live_dpi()
                    .or_else(|| crate::config::load().ok().and_then(|c| c.desired_dpi))
                    .unwrap_or(1600);
                let clamped_dpi = live_dpi.clamp(100, 25600);
                h.active_dpi.set(clamped_dpi);
                unsafe {
                    h.dpi_value_label
                        .setStringValue(&NSString::from_str(&format!(
                            "{} DPI",
                            format_dpi(clamped_dpi)
                        )));
                    h.dpi_value_label.setTextColor(Some(&Self::color_accent()));
                    h.dpi_slider
                        .setDoubleValue(Self::dpi_to_slider(clamped_dpi));
                }
                Self::update_dpi_tiles(h, clamped_dpi);

                let spec = Self::current_spec(zone_seg);
                h.active_rgb.set(spec.rgb);

                unsafe {
                    h.zone_control.setSelectedSegment(zone_seg as isize);

                    let effect_idx: usize = if spec.off {
                        0
                    } else {
                        match spec.effect.as_str() {
                            "solid" => 1,
                            "cycle" => 2,
                            _ => 3, // breathing
                        }
                    };
                    h.effect_control.setSelectedSegment(effect_idx as isize);

                    Self::update_color_ui(&h.color_buttons, &h.color_label, spec.rgb, effect_idx);

                    h.brightness_slider.setDoubleValue(spec.brightness as f64);
                    h.brightness_label
                        .setStringValue(&NSString::from_str(&format!("{}%", spec.brightness)));
                    h.brightness_label.setTextColor(Some(&Self::color_accent()));

                    let raw_period = led::rate_period_ms(spec.rate.as_deref()).unwrap_or(2000);
                    let period = if raw_period == 0 {
                        2000
                    } else {
                        raw_period.clamp(1000, 10000)
                    };
                    h.rate_slider.setDoubleValue(period as f64);
                    h.rate_slider.setEnabled(effect_idx == 2 || effect_idx == 3);
                    h.rate_label
                        .setStringValue(&NSString::from_str(&Self::format_rate_label(
                            period, effect_idx,
                        )));
                    h.rate_label.setTextColor(Some(&Self::color_accent()));
                }

                Self::update_led_preview(h);
            }
        });
        Self::sync_macro_ui();
    }

    pub fn sync_macro_ui() {
        let clicked_key = HOLDER.with(|cell| {
            cell.borrow()
                .as_ref()
                .and_then(|h| h.mouse_canvas.take_clicked_key())
        });
        if let Some(name) = clicked_key {
            if let Some(gk) = crate::macro_engine::G_KEYS
                .iter()
                .find(|gk| gk.name == name)
            {
                Self::select_gkey(gk.id, true);
            }
        }

        HOLDER.with(|cell| {
            if let Some(h) = cell.borrow().as_ref() {
                let running = crate::macro_engine::is_tap_running();
                let phase = crate::macro_engine::recording_phase();
                let is_recording = !matches!(phase, crate::macro_engine::RecordingPhase::Idle);
                let mut bound_keys = HashSet::new();

                unsafe {
                    h.macro_switch.setState(if running {
                        objc2_app_kit::NSControlStateValueOn
                    } else {
                        objc2_app_kit::NSControlStateValueOff
                    });
                    let dot_color = if is_recording {
                        Self::color_recording_red()
                    } else if running {
                        Self::color_success_green()
                    } else {
                        Self::color_idle_gray()
                    };
                    h.macro_status_dot.setFillColor(&dot_color);

                    for row in &h.gkey_rows {
                        let binding = crate::macro_engine::get_binding_for_key(row.key_id);
                        if Self::active_editor(&row.text_input).is_some()
                            && binding
                                .as_ref()
                                .is_none_or(|b| crate::macro_engine::get_binding_text(b).is_some())
                        {
                            continue;
                        }
                        if let Some(b) = binding {
                            if let Some(name) = Self::gkey_name_for_id(row.key_id) {
                                bound_keys.insert(name.to_string());
                            }
                            if let Some(text) = crate::macro_engine::get_binding_text(&b) {
                                row.summary_label
                                    .setStringValue(&NSString::from_str("文字 · 自动输入"));
                                row.summary_label.setTextColor(Some(&Self::color_accent()));
                                row.text_input.setEditable(true);
                                row.text_input.setStringValue(&NSString::from_str(&text));
                                row.text_input
                                    .setPlaceholderString(Some(&NSString::from_str(
                                        "输入自动打字文本...",
                                    )));
                                row.text_input
                                    .setTextColor(Some(&Self::color_primary_text()));
                            } else {
                                let summary = crate::macro_engine::format_binding_summary(&b);
                                row.summary_label
                                    .setStringValue(&NSString::from_str(&summary));
                                row.summary_label
                                    .setTextColor(Some(&Self::color_success_green()));
                                let recorded_keys = crate::macro_engine::format_recorded_keys(&b);
                                row.text_input.setEditable(false);
                                row.text_input.setStringValue(&NSString::from_str(
                                    recorded_keys.as_deref().unwrap_or(""),
                                ));
                                row.text_input
                                    .setPlaceholderString(Some(&NSString::from_str(
                                        "非文字动作；清空后可输入文字",
                                    )));
                                let text_color = if recorded_keys.is_some() {
                                    Self::color_primary_text()
                                } else {
                                    Self::color_secondary_text()
                                };
                                row.text_input.setTextColor(Some(&text_color));
                            }
                            row.clear_btn.setEnabled(true);
                        } else {
                            row.summary_label
                                .setStringValue(&NSString::from_str("未绑定"));
                            row.summary_label
                                .setTextColor(Some(&Self::color_secondary_text()));
                            row.text_input.setEditable(true);
                            row.text_input.setStringValue(&NSString::from_str(""));
                            row.text_input
                                .setPlaceholderString(Some(&NSString::from_str(
                                    "输入自动打字文本...",
                                )));
                            row.text_input
                                .setTextColor(Some(&Self::color_primary_text()));
                            row.clear_btn.setEnabled(false);
                        }
                        row.record_btn.setEnabled(!is_recording);
                    }
                    h.mouse_canvas.set_bound_keys(bound_keys);

                    h.macro_rec_seq_btn.setEnabled(!is_recording);
                    h.macro_rec_cancel_btn.setEnabled(is_recording);

                    let is_sequence =
                        matches!(phase, crate::macro_engine::RecordingPhase::Sequence { .. });
                    h.macro_rec_finish_btn.setEnabled(is_sequence);

                    let (status_text, is_warn) = match &phase {
                        crate::macro_engine::RecordingPhase::Idle => {
                            if running {
                                ("就绪 · 支持组合键与指定文字".to_string(), false)
                            } else {
                                ("已停用 · 开启开关以启用按键拦截".to_string(), false)
                            }
                        }
                        crate::macro_engine::RecordingPhase::AwaitMouse(
                            crate::macro_engine::RecordingKind::Shortcut,
                        ) => ("请按目标侧键 (G4..G11)...".to_string(), true),
                        crate::macro_engine::RecordingPhase::AwaitMouse(
                            crate::macro_engine::RecordingKind::Sequence,
                        ) => ("请按目标侧键开始录制序列...".to_string(), true),
                        crate::macro_engine::RecordingPhase::Shortcut { button } => {
                            let name = crate::macro_engine::get_gkey_by_button(*button)
                                .map(|gk| format!("{}({})", gk.name, gk.desc))
                                .unwrap_or_else(|| format!("button{button}"));
                            (format!("正在录制 {name}: 请按一次键盘快捷键..."), true)
                        }
                        crate::macro_engine::RecordingPhase::Sequence { button, events } => {
                            let name = crate::macro_engine::get_gkey_by_button(*button)
                                .map(|gk| format!("{}({})", gk.name, gk.desc))
                                .unwrap_or_else(|| format!("button{button}"));
                            (format!("录制中 {name}: 已捕获 {events} 个按键事件"), true)
                        }
                    };

                    h.macro_status_label
                        .setStringValue(&NSString::from_str(&status_text));
                    if is_warn {
                        h.macro_status_label
                            .setTextColor(Some(&Self::color_recording_red()));
                    } else {
                        h.macro_status_label
                            .setTextColor(Some(&Self::color_secondary_text()));
                    }
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dpi_slider_mapping_endpoints_and_presets() {
        assert_eq!(PopoverPanel::slider_to_dpi(0.0), 100);
        assert_eq!(PopoverPanel::slider_to_dpi(0.25), 400);
        assert_eq!(PopoverPanel::slider_to_dpi(0.375), 800);
        assert_eq!(PopoverPanel::slider_to_dpi(0.50), 1600);
        assert_eq!(PopoverPanel::slider_to_dpi(0.625), 3200);
        assert_eq!(PopoverPanel::slider_to_dpi(0.75), 6400);
        assert_eq!(PopoverPanel::slider_to_dpi(1.0), 25600);

        for &preset in PopoverPanel::DPI_PRESETS {
            let t = PopoverPanel::dpi_to_slider(preset);
            assert_eq!(PopoverPanel::slider_to_dpi(t), preset);
        }
    }

    #[test]
    fn test_dpi_slider_step_and_bounds() {
        for step in 0..=1000 {
            let t = step as f64 / 1000.0;
            let dpi = PopoverPanel::slider_to_dpi(t);
            assert!(dpi >= 100 && dpi <= 25600);
            assert_eq!(dpi % 50, 0);
        }
    }

    #[test]
    fn test_format_dpi_grouping() {
        assert_eq!(format_dpi(100), "100");
        assert_eq!(format_dpi(1600), "1,600");
        assert_eq!(format_dpi(3200), "3,200");
        assert_eq!(format_dpi(25600), "25,600");
    }

    #[test]
    fn test_battery_meta_parsing() {
        assert_eq!(battery_meta("🔋 85%"), (Some(85), false));
        assert_eq!(battery_meta("⚡️ 71%"), (Some(71), true));
        assert_eq!(battery_meta("🔋 9%"), (Some(9), false));
        assert_eq!(battery_meta("🔋 未连接"), (None, false));
        assert_eq!(battery_meta("🔋 --%"), (None, false));
    }

    #[test]
    fn test_battery_icon_spec() {
        assert_eq!(battery_icon_spec(Some(85), false).0, "battery.100");
        assert_eq!(battery_icon_spec(Some(40), false).0, "battery.75");
        assert_eq!(battery_icon_spec(Some(20), false).0, "battery.50");
        assert_eq!(battery_icon_spec(Some(8), false).0, "battery.25");
        assert_eq!(battery_icon_spec(Some(0), false).0, "battery.0");
        assert_eq!(battery_icon_spec(None, false).0, "battery.0");
        let (sym, rgb) = battery_icon_spec(Some(50), true);
        assert_eq!(sym, "bolt.fill");
        assert!(rgb[1] > 0.5); // 绿色系
    }

    #[test]
    fn test_hsv_to_rgb_primaries() {
        assert_eq!(hsv_to_rgb(0.0, 1.0, 1.0), [255, 0, 0]);
        assert_eq!(hsv_to_rgb(120.0, 1.0, 1.0), [0, 255, 0]);
        assert_eq!(hsv_to_rgb(240.0, 1.0, 1.0), [0, 0, 255]);
        assert_eq!(hsv_to_rgb(60.0, 1.0, 1.0), [255, 255, 0]);
        assert_eq!(hsv_to_rgb(0.0, 0.0, 0.5), [128, 128, 128]);
    }
}
