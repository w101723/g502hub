//! G502 鼠标按键布局高精实机画布 (Logitech G HUB 原生电竞实机渲染)
//!
//! 采用 G502 LIGHTSPEED / HERO 原生实机俯视与侧视高精渲染，
//! 结合硬件级发光按键热点（Hotspots）、折线引出线（Leader Lines）与电竞药丸徽章（Badges），
//! 支持鼠标悬停高亮、按键选中、已绑定宏状态提示以及精准命中测试。

use objc2::mutability::MainThreadOnly;
use objc2::rc::{Allocated, Retained};
use objc2::{declare_class, msg_send, msg_send_id, ClassType, DeclaredClass};
use objc2_app_kit::{
    NSBezierPath, NSColor, NSCursor, NSEvent, NSFont, NSFontAttributeName,
    NSForegroundColorAttributeName, NSImage, NSTrackingArea, NSTrackingAreaOptions, NSView,
};
use objc2_foundation::{
    MainThreadMarker, NSData, NSMutableDictionary, NSPoint, NSRect, NSSize, NSString,
};
use std::cell::{Cell, RefCell};
use std::collections::HashSet;

const TOP_PNG: &[u8] = include_bytes!("../assets/g502_top.png");
const SIDE_PNG: &[u8] = include_bytes!("../assets/g502_side.png");

thread_local! {
    static TOP_IMAGE: RefCell<Option<Retained<NSImage>>> = const { RefCell::new(None) };
    static SIDE_IMAGE: RefCell<Option<Retained<NSImage>>> = const { RefCell::new(None) };
}

fn with_top_image<R>(f: impl FnOnce(&NSImage) -> R) -> R {
    TOP_IMAGE.with(|cell| {
        let mut opt = cell.borrow_mut();
        if opt.is_none() {
            let mtm = MainThreadMarker::new().expect("must be on main thread");
            let data = NSData::with_bytes(TOP_PNG);
            let img = NSImage::initWithData(mtm.alloc(), &data).expect("load top png");
            *opt = Some(img);
        }
        f(opt.as_ref().unwrap())
    })
}

fn with_side_image<R>(f: impl FnOnce(&NSImage) -> R) -> R {
    SIDE_IMAGE.with(|cell| {
        let mut opt = cell.borrow_mut();
        if opt.is_none() {
            let mtm = MainThreadMarker::new().expect("must be on main thread");
            let data = NSData::with_bytes(SIDE_PNG);
            let img = NSImage::initWithData(mtm.alloc(), &data).expect("load side png");
            *opt = Some(img);
        }
        f(opt.as_ref().unwrap())
    })
}

/// 视角模式: 俯视图(G7-G11) 或 侧视图(G4-G6)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CanvasMode {
    Top,
    Side,
}

impl Default for CanvasMode {
    fn default() -> Self {
        CanvasMode::Top
    }
}

/// 按键区域与指示热点配置
#[derive(Debug, Clone)]
pub struct ZonePolygon {
    pub key: &'static str,
    pub desc: &'static str,
    pub points: &'static [(f64, f64)],
    pub hotspot: (f64, f64),
    pub badge_left: bool,
    pub badge_y_ratio: f64,
    #[allow(dead_code)]
    pub label_pos: (f64, f64),
    #[allow(dead_code)]
    pub leader_start: (f64, f64),
    #[allow(dead_code)]
    pub leader_end: (f64, f64),
}

/// 判断归一化点 (px, py) 是否在多边形 poly 内部 (Ray casting 算法)
pub fn point_in_polygon(px: f64, py: f64, poly: &[(f64, f64)]) -> bool {
    if poly.len() < 3 {
        return false;
    }
    let mut inside = false;
    let n = poly.len();
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = poly[i];
        let (xj, yj) = poly[j];
        let intersect = ((yi > py) != (yj > py)) && (px < (xj - xi) * (py - yi) / (yj - yi) + xi);
        if intersect {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// 计算等比居中的鼠标绘制视口 (保持 G502 真实硬件高精比例)
pub fn calc_viewport(w: f64, h: f64, mode: CanvasMode) -> (f64, f64, f64, f64) {
    let target_ratio = match mode {
        CanvasMode::Top => 850.0 / 872.0,  // ~0.9748
        CanvasMode::Side => 658.0 / 854.0, // ~0.7705
    };
    let dh = (h * 0.90).min(h - 14.0);
    let dw = dh * target_ratio;
    let ox = match mode {
        CanvasMode::Top => (w - dw) * 0.5,
        CanvasMode::Side => (w * 0.53 - dw * 0.5).clamp(10.0, w - dw - 10.0),
    };
    let oy = (h - dh) * 0.5;
    (dw, dh, ox, oy)
}

/// 顶部视角按键热区 (俯视)
pub const TOP_KEY_ZONES: &[ZonePolygon] = &[
    // G8: 左键外缘前方 (DPI +)
    ZonePolygon {
        key: "G8",
        desc: "DPI+",
        points: &[(0.24, 0.77), (0.35, 0.77), (0.35, 0.88), (0.24, 0.88)],
        hotspot: (0.368, 0.693),
        badge_left: true,
        badge_y_ratio: 0.62,
        label_pos: (0.04, 0.86),
        leader_start: (0.24, 0.83),
        leader_end: (0.16, 0.86),
    },
    // G7: 左键外缘后方 (DPI -)
    ZonePolygon {
        key: "G7",
        desc: "DPI-",
        points: &[(0.24, 0.63), (0.35, 0.63), (0.35, 0.74), (0.24, 0.74)],
        hotspot: (0.367, 0.618),
        badge_left: true,
        badge_y_ratio: 0.30,
        label_pos: (0.04, 0.67),
        leader_start: (0.24, 0.68),
        leader_end: (0.16, 0.67),
    },
    // G10: 滚轮向左摆动
    ZonePolygon {
        key: "G10",
        desc: "滚轮左",
        points: &[(0.40, 0.72), (0.47, 0.72), (0.47, 0.87), (0.40, 0.87)],
        hotspot: (0.502, 0.658),
        badge_left: true,
        badge_y_ratio: 0.88,
        label_pos: (0.12, 0.94),
        leader_start: (0.42, 0.82),
        leader_end: (0.24, 0.94),
    },
    // G11: 滚轮向右摆动
    ZonePolygon {
        key: "G11",
        desc: "滚轮右",
        points: &[(0.53, 0.72), (0.60, 0.72), (0.60, 0.87), (0.53, 0.87)],
        hotspot: (0.568, 0.658),
        badge_left: false,
        badge_y_ratio: 0.88,
        label_pos: (0.78, 0.94),
        leader_start: (0.58, 0.82),
        leader_end: (0.76, 0.94),
    },
    // G9: 滚轮后方按键 (默认电量/配置切换)
    ZonePolygon {
        key: "G9",
        desc: "⚡️电量",
        points: &[(0.44, 0.54), (0.56, 0.54), (0.56, 0.64), (0.44, 0.64)],
        hotspot: (0.541, 0.481),
        badge_left: false,
        badge_y_ratio: 0.48,
        label_pos: (0.78, 0.60),
        leader_start: (0.56, 0.59),
        leader_end: (0.76, 0.60),
    },
];

/// 侧面视角按键热区 (左侧视)
pub const SIDE_KEY_ZONES: &[ZonePolygon] = &[
    // G5: 侧键前进
    ZonePolygon {
        key: "G5",
        desc: "前进",
        points: &[(0.45, 0.54), (0.59, 0.54), (0.59, 0.67), (0.45, 0.67)],
        hotspot: (0.444, 0.528),
        badge_left: true,
        badge_y_ratio: 0.78,
        label_pos: (0.50, 0.88),
        leader_start: (0.52, 0.67),
        leader_end: (0.54, 0.86),
    },
    // G4: 侧键后退
    ZonePolygon {
        key: "G4",
        desc: "后退",
        points: &[(0.30, 0.54), (0.44, 0.54), (0.44, 0.67), (0.30, 0.67)],
        hotspot: (0.480, 0.394),
        badge_left: true,
        badge_y_ratio: 0.50,
        label_pos: (0.14, 0.76),
        leader_start: (0.36, 0.65),
        leader_end: (0.24, 0.76),
    },
    // G6: 瞄准键 / DPI Shift (最前端拇指键)
    ZonePolygon {
        key: "G6",
        desc: "瞄准键",
        points: &[(0.57, 0.39), (0.70, 0.39), (0.70, 0.52), (0.57, 0.52)],
        hotspot: (0.345, 0.577),
        badge_left: true,
        badge_y_ratio: 0.22,
        label_pos: (0.76, 0.66),
        leader_start: (0.66, 0.50),
        leader_end: (0.74, 0.66),
    },
];

/// 顶部视角灯效分区热区 (LIGHTSYNC 分区模式)
pub const LIGHTING_ZONES: &[ZonePolygon] = &[
    // 主要灯带: 鼠标左肩部 3 段式 LED 指示灯带
    ZonePolygon {
        key: "主要灯带",
        desc: "侧边条",
        points: &[(0.30, 0.48), (0.42, 0.48), (0.42, 0.66), (0.30, 0.66)],
        hotspot: (0.355, 0.575),
        badge_left: true,
        badge_y_ratio: 0.58,
        label_pos: (0.04, 0.58),
        leader_start: (0.30, 0.58),
        leader_end: (0.16, 0.58),
    },
    // G 标志: 鼠标掌托罗技 G 发光徽标
    ZonePolygon {
        key: "G 标志",
        desc: "掌托",
        points: &[(0.45, 0.20), (0.62, 0.20), (0.62, 0.38), (0.45, 0.38)],
        hotspot: (0.530, 0.285),
        badge_left: false,
        badge_y_ratio: 0.30,
        label_pos: (0.78, 0.30),
        leader_start: (0.58, 0.29),
        leader_end: (0.76, 0.30),
    },
];

/// 根据当前模式和归一化坐标测试命中的按键
#[allow(dead_code)]
pub fn hit_test_key(mode: CanvasMode, nx: f64, ny: f64) -> Option<&'static str> {
    let zones = match mode {
        CanvasMode::Top => TOP_KEY_ZONES,
        CanvasMode::Side => SIDE_KEY_ZONES,
    };
    for z in zones {
        if point_in_polygon(nx, ny, z.points) {
            return Some(z.key);
        }
    }
    None
}

/// 画布内部状态
pub struct MouseCanvasIvars {
    pub mode: RefCell<CanvasMode>,
    pub hovered_key: RefCell<Option<String>>,
    pub selected_key: RefCell<Option<String>>,
    pub bound_keys: RefCell<HashSet<String>>,
    pub clicked_key: RefCell<Option<String>>,
    pub tracking_area: RefCell<Option<Retained<NSTrackingArea>>>,
    pub tab_idx: Cell<usize>,
}

impl Default for MouseCanvasIvars {
    fn default() -> Self {
        Self {
            mode: RefCell::new(CanvasMode::Top),
            hovered_key: RefCell::new(None),
            selected_key: RefCell::new(None),
            bound_keys: RefCell::new(HashSet::new()),
            clicked_key: RefCell::new(None),
            tracking_area: RefCell::new(None),
            tab_idx: Cell::new(0),
        }
    }
}

declare_class!(
    pub struct G502MouseCanvas;

    unsafe impl ClassType for G502MouseCanvas {
        type Super = NSView;
        type Mutability = MainThreadOnly;
        const NAME: &'static str = "G502MouseCanvas";
    }

    impl DeclaredClass for G502MouseCanvas {
        type Ivars = MouseCanvasIvars;
    }

    unsafe impl G502MouseCanvas {
        #[method_id(initWithFrame:)]
        fn init_with_frame(this: Allocated<Self>, frame: NSRect) -> Option<Retained<Self>> {
            let this = this.set_ivars(MouseCanvasIvars::default());
            unsafe { msg_send_id![super(this), initWithFrame: frame] }
        }

        #[method(updateTrackingAreas)]
        fn update_tracking_areas(&self) {
            let ivars = self.ivars();
            if let Some(old_area) = ivars.tracking_area.borrow_mut().take() {
                unsafe { self.removeTrackingArea(&old_area) };
            }
            let bounds = self.bounds();
            let opts = NSTrackingAreaOptions::NSTrackingMouseMoved
                | NSTrackingAreaOptions::NSTrackingMouseEnteredAndExited
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

        #[method(mouseMoved:)]
        fn mouse_moved(&self, event: &NSEvent) {
            let win_pt = unsafe { event.locationInWindow() };
            let local_pt = self.convertPoint_fromView(win_pt, None);
            let bounds = self.bounds();
            let w = bounds.size.width;
            let h = bounds.size.height;
            if w <= 0.0 || h <= 0.0 {
                return;
            }
            let mode = *self.ivars().mode.borrow();
            let tab_idx = self.ivars().tab_idx.get();
            let hit = Self::resolve_hit(mode, w, h, local_pt, tab_idx).map(|s| s.to_string());

            let mut hovered = self.ivars().hovered_key.borrow_mut();
            if *hovered != hit {
                *hovered = hit.clone();
                drop(hovered);
                if hit.is_some() {
                    unsafe { NSCursor::pointingHandCursor().set() };
                } else {
                    unsafe { NSCursor::arrowCursor().set() };
                }
                unsafe { self.setNeedsDisplay(true) };
            }
        }

        #[method(mouseExited:)]
        fn mouse_exited(&self, _event: &NSEvent) {
            let mut hovered = self.ivars().hovered_key.borrow_mut();
            if hovered.is_some() {
                *hovered = None;
                drop(hovered);
                unsafe {
                    NSCursor::arrowCursor().set();
                    self.setNeedsDisplay(true);
                }
            }
        }

        #[method(mouseDown:)]
        fn mouse_down(&self, event: &NSEvent) {
            let win_pt = unsafe { event.locationInWindow() };
            let local_pt = self.convertPoint_fromView(win_pt, None);
            let bounds = self.bounds();
            let w = bounds.size.width;
            let h = bounds.size.height;
            if w <= 0.0 || h <= 0.0 {
                return;
            }
            let mode = *self.ivars().mode.borrow();
            let tab_idx = self.ivars().tab_idx.get();
            if let Some(key) = Self::resolve_hit(mode, w, h, local_pt, tab_idx) {
                let key_str = key.to_string();
                *self.ivars().selected_key.borrow_mut() = Some(key_str.clone());
                *self.ivars().clicked_key.borrow_mut() = Some(key_str);
                unsafe { self.setNeedsDisplay(true) };
                if tab_idx == 1 {
                    match key {
                        "主要灯带" => crate::panel::PopoverPanel::dispatch_zone(1),
                        "G 标志" => crate::panel::PopoverPanel::dispatch_zone(2),
                        _ => crate::panel::PopoverPanel::dispatch_zone(0),
                    }
                } else if let Some(gk) = crate::macro_engine::G_KEYS.iter().find(|gk| gk.name == key) {
                    crate::panel::PopoverPanel::select_gkey(gk.id, true);
                }
            } else if tab_idx == 1 {
                *self.ivars().selected_key.borrow_mut() = None;
                unsafe { self.setNeedsDisplay(true) };
                crate::panel::PopoverPanel::dispatch_zone(0);
            }
        }

        #[method(drawRect:)]
        fn draw_rect(&self, dirty_rect: NSRect) {
            let _ = dirty_rect;
            let bounds = self.bounds();
            let w = bounds.size.width;
            let h = bounds.size.height;
            if w <= 10.0 || h <= 10.0 {
                return;
            }

            let mode = *self.ivars().mode.borrow();
            let hovered = self.ivars().hovered_key.borrow().clone();
            let selected = self.ivars().selected_key.borrow().clone();
            let bound_set = self.ivars().bound_keys.borrow().clone();

            unsafe {
                // 1. G HUB 原生深空炭黑画布底板 (#131417) 与精致微光切角微边框 (#26292E)
                let bg_path =
                    NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(bounds, 10.0, 10.0);
                let bg_color =
                    NSColor::colorWithSRGBRed_green_blue_alpha(0.075, 0.078, 0.090, 1.0);
                bg_color.setFill();
                bg_path.fill();

                let rim_color =
                    NSColor::colorWithSRGBRed_green_blue_alpha(0.149, 0.161, 0.180, 1.0);
                rim_color.setStroke();
                bg_path.setLineWidth(1.0);
                bg_path.stroke();

                // 2. 居中鼠标机身背光光晕 (微弱电竞蓝青环境光晕，强化空间层次)
                let (dw, dh, ox, oy) = calc_viewport(w, h, mode);
                let center_x = ox + dw * 0.5;
                let center_y = oy + dh * 0.5;
                let aura_w = dw * 0.75;
                let aura_h = dh * 0.75;
                let aura_rect = NSRect::new(
                    NSPoint::new(center_x - aura_w * 0.5, center_y - aura_h * 0.5),
                    NSSize::new(aura_w, aura_h),
                );
                let aura_path = NSBezierPath::bezierPathWithOvalInRect(aura_rect);
                let aura_color =
                    NSColor::colorWithSRGBRed_green_blue_alpha(0.0, 0.80, 1.0, 0.035);
                aura_color.setFill();
                aura_path.fill();

                // 3. 绘制 G502 原生高精实机机身渲染图
                let img_rect = NSRect::new(NSPoint::new(ox, oy), NSSize::new(dw, dh));
                match mode {
                    CanvasMode::Top => {
                        with_top_image(|img| {
                            img.drawInRect(img_rect);
                        });
                    }
                    CanvasMode::Side => {
                        with_side_image(|img| {
                            img.drawInRect(img_rect);
                        });
                    }
                }

                // 4. 绘制按键/灯效分区指示光圈、折线引出线与高对比度药丸徽章
                let tab = self.ivars().tab_idx.get();
                let zones = match mode {
                    CanvasMode::Top if tab == 1 => LIGHTING_ZONES,
                    CanvasMode::Top => TOP_KEY_ZONES,
                    CanvasMode::Side => SIDE_KEY_ZONES,
                };

                for z in zones {
                    let is_sel = if tab == 1 && selected.is_none() {
                        true // 全部联动时两分区均高亮激活
                    } else {
                        selected.as_deref() == Some(z.key)
                    };
                    let is_hov = hovered.as_deref() == Some(z.key);
                    let is_bnd = bound_set.contains(z.key);

                    Self::draw_zone(w, dw, dh, ox, oy, z, is_sel, is_hov, is_bnd);
                }
            }
        }
    }
);

impl G502MouseCanvas {
    /// 创建画布视图
    pub fn new(frame: NSRect, mtm: MainThreadMarker) -> Retained<Self> {
        unsafe {
            let alloc = mtm.alloc::<Self>();
            let this: Option<Retained<Self>> = msg_send_id![alloc, initWithFrame: frame];
            this.expect("Failed to initialize G502MouseCanvas")
        }
    }

    /// 设置视角模式: 俯视 / 侧视
    pub fn set_mode(&self, mode: CanvasMode) {
        let mut m = self.ivars().mode.borrow_mut();
        if *m != mode {
            *m = mode;
            drop(m);
            unsafe { self.setNeedsDisplay(true) };
        }
    }

    /// 设置当前功能页模式 (0: DPI, 1: 灯效, 2: 宏)
    pub fn set_tab(&self, tab_idx: usize) {
        if self.ivars().tab_idx.get() != tab_idx {
            self.ivars().tab_idx.set(tab_idx);
            *self.ivars().hovered_key.borrow_mut() = None;
            *self.ivars().selected_key.borrow_mut() = None;
            unsafe { self.setNeedsDisplay(true) };
        }
    }

    /// 设置当前选中的按键名称 (例如 "G7")
    pub fn set_selected_key(&self, key: Option<String>) {
        let mut cur = self.ivars().selected_key.borrow_mut();
        if *cur != key {
            *cur = key;
            drop(cur);
            unsafe { self.setNeedsDisplay(true) };
        }
    }

    /// 设置有宏绑定的按键集合 (用于高亮/标记已配置宏的按键)
    pub fn set_bound_keys(&self, keys: HashSet<String>) {
        *self.ivars().bound_keys.borrow_mut() = keys;
        unsafe { self.setNeedsDisplay(true) };
    }

    /// 消费取走最后一次被点击的按键，无外部副作用
    pub fn take_clicked_key(&self) -> Option<String> {
        self.ivars().clicked_key.borrow_mut().take()
    }

    /// 计算指定按键在画布中的药丸徽章外接矩形
    pub fn badge_rect_for_zone(
        w: f64,
        dw: f64,
        dh: f64,
        ox: f64,
        oy: f64,
        z: &ZonePolygon,
    ) -> NSRect {
        // 灯效分区名称较长 (主要灯带 4 字 + 描述 3 字)，预留 118.0pt 保证药丸徽章绝不截断
        let is_lighting = z.key == "主要灯带" || z.key == "G 标志";
        let badge_w = if is_lighting { 118.0 } else { 96.0 };
        let badge_h = 22.0;
        let bx = if z.badge_left {
            (ox - badge_w - 24.0).max(12.0)
        } else {
            (ox + dw + 24.0).min(w - badge_w - 12.0)
        };
        let by = oy + z.badge_y_ratio * dh - badge_h * 0.5;
        NSRect::new(NSPoint::new(bx, by), NSSize::new(badge_w, badge_h))
    }

    /// 在画布中定位按键热点或标签徽章的命中测试
    pub fn resolve_hit(
        mode: CanvasMode,
        w: f64,
        h: f64,
        local_pt: NSPoint,
        tab_idx: usize,
    ) -> Option<&'static str> {
        let (dw, dh, ox, oy) = calc_viewport(w, h, mode);
        let zones = match mode {
            CanvasMode::Top if tab_idx == 1 => LIGHTING_ZONES,
            CanvasMode::Top => TOP_KEY_ZONES,
            CanvasMode::Side => SIDE_KEY_ZONES,
        };

        // 1. 优先检查引出端药丸徽章 (Badge Rect)
        for z in zones {
            let rect = Self::badge_rect_for_zone(w, dw, dh, ox, oy, z);
            if local_pt.x >= rect.origin.x
                && local_pt.x <= rect.origin.x + rect.size.width
                && local_pt.y >= rect.origin.y
                && local_pt.y <= rect.origin.y + rect.size.height
            {
                return Some(z.key);
            }
        }

        // 2. 检查鼠标机身上的精确发光圆点热区 (半径 18pt)
        for z in zones {
            let hx = ox + z.hotspot.0 * dw;
            let hy = oy + z.hotspot.1 * dh;
            let dist_sq =
                (local_pt.x - hx) * (local_pt.x - hx) + (local_pt.y - hy) * (local_pt.y - hy);
            if dist_sq <= 18.0 * 18.0 {
                return Some(z.key);
            }
        }

        // 3. 检查兼容性多边形区域 (Ray casting)
        let nx = ((local_pt.x - ox) / dw).clamp(0.0, 1.0);
        let ny = ((local_pt.y - oy) / dh).clamp(0.0, 1.0);
        for z in zones {
            if point_in_polygon(nx, ny, z.points) {
                return Some(z.key);
            }
        }

        None
    }

    // ------------------------------------------------------------------ //
    // 私有绘制辅助函数
    // ------------------------------------------------------------------ //

    /// 文本绘制辅助
    unsafe fn draw_text(text: &str, rect: NSRect, font: &NSFont, color: &NSColor) {
        let s = NSString::from_str(text);
        let dict =
            NSMutableDictionary::<objc2::runtime::AnyObject, objc2::runtime::AnyObject>::new();
        let _: () = msg_send![&dict, setObject: &*font, forKey: NSFontAttributeName];
        let _: () = msg_send![&dict, setObject: &*color, forKey: NSForegroundColorAttributeName];
        let _: () = msg_send![&s, drawInRect: rect withAttributes: &*dict];
    }

    /// 绘制按键热区、发光光圈、引出折线与高对比度药丸徽章
    unsafe fn draw_zone(
        w: f64,
        dw: f64,
        dh: f64,
        ox: f64,
        oy: f64,
        zone: &ZonePolygon,
        is_selected: bool,
        is_hovered: bool,
        is_bound: bool,
    ) {
        // 1. 色彩管线 (Logitech G HUB 原生深空炭黑与青色/电竞霓虹色彩标准)
        // 选中: Logitech 亮青色 (#00CEFF) 实心/强发光
        // 悬停: 亮青色边框 + 半透明深青背景
        // 已绑定: 罗技电竞亮绿 (#10D070 / #2BD980) 强调配置激活
        // 常规: 深灰炭黑胶囊 (#1E2126) + 柔和边界 (#3A3F47) + 高清晰字色
        let (stroke_c, tag_bg_c, tag_fg_c, key_fg_c) = if is_selected {
            (
                NSColor::colorWithSRGBRed_green_blue_alpha(0.0, 0.808, 1.0, 1.0), // #00CEFF
                NSColor::colorWithSRGBRed_green_blue_alpha(0.04, 0.32, 0.45, 0.95), // 深青底
                NSColor::colorWithSRGBRed_green_blue_alpha(0.92, 0.98, 1.0, 1.0), // 亮白蓝文字
                NSColor::colorWithSRGBRed_green_blue_alpha(0.0, 0.85, 1.0, 1.0),  // 纯青Key标
            )
        } else if is_hovered {
            (
                NSColor::colorWithSRGBRed_green_blue_alpha(0.18, 0.78, 1.0, 0.95),
                NSColor::colorWithSRGBRed_green_blue_alpha(0.09, 0.18, 0.26, 0.92),
                NSColor::colorWithSRGBRed_green_blue_alpha(0.85, 0.94, 1.0, 1.0),
                NSColor::colorWithSRGBRed_green_blue_alpha(0.35, 0.85, 1.0, 1.0),
            )
        } else if is_bound {
            (
                NSColor::colorWithSRGBRed_green_blue_alpha(0.169, 0.851, 0.502, 0.95), // 罗技绿 #2BD980
                NSColor::colorWithSRGBRed_green_blue_alpha(0.08, 0.19, 0.13, 0.90),
                NSColor::colorWithSRGBRed_green_blue_alpha(0.80, 0.98, 0.88, 1.0),
                NSColor::colorWithSRGBRed_green_blue_alpha(0.20, 0.92, 0.55, 1.0),
            )
        } else {
            (
                NSColor::colorWithSRGBRed_green_blue_alpha(0.227, 0.247, 0.278, 1.0), // #3A3F47
                NSColor::colorWithSRGBRed_green_blue_alpha(0.118, 0.129, 0.149, 0.92), // #1E2126
                NSColor::colorWithSRGBRed_green_blue_alpha(0.820, 0.840, 0.870, 1.0), // 浅灰中文字
                NSColor::colorWithSRGBRed_green_blue_alpha(0.550, 0.580, 0.630, 1.0), // 灰蓝键标
            )
        };

        // 2. 按键机身热点坐标 (基于高精实机照片按键中心)
        let hx = ox + zone.hotspot.0 * dw;
        let hy = oy + zone.hotspot.1 * dh;

        // 3. 药丸胶囊徽章定位
        let badge_rect = Self::badge_rect_for_zone(w, dw, dh, ox, oy, zone);
        let badge_w = badge_rect.size.width;
        let badge_h = badge_rect.size.height;
        let bx = badge_rect.origin.x;
        let by = badge_rect.origin.y;

        // 引出线锚点 (徽章贴近机身一侧的垂直中心)
        let (target_x, target_y) = if zone.badge_left {
            (bx + badge_w, by + badge_h * 0.5)
        } else {
            (bx, by + badge_h * 0.5)
        };

        // 4. 绘制折角引出线 (Leader Line)
        let leader = NSBezierPath::bezierPath();
        leader.moveToPoint(NSPoint::new(hx, hy));
        let mid_x = (hx + target_x) * 0.5;
        leader.lineToPoint(NSPoint::new(mid_x, target_y));
        leader.lineToPoint(NSPoint::new(target_x, target_y));

        let leader_color = if is_selected {
            NSColor::colorWithSRGBRed_green_blue_alpha(0.0, 0.808, 1.0, 0.85)
        } else if is_hovered {
            NSColor::colorWithSRGBRed_green_blue_alpha(0.18, 0.78, 1.0, 0.75)
        } else if is_bound {
            NSColor::colorWithSRGBRed_green_blue_alpha(0.169, 0.851, 0.502, 0.60)
        } else {
            NSColor::colorWithSRGBRed_green_blue_alpha(0.28, 0.31, 0.35, 0.50)
        };
        leader_color.setStroke();
        leader.setLineWidth(if is_selected {
            1.5
        } else if is_hovered {
            1.2
        } else {
            1.0
        });
        leader.stroke();

        // 5. 绘制机身发光圆圈热区 (Hotspot Indicator)
        if is_selected || is_hovered {
            let glow_r = if is_selected { 9.0 } else { 7.5 };
            let glow_rect = NSRect::new(
                NSPoint::new(hx - glow_r, hy - glow_r),
                NSSize::new(glow_r * 2.0, glow_r * 2.0),
            );
            let glow_path = NSBezierPath::bezierPathWithOvalInRect(glow_rect);
            let glow_c = if is_selected {
                NSColor::colorWithSRGBRed_green_blue_alpha(0.0, 0.808, 1.0, 0.35)
            } else {
                NSColor::colorWithSRGBRed_green_blue_alpha(0.0, 0.808, 1.0, 0.20)
            };
            glow_c.setFill();
            glow_path.fill();
        }

        // 外层热点光圈环
        let ring_r = 4.5;
        let ring_rect = NSRect::new(
            NSPoint::new(hx - ring_r, hy - ring_r),
            NSSize::new(ring_r * 2.0, ring_r * 2.0),
        );
        let ring_path = NSBezierPath::bezierPathWithOvalInRect(ring_rect);
        stroke_c.setStroke();
        ring_path.setLineWidth(if is_selected { 1.5 } else { 1.0 });
        ring_path.stroke();

        // 核心发光微珠
        let dot_r = if is_selected { 2.2 } else { 1.5 };
        let dot_rect = NSRect::new(
            NSPoint::new(hx - dot_r, hy - dot_r),
            NSSize::new(dot_r * 2.0, dot_r * 2.0),
        );
        let dot_path = NSBezierPath::bezierPathWithOvalInRect(dot_rect);
        if is_selected {
            NSColor::colorWithSRGBRed_green_blue_alpha(0.5, 0.95, 1.0, 1.0).setFill();
        } else {
            stroke_c.setFill();
        }
        dot_path.fill();

        // 6. 绘制引出端电竞药丸徽章 (Capsule Badge)
        let badge_path =
            NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(badge_rect, 5.0, 5.0);
        tag_bg_c.setFill();
        badge_path.fill();

        stroke_c.setStroke();
        badge_path.setLineWidth(if is_selected { 1.5 } else { 1.0 });
        badge_path.stroke();

        // 7. 徽章内部双段文字: 按键代号 (键标高亮) + 分隔点 + 中文功能名
        let key_font = NSFont::boldSystemFontOfSize(10.0);
        let key_rect = NSRect::new(
            NSPoint::new(bx + 6.0, by + 3.0),
            NSSize::new(badge_w - 12.0, badge_h - 5.0),
        );
        Self::draw_text(zone.key, key_rect, &key_font, &key_fg_c);

        let key_w: f64 = {
            let s = NSString::from_str(zone.key);
            let dict = Self::text_attributes(&key_font, &key_fg_c);
            let size: NSSize = msg_send![&*s, sizeWithAttributes: &*dict];
            size.width
        };

        // 紧凑分隔圆点
        let dot_x = bx + 6.0 + key_w + 3.5;
        let sep_dot_rect = NSRect::new(
            NSPoint::new(dot_x, by + badge_h * 0.5 - 1.0),
            NSSize::new(2.0, 2.0),
        );
        let sep_dot_path = NSBezierPath::bezierPathWithOvalInRect(sep_dot_rect);
        let sep_color = if is_selected {
            NSColor::colorWithSRGBRed_green_blue_alpha(0.0, 0.808, 1.0, 0.70)
        } else {
            NSColor::colorWithSRGBRed_green_blue_alpha(0.40, 0.44, 0.50, 0.60)
        };
        sep_color.setFill();
        sep_dot_path.fill();

        let desc_font = NSFont::systemFontOfSize(10.0);
        let desc_x = dot_x + 5.5;
        let desc_rect = NSRect::new(
            NSPoint::new(desc_x, by + 2.5),
            NSSize::new((badge_w - (desc_x - bx) - 4.0).max(10.0), badge_h - 4.0),
        );
        Self::draw_text(zone.desc, desc_rect, &desc_font, &tag_fg_c);
    }

    /// 构建文本绘制属性字典 (字体 + 前景色)
    unsafe fn text_attributes(
        font: &NSFont,
        color: &NSColor,
    ) -> Retained<NSMutableDictionary<objc2::runtime::AnyObject, objc2::runtime::AnyObject>> {
        let dict =
            NSMutableDictionary::<objc2::runtime::AnyObject, objc2::runtime::AnyObject>::new();
        let _: () = msg_send![&dict, setObject: &*font, forKey: NSFontAttributeName];
        let _: () = msg_send![&dict, setObject: &*color, forKey: NSForegroundColorAttributeName];
        dict
    }
}

// ---------------------------------------------------------------------- //
// 纯单元测试 (Point-in-polygon 与 Zone 成员隶属测试)
// ---------------------------------------------------------------------- //

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_point_in_polygon_square() {
        let square = [(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)];
        assert!(point_in_polygon(5.0, 5.0, &square));
        assert!(point_in_polygon(1.0, 1.0, &square));
        assert!(!point_in_polygon(12.0, 5.0, &square));
        assert!(!point_in_polygon(-1.0, 5.0, &square));
        assert!(!point_in_polygon(5.0, -1.0, &square));
    }

    #[test]
    fn test_point_in_polygon_triangle() {
        let triangle = [(0.0, 0.0), (4.0, 0.0), (2.0, 4.0)];
        assert!(point_in_polygon(2.0, 1.0, &triangle));
        assert!(!point_in_polygon(2.0, 5.0, &triangle));
        assert!(!point_in_polygon(0.0, 3.0, &triangle));
    }

    #[test]
    fn test_top_zones_coverage_and_uniqueness() {
        let mut keys = HashSet::new();
        for z in TOP_KEY_ZONES {
            assert!(
                keys.insert(z.key),
                "Duplicate key in TOP_KEY_ZONES: {}",
                z.key
            );
            assert!(z.points.len() >= 3);
            for &(x, y) in z.points {
                assert!((0.0..=1.0).contains(&x));
                assert!((0.0..=1.0).contains(&y));
            }
        }
        // 顶部必须包含 G7, G8, G9, G10, G11
        for expected in ["G7", "G8", "G9", "G10", "G11"] {
            assert!(keys.contains(expected), "Missing top key: {expected}");
        }
    }

    #[test]
    fn test_side_zones_coverage_and_uniqueness() {
        let mut keys = HashSet::new();
        for z in SIDE_KEY_ZONES {
            assert!(
                keys.insert(z.key),
                "Duplicate key in SIDE_KEY_ZONES: {}",
                z.key
            );
            assert!(z.points.len() >= 3);
            for &(x, y) in z.points {
                assert!((0.0..=1.0).contains(&x));
                assert!((0.0..=1.0).contains(&y));
            }
        }
        // 侧面必须包含 G4, G5, G6
        for expected in ["G4", "G5", "G6"] {
            assert!(keys.contains(expected), "Missing side key: {expected}");
        }
    }

    #[test]
    fn test_zone_membership_top_hit_test() {
        // G7
        assert_eq!(hit_test_key(CanvasMode::Top, 0.28, 0.68), Some("G7"));
        // G8
        assert_eq!(hit_test_key(CanvasMode::Top, 0.28, 0.82), Some("G8"));
        // G9
        assert_eq!(hit_test_key(CanvasMode::Top, 0.50, 0.59), Some("G9"));
        // G10
        assert_eq!(hit_test_key(CanvasMode::Top, 0.44, 0.80), Some("G10"));
        // G11
        assert_eq!(hit_test_key(CanvasMode::Top, 0.56, 0.80), Some("G11"));

        // 空白背景区域不应命中任何键
        assert_eq!(hit_test_key(CanvasMode::Top, 0.05, 0.05), None);
        assert_eq!(hit_test_key(CanvasMode::Top, 0.95, 0.95), None);
    }

    #[test]
    fn test_zone_membership_side_hit_test() {
        // G4
        assert_eq!(hit_test_key(CanvasMode::Side, 0.38, 0.60), Some("G4"));
        // G5
        assert_eq!(hit_test_key(CanvasMode::Side, 0.52, 0.60), Some("G5"));
        // G6
        assert_eq!(hit_test_key(CanvasMode::Side, 0.64, 0.45), Some("G6"));

        // 空平背景区域不应命中任何键
        assert_eq!(hit_test_key(CanvasMode::Side, 0.05, 0.05), None);
        assert_eq!(hit_test_key(CanvasMode::Side, 0.95, 0.95), None);
    }

    #[test]
    fn test_lighting_zones_coverage() {
        assert_eq!(LIGHTING_ZONES.len(), 2);
        for z in LIGHTING_ZONES {
            assert!(z.points.len() >= 3);
            for &(x, y) in z.points {
                assert!((0.0..=1.0).contains(&x));
                assert!((0.0..=1.0).contains(&y));
            }
        }
        assert_eq!(LIGHTING_ZONES[0].key, "主要灯带");
        assert_eq!(LIGHTING_ZONES[1].key, "G 标志");
    }
}
