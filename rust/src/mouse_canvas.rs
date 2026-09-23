//! G502 鼠标按键布局矢量画布 (macOS AppKit 原生 NSBezierPath 绘制)
//!
//! 绘制 G502 LIGHTSPEED 顶部视角与侧面视角的简化矢量轮廓与按键区域，
//! 支持鼠标悬停、按键选中状态、已绑定宏状态显示，以及点击选中交互。

use objc2::mutability::MainThreadOnly;
use objc2::rc::{Allocated, Retained};
use objc2::{declare_class, msg_send, msg_send_id, ClassType, DeclaredClass};
use objc2_app_kit::{
    NSBezierPath, NSColor, NSCursor, NSEvent, NSFont, NSFontAttributeName,
    NSForegroundColorAttributeName, NSTrackingArea, NSTrackingAreaOptions, NSView,
};
use objc2_foundation::{MainThreadMarker, NSMutableDictionary, NSPoint, NSRect, NSSize, NSString};
use std::cell::RefCell;
use std::collections::HashSet;

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

/// 归一化多边形 / 区域
#[derive(Debug, Clone)]
pub struct ZonePolygon {
    pub key: &'static str,
    pub points: &'static [(f64, f64)],
    pub label_pos: (f64, f64),
    pub leader_start: (f64, f64),
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

/// 顶部视角按键热区 (俯视: 鼠标头部朝上 Y=1.0, 尾部朝下 Y=0.0, 左键在 X<0.5, 右键在 X>0.5)
pub const TOP_KEY_ZONES: &[ZonePolygon] = &[
    // G8: 左键外缘前方 (DPI +)
    ZonePolygon {
        key: "G8",
        points: &[(0.24, 0.77), (0.35, 0.77), (0.35, 0.88), (0.24, 0.88)],
        label_pos: (0.04, 0.86),
        leader_start: (0.24, 0.83),
        leader_end: (0.16, 0.86),
    },
    // G7: 左键外缘后方 (DPI -)
    ZonePolygon {
        key: "G7",
        points: &[(0.24, 0.63), (0.35, 0.63), (0.35, 0.74), (0.24, 0.74)],
        label_pos: (0.04, 0.67),
        leader_start: (0.24, 0.68),
        leader_end: (0.16, 0.67),
    },
    // G10: 滚轮左摆
    ZonePolygon {
        key: "G10",
        points: &[(0.40, 0.72), (0.47, 0.72), (0.47, 0.87), (0.40, 0.87)],
        label_pos: (0.12, 0.94),
        leader_start: (0.42, 0.82),
        leader_end: (0.24, 0.94),
    },
    // G11: 滚轮右摆
    ZonePolygon {
        key: "G11",
        points: &[(0.53, 0.72), (0.60, 0.72), (0.60, 0.87), (0.53, 0.87)],
        label_pos: (0.78, 0.94),
        leader_start: (0.58, 0.82),
        leader_end: (0.76, 0.94),
    },
    // G9: 滚轮后方按键 (默认电量/配置切换)
    ZonePolygon {
        key: "G9",
        points: &[(0.44, 0.54), (0.56, 0.54), (0.56, 0.64), (0.44, 0.64)],
        label_pos: (0.78, 0.60),
        leader_start: (0.56, 0.59),
        leader_end: (0.76, 0.60),
    },
];

/// 侧面视角按键热区 (左侧视: 鼠标头部在右 X=1.0, 尾部在左 X=0.0, 底部在 Y=0.0)
pub const SIDE_KEY_ZONES: &[ZonePolygon] = &[
    // G4: 侧键后退
    ZonePolygon {
        key: "G4",
        points: &[(0.30, 0.54), (0.44, 0.54), (0.44, 0.67), (0.30, 0.67)],
        label_pos: (0.14, 0.76),
        leader_start: (0.36, 0.65),
        leader_end: (0.24, 0.76),
    },
    // G5: 侧键前进
    ZonePolygon {
        key: "G5",
        points: &[(0.45, 0.54), (0.59, 0.54), (0.59, 0.67), (0.45, 0.67)],
        label_pos: (0.50, 0.88),
        leader_start: (0.52, 0.67),
        leader_end: (0.54, 0.86),
    },
    // G6: 瞄准键 / DPI Shift (最前端拇指键)
    ZonePolygon {
        key: "G6",
        points: &[(0.57, 0.39), (0.70, 0.39), (0.70, 0.52), (0.57, 0.52)],
        label_pos: (0.76, 0.66),
        leader_start: (0.66, 0.50),
        leader_end: (0.74, 0.66),
    },
];

/// 根据当前模式和归一化坐标测试命中的按键
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
            let nx = (local_pt.x / w).clamp(0.0, 1.0);
            let ny = (local_pt.y / h).clamp(0.0, 1.0);
            let mode = *self.ivars().mode.borrow();
            let hit = hit_test_key(mode, nx, ny).map(|s| s.to_string());

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
            let nx = (local_pt.x / w).clamp(0.0, 1.0);
            let ny = (local_pt.y / h).clamp(0.0, 1.0);
            let mode = *self.ivars().mode.borrow();
            if let Some(key) = hit_test_key(mode, nx, ny) {
                let key_str = key.to_string();
                *self.ivars().selected_key.borrow_mut() = Some(key_str.clone());
                *self.ivars().clicked_key.borrow_mut() = Some(key_str);
                unsafe { self.setNeedsDisplay(true) };
                if let Some(gk) = crate::macro_engine::G_KEYS.iter().find(|gk| gk.name == key) {
                    crate::panel::PopoverPanel::select_gkey(gk.id, true);
                }
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
                // 1. 底板微弱暗色背景
                let bg_color = NSColor::colorWithSRGBRed_green_blue_alpha(0.09, 0.10, 0.13, 0.85);
                bg_color.setFill();
                let bg_path = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
                    bounds,
                    8.0,
                    8.0,
                );
                bg_path.fill();

                // 2. 绘制对应视角的鼠标主体轮廓与关键结构
                match mode {
                    CanvasMode::Top => Self::draw_top_silhouette(w, h),
                    CanvasMode::Side => Self::draw_side_silhouette(w, h),
                }

                // 3. 绘制热区按键与引出线/标签
                let zones = match mode {
                    CanvasMode::Top => TOP_KEY_ZONES,
                    CanvasMode::Side => SIDE_KEY_ZONES,
                };

                for z in zones {
                    let is_sel = selected.as_deref() == Some(z.key);
                    let is_hov = hovered.as_deref() == Some(z.key);
                    let is_bnd = bound_set.contains(z.key);

                    Self::draw_zone(w, h, z, is_sel, is_hov, is_bnd);
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

    /// 绘制 G502 经典俯视多边形机身轮廓 (带左侧指托、分体左右主按键、滚轮槽)
    unsafe fn draw_top_silhouette(w: f64, h: f64) {
        // 主外壳轮廓
        let body_color = NSColor::colorWithSRGBRed_green_blue_alpha(0.16, 0.18, 0.22, 1.0);
        let border_color = NSColor::colorWithSRGBRed_green_blue_alpha(0.28, 0.32, 0.40, 0.9);

        let path = NSBezierPath::bezierPath();
        let body_pts: &[(f64, f64)] = &[
            (0.50, 0.12), // 尾部中心底
            (0.38, 0.15), // 尾部左下
            (0.28, 0.24), // 掌托左下
            (0.22, 0.38), // 左指托扩展尖端
            (0.23, 0.50), // 左指托前过渡
            (0.28, 0.60), // 腰部左内收
            (0.27, 0.75), // 左前边缘
            (0.34, 0.92), // 左前按键尖端
            (0.48, 0.88), // 主按键分界左凹槽
            (0.52, 0.88), // 主按键分界右凹槽
            (0.66, 0.91), // 右前按键尖端
            (0.72, 0.72), // 右前侧边缘
            (0.73, 0.45), // 右腰防滑区
            (0.68, 0.26), // 掌托右侧
            (0.60, 0.15), // 尾部右下
        ];

        let pt0 = body_pts[0];
        path.moveToPoint(NSPoint::new(pt0.0 * w, pt0.1 * h));
        for pt in &body_pts[1..] {
            path.lineToPoint(NSPoint::new(pt.0 * w, pt.1 * h));
        }
        path.closePath();

        body_color.setFill();
        path.fill();
        border_color.setStroke();
        path.setLineWidth(1.2);
        path.stroke();

        // 掌托与指托装饰棱线 (G502 标志性机甲倒角切面)
        let wing_line = NSBezierPath::bezierPath();
        wing_line.moveToPoint(NSPoint::new(0.22 * w, 0.38 * h));
        wing_line.lineToPoint(NSPoint::new(0.34 * w, 0.42 * h));
        wing_line.lineToPoint(NSPoint::new(0.40 * w, 0.30 * h));
        let accent_line_color = NSColor::colorWithSRGBRed_green_blue_alpha(0.22, 0.25, 0.30, 0.8);
        accent_line_color.setStroke();
        wing_line.setLineWidth(1.0);
        wing_line.stroke();

        // 中间滚轮槽及金属滚轮
        let wheel_well_rect = NSRect::new(
            NSPoint::new(0.46 * w, 0.68 * h),
            NSSize::new(0.08 * w, 0.22 * h),
        );
        let well_path =
            NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(wheel_well_rect, 2.0, 2.0);
        let well_color = NSColor::colorWithSRGBRed_green_blue_alpha(0.10, 0.11, 0.14, 1.0);
        well_color.setFill();
        well_path.fill();

        // 滚轮本体
        let wheel_rect = NSRect::new(
            NSPoint::new(0.475 * w, 0.72 * h),
            NSSize::new(0.05 * w, 0.15 * h),
        );
        let wheel_path =
            NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(wheel_rect, 2.0, 2.0);
        let wheel_color = NSColor::colorWithSRGBRed_green_blue_alpha(0.35, 0.38, 0.44, 1.0);
        wheel_color.setFill();
        wheel_path.fill();

        // 左右主按键中缝分界线
        let split_line = NSBezierPath::bezierPath();
        split_line.moveToPoint(NSPoint::new(0.50 * w, 0.90 * h));
        split_line.lineToPoint(NSPoint::new(0.50 * w, 0.92 * h));
        let seam_color = NSColor::colorWithSRGBRed_green_blue_alpha(0.10, 0.11, 0.14, 1.0);
        seam_color.setStroke();
        split_line.setLineWidth(1.5);
        split_line.stroke();

        // G 徽标指示 (发光区示意)
        let logo_center = NSPoint::new(0.50 * w, 0.28 * h);
        let logo_rect = NSRect::new(
            NSPoint::new(logo_center.x - 6.0, logo_center.y - 6.0),
            NSSize::new(12.0, 12.0),
        );
        let logo_path = NSBezierPath::bezierPathWithOvalInRect(logo_rect);
        let logo_color = NSColor::colorWithSRGBRed_green_blue_alpha(0.18, 0.45, 0.70, 0.7);
        logo_color.setFill();
        logo_path.fill();
    }

    /// 绘制 G502 经典侧视流线轮廓 (带前俯冲、大拇指托、顶部滚轮突起)
    unsafe fn draw_side_silhouette(w: f64, h: f64) {
        let body_color = NSColor::colorWithSRGBRed_green_blue_alpha(0.16, 0.18, 0.22, 1.0);
        let border_color = NSColor::colorWithSRGBRed_green_blue_alpha(0.28, 0.32, 0.40, 0.9);

        let path = NSBezierPath::bezierPath();
        let side_pts: &[(f64, f64)] = &[
            (0.18, 0.26), // 尾部着地处
            (0.22, 0.45), // 掌托后背弧顶起
            (0.35, 0.68), // 掌托高点
            (0.50, 0.73), // 隆起最高峰 (滚轮后)
            (0.66, 0.65), // 主按键向下倾斜
            (0.84, 0.48), // 前端尖部俯冲
            (0.85, 0.38), // 前底唇
            (0.72, 0.36), // 拇指前方托槽底部
            (0.42, 0.34), // 拇指托底侧展裙边
            (0.25, 0.28), // 尾部底部
        ];

        let pt0 = side_pts[0];
        path.moveToPoint(NSPoint::new(pt0.0 * w, pt0.1 * h));
        for pt in &side_pts[1..] {
            path.lineToPoint(NSPoint::new(pt.0 * w, pt.1 * h));
        }
        path.closePath();

        body_color.setFill();
        path.fill();
        border_color.setStroke();
        path.setLineWidth(1.2);
        path.stroke();

        // 侧面拇指防滑纹理三角形示意
        let grip_color = NSColor::colorWithSRGBRed_green_blue_alpha(0.12, 0.13, 0.16, 0.9);
        let grip_pts: &[(f64, f64)] = &[(0.30, 0.38), (0.54, 0.38), (0.48, 0.50), (0.34, 0.50)];
        let grip_path = NSBezierPath::bezierPath();
        grip_path.moveToPoint(NSPoint::new(grip_pts[0].0 * w, grip_pts[0].1 * h));
        for pt in &grip_pts[1..] {
            grip_path.lineToPoint(NSPoint::new(pt.0 * w, pt.1 * h));
        }
        grip_path.closePath();
        grip_color.setFill();
        grip_path.fill();

        // 滚轮顶部露出剪影
        let wheel_rect = NSRect::new(
            NSPoint::new(0.68 * w, 0.68 * h),
            NSSize::new(0.06 * w, 0.10 * h),
        );
        let wheel_path =
            NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(wheel_rect, 3.0, 3.0);
        let wheel_color = NSColor::colorWithSRGBRed_green_blue_alpha(0.35, 0.38, 0.44, 1.0);
        wheel_color.setFill();
        wheel_path.fill();
    }

    /// 绘制单个按键热区、引出线与 G 编号标签
    unsafe fn draw_zone(
        w: f64,
        h: f64,
        zone: &ZonePolygon,
        is_selected: bool,
        is_hovered: bool,
        is_bound: bool,
    ) {
        // 1. 颜色状态决议
        // 默认未绑定: 半透明青灰; 已绑定: 柔和绿; 悬停: 亮天蓝; 选中: 活力青/蓝
        let (fill_c, stroke_c, tag_bg_c, tag_fg_c) = if is_selected {
            (
                NSColor::colorWithSRGBRed_green_blue_alpha(0.0, 0.55, 0.95, 0.85),
                NSColor::colorWithSRGBRed_green_blue_alpha(0.4, 0.85, 1.0, 1.0),
                NSColor::colorWithSRGBRed_green_blue_alpha(0.0, 0.55, 0.95, 1.0),
                NSColor::whiteColor(),
            )
        } else if is_hovered {
            (
                NSColor::colorWithSRGBRed_green_blue_alpha(0.15, 0.45, 0.75, 0.75),
                NSColor::colorWithSRGBRed_green_blue_alpha(0.30, 0.75, 1.0, 0.95),
                NSColor::colorWithSRGBRed_green_blue_alpha(0.20, 0.55, 0.85, 1.0),
                NSColor::whiteColor(),
            )
        } else if is_bound {
            (
                NSColor::colorWithSRGBRed_green_blue_alpha(0.12, 0.42, 0.30, 0.70),
                NSColor::colorWithSRGBRed_green_blue_alpha(0.25, 0.80, 0.55, 0.90),
                NSColor::colorWithSRGBRed_green_blue_alpha(0.14, 0.48, 0.34, 1.0),
                NSColor::colorWithSRGBRed_green_blue_alpha(0.85, 1.0, 0.90, 1.0),
            )
        } else {
            (
                NSColor::colorWithSRGBRed_green_blue_alpha(0.20, 0.24, 0.30, 0.60),
                NSColor::colorWithSRGBRed_green_blue_alpha(0.38, 0.44, 0.55, 0.80),
                NSColor::colorWithSRGBRed_green_blue_alpha(0.22, 0.26, 0.32, 0.95),
                NSColor::colorWithSRGBRed_green_blue_alpha(0.75, 0.80, 0.88, 1.0),
            )
        };

        // 2. 绘制多边形热区按键块
        let path = NSBezierPath::bezierPath();
        let pt0 = zone.points[0];
        path.moveToPoint(NSPoint::new(pt0.0 * w, pt0.1 * h));
        for pt in &zone.points[1..] {
            path.lineToPoint(NSPoint::new(pt.0 * w, pt.1 * h));
        }
        path.closePath();

        fill_c.setFill();
        path.fill();
        stroke_c.setStroke();
        path.setLineWidth(if is_selected || is_hovered { 1.6 } else { 1.0 });
        path.stroke();

        // 3. 常显折线引出线 (Leader Line)
        let leader = NSBezierPath::bezierPath();
        let s_x = zone.leader_start.0 * w;
        let s_y = zone.leader_start.1 * h;
        let e_x = zone.leader_end.0 * w;
        let e_y = zone.leader_end.1 * h;

        leader.moveToPoint(NSPoint::new(s_x, s_y));
        // 水平/垂直两段折线，产生工整指引效果
        let mid_x = (s_x + e_x) * 0.5;
        leader.lineToPoint(NSPoint::new(mid_x, e_y));
        leader.lineToPoint(NSPoint::new(e_x, e_y));

        stroke_c.setStroke();
        leader.setLineWidth(1.1);
        leader.stroke();

        // 4. 引出端常显标签徽章 (Badge Pill / Rect)
        let badge_w = 34.0;
        let badge_h = 17.0;
        let badge_x = zone.label_pos.0 * w;
        let badge_y = zone.label_pos.1 * h - badge_h * 0.5;
        let badge_rect = NSRect::new(
            NSPoint::new(badge_x, badge_y),
            NSSize::new(badge_w, badge_h),
        );

        let badge_path =
            NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(badge_rect, 4.0, 4.0);
        tag_bg_c.setFill();
        badge_path.fill();
        stroke_c.setStroke();
        badge_path.setLineWidth(1.0);
        badge_path.stroke();

        // 5. 徽章内部居中文本 "G4"..."G11"
        let font = NSFont::boldSystemFontOfSize(10.5);
        let text_rect = NSRect::new(
            NSPoint::new(badge_x + 4.0, badge_y + 1.5),
            NSSize::new(badge_w - 4.0, badge_h),
        );
        Self::draw_text(zone.key, text_rect, &font, &tag_fg_c);
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

        // 空白背景区域不应命中任何键
        assert_eq!(hit_test_key(CanvasMode::Side, 0.05, 0.05), None);
        assert_eq!(hit_test_key(CanvasMode::Side, 0.95, 0.95), None);
    }
}
