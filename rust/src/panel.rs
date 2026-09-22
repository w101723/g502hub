//! G502 LIGHTSPEED 原生控制中心 Popover 面板 (macOS 控制中心卡片风格)

use crate::controller::{self, TargetZone};
use crate::features::led;
use objc2::mutability::MainThreadOnly;
use objc2::rc::Retained;
use objc2::runtime::NSObject;
use objc2::{declare_class, msg_send_id, sel, ClassType, DeclaredClass};
use objc2_app_kit::{
    NSApplication, NSBackingStoreType, NSBox, NSBoxType, NSButton, NSColor, NSControl, NSEvent,
    NSFont, NSPanel, NSPopUpMenuWindowLevel, NSScreen, NSSegmentSwitchTracking,
    NSSegmentedControl, NSSlider, NSStackView, NSTextField,
    NSUserInterfaceLayoutOrientation, NSVisualEffectBlendingMode,
    NSVisualEffectMaterial, NSVisualEffectState, NSVisualEffectView, NSWindowButton,
    NSWindowCollectionBehavior, NSWindowStyleMask, NSWindowTitleVisibility,
};
use objc2_foundation::{
    MainThreadMarker, NSArray, NSEdgeInsets, NSNotification, NSNotificationCenter, NSPoint, NSRect,
    NSSize, NSString,
};
use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};

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

declare_class!(
    pub struct PanelDispatcher;

    unsafe impl ClassType for PanelDispatcher {
        type Super = NSObject;
        type Mutability = MainThreadOnly;
        const NAME: &'static str = "G502PanelDispatcher";
    }

    impl DeclaredClass for PanelDispatcher {}

    unsafe impl PanelDispatcher {
        #[method(onDpiChanged:)]
        fn on_dpi_changed(&self, sender: Option<&NSSegmentedControl>) {
            if let Some(ctrl) = sender {
                let seg = unsafe { ctrl.selectedSegment() };
                let dpis = [400u16, 800, 1600, 3200];
                if let Some(&dpi) = dpis.get(seg as usize) {
                    PopoverPanel::dispatch_dpi(dpi);
                }
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

        #[method(onWindowResignKey:)]
        fn on_window_resign_key(&self, _notification: Option<&NSNotification>) {
            PopoverPanel::shared_hide();
        }
    }
);

struct PanelHolder {
    panel: Retained<G502KeyPanel>,
    battery_label: Retained<NSTextField>,
    mode_label: Retained<NSTextField>,
    dpi_control: Retained<NSSegmentedControl>,
    zone_control: Retained<NSSegmentedControl>,
    effect_control: Retained<NSSegmentedControl>,
    brightness_slider: Retained<NSSlider>,
    brightness_label: Retained<NSTextField>,
    rate_slider: Retained<NSSlider>,
    rate_label: Retained<NSTextField>,
    _color_buttons: Vec<Retained<NSButton>>,
    _dispatcher: Retained<PanelDispatcher>,
}

thread_local! {
    static HOLDER: RefCell<Option<PanelHolder>> = const { RefCell::new(None) };
}

static OPEN_REQUEST: AtomicBool = AtomicBool::new(false);
static GLOBAL_MONITOR_RUNNING: AtomicBool = AtomicBool::new(false);

pub struct PopoverPanel;

impl PopoverPanel {
    pub fn init(mtm: MainThreadMarker) {
        let panel_width = 330.0;
        let panel_height = 490.0;
        let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(panel_width, panel_height));
        let style = NSWindowStyleMask::Titled
            | NSWindowStyleMask::FullSizeContentView
            | NSWindowStyleMask::NonactivatingPanel;

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
            panel.setTitlebarAppearsTransparent(true);
            panel.setTitleVisibility(NSWindowTitleVisibility::NSWindowTitleHidden);
            panel.setBackgroundColor(Some(&NSColor::clearColor()));
            panel.setHasShadow(true);
            panel.setLevel(NSPopUpMenuWindowLevel);
            panel.setBecomesKeyOnlyIfNeeded(false);
            panel.setHidesOnDeactivate(false);
            panel.setCollectionBehavior(
                NSWindowCollectionBehavior::MoveToActiveSpace
                    | NSWindowCollectionBehavior::Transient
                    | NSWindowCollectionBehavior::IgnoresCycle,
            );

            if let Some(btn) = panel.standardWindowButton(NSWindowButton::NSWindowCloseButton) {
                btn.setHidden(true);
            }
            if let Some(btn) = panel.standardWindowButton(NSWindowButton::NSWindowMiniaturizeButton) {
                btn.setHidden(true);
            }
            if let Some(btn) = panel.standardWindowButton(NSWindowButton::NSWindowZoomButton) {
                btn.setHidden(true);
            }
        }

        let effect_view = unsafe { NSVisualEffectView::new(mtm) };
        unsafe {
            effect_view.setMaterial(NSVisualEffectMaterial::Popover);
            effect_view.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
            effect_view.setState(NSVisualEffectState::Active);
            panel.setContentView(Some(&effect_view));
        }

        let dispatcher: Retained<PanelDispatcher> = unsafe { msg_send_id![mtm.alloc(), init] };

        // 根垂直堆叠
        let root_stack = unsafe { NSStackView::new(mtm) };
        unsafe {
            root_stack.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
            root_stack.setSpacing(12.0);
            root_stack.setEdgeInsets(NSEdgeInsets {
                top: 16.0,
                left: 16.0,
                bottom: 16.0,
                right: 16.0,
            });
            root_stack.setFrame(NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(panel_width, panel_height),
            ));
            effect_view.addSubview(&root_stack);
        }

        // 1. Header 卡片 (设备标题 + 电池状态)
        let header_row = unsafe { NSStackView::new(mtm) };
        unsafe {
            header_row.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
            header_row.setSpacing(8.0);
        }
        let title_label = unsafe { NSTextField::labelWithString(&NSString::from_str("G502 LIGHTSPEED"), mtm) };
        unsafe {
            title_label.setFont(Some(&NSFont::boldSystemFontOfSize(14.0)));
            header_row.addArrangedSubview(&title_label);
        }
        let battery_label = unsafe { NSTextField::labelWithString(&NSString::from_str("🔋 --%"), mtm) };
        unsafe {
            battery_label.setFont(Some(&NSFont::systemFontOfSize(12.0)));
            header_row.addArrangedSubview(&battery_label);
            root_stack.addArrangedSubview(&header_row);
        }

        // 模式状态
        let mode_label = unsafe { NSTextField::labelWithString(&NSString::from_str("控制模式: 主机控制"), mtm) };
        unsafe {
            mode_label.setFont(Some(&NSFont::systemFontOfSize(11.0)));
            mode_label.setTextColor(Some(&NSColor::secondaryLabelColor()));
            root_stack.addArrangedSubview(&mode_label);
            root_stack.addArrangedSubview(&Self::create_separator(mtm));
        }

        // 2. DPI 快捷切换卡片
        let dpi_title = unsafe { NSTextField::labelWithString(&NSString::from_str("灵敏度 DPI"), mtm) };
        unsafe {
            dpi_title.setFont(Some(&NSFont::boldSystemFontOfSize(12.0)));
            root_stack.addArrangedSubview(&dpi_title);
        }

        let dpi_labels = NSArray::from_vec(vec![
            NSString::from_str("400"),
            NSString::from_str("800"),
            NSString::from_str("1600"),
            NSString::from_str("3200"),
        ]);
        let dpi_control = unsafe {
            NSSegmentedControl::segmentedControlWithLabels_trackingMode_target_action(
                &dpi_labels,
                NSSegmentSwitchTracking::SelectOne,
                Some(&dispatcher),
                Some(sel!(onDpiChanged:)),
                mtm,
            )
        };
        unsafe {
            root_stack.addArrangedSubview(&dpi_control);
            root_stack.addArrangedSubview(&Self::create_separator(mtm));
        }

        // 3. RGB 分区选择卡片
        let rgb_title = unsafe { NSTextField::labelWithString(&NSString::from_str("RGB 分区控制"), mtm) };
        unsafe {
            rgb_title.setFont(Some(&NSFont::boldSystemFontOfSize(12.0)));
            root_stack.addArrangedSubview(&rgb_title);
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
            root_stack.addArrangedSubview(&zone_control);
        }

        // 4. 灯效模式切换
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
            effect_control.setSelectedSegment(3); // 默认呼吸
            root_stack.addArrangedSubview(&effect_control);
        }

        // 5. 颜色预设网格 (10 个颜色按钮)
        let color_row1 = unsafe { NSStackView::new(mtm) };
        let color_row2 = unsafe { NSStackView::new(mtm) };
        unsafe {
            color_row1.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
            color_row1.setSpacing(6.0);
            color_row2.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
            color_row2.setSpacing(6.0);
        }

        let mut color_buttons = Vec::new();
        for (i, (c_name, _c_hex)) in crate::menubar::LED_COLORS.iter().enumerate() {
            let btn = unsafe {
                NSButton::buttonWithTitle_target_action(
                    &NSString::from_str(c_name),
                    Some(&dispatcher),
                    Some(sel!(onColorClicked:)),
                    mtm,
                )
            };
            unsafe {
                btn.setTag(i as isize);
                btn.setFont(Some(&NSFont::systemFontOfSize(11.0)));
            }
            if i < 5 {
                unsafe { color_row1.addArrangedSubview(&btn); }
            } else {
                unsafe { color_row2.addArrangedSubview(&btn); }
            }
            color_buttons.push(btn);
        }
        unsafe {
            root_stack.addArrangedSubview(&color_row1);
            root_stack.addArrangedSubview(&color_row2);
            root_stack.addArrangedSubview(&Self::create_separator(mtm));
        }

        // 6. 亮度调节滑块
        let bright_row = unsafe { NSStackView::new(mtm) };
        unsafe {
            bright_row.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
            bright_row.setSpacing(8.0);
        }
        let bright_title = unsafe { NSTextField::labelWithString(&NSString::from_str("亮度:"), mtm) };
        unsafe {
            bright_title.setFont(Some(&NSFont::systemFontOfSize(12.0)));
            bright_row.addArrangedSubview(&bright_title);
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
            bright_row.addArrangedSubview(&brightness_slider);
        }
        let brightness_label = unsafe { NSTextField::labelWithString(&NSString::from_str("100%"), mtm) };
        unsafe {
            brightness_label.setFont(Some(&NSFont::systemFontOfSize(11.0)));
            bright_row.addArrangedSubview(&brightness_label);
            root_stack.addArrangedSubview(&bright_row);
        }

        // 7. 速率调节滑块
        let rate_row = unsafe { NSStackView::new(mtm) };
        unsafe {
            rate_row.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
            rate_row.setSpacing(8.0);
        }
        let rate_title = unsafe { NSTextField::labelWithString(&NSString::from_str("速率:"), mtm) };
        unsafe {
            rate_title.setFont(Some(&NSFont::systemFontOfSize(12.0)));
            rate_row.addArrangedSubview(&rate_title);
        }
        let rate_slider = unsafe {
            NSSlider::sliderWithValue_minValue_maxValue_target_action(
                2000.0,
                1000.0,
                20000.0,
                Some(&dispatcher),
                Some(sel!(onRateChanged:)),
                mtm,
            )
        };
        unsafe {
            rate_slider.setContinuous(true);
            rate_row.addArrangedSubview(&rate_slider);
        }
        let rate_label = unsafe { NSTextField::labelWithString(&NSString::from_str("2.0s"), mtm) };
        unsafe {
            rate_label.setFont(Some(&NSFont::systemFontOfSize(11.0)));
            rate_row.addArrangedSubview(&rate_label);
            root_stack.addArrangedSubview(&rate_row);
        }

        // 注册窗口失焦通知
        unsafe {
            let center = NSNotificationCenter::defaultCenter();
            center.addObserver_selector_name_object(
                &dispatcher,
                sel!(onWindowResignKey:),
                Some(objc2_app_kit::NSWindowDidResignKeyNotification),
                Some(&panel),
            );
        }

        let holder = PanelHolder {
            panel,
            battery_label,
            mode_label,
            dpi_control,
            zone_control,
            effect_control,
            brightness_slider,
            brightness_label,
            rate_slider,
            rate_label,
            _color_buttons: color_buttons,
            _dispatcher: dispatcher,
        };

        HOLDER.with(|cell| {
            *cell.borrow_mut() = Some(holder);
        });
    }

    unsafe fn create_separator(mtm: MainThreadMarker) -> Retained<NSBox> {
        let sep = NSBox::new(mtm);
        sep.setBoxType(NSBoxType::NSBoxSeparator);
        sep
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
    }

    pub fn toggle_at(tray_rect: Option<tray_icon::Rect>) {
        let is_visible = HOLDER.with(|cell| {
            cell.borrow()
                .as_ref()
                .map(|h| h.panel.isVisible())
                .unwrap_or(false)
        });

        if is_visible {
            Self::shared_hide();
        } else {
            Self::show_at(tray_rect);
        }
    }

    pub fn show_at(tray_rect: Option<tray_icon::Rect>) {
        Self::sync_ui_from_config();
        HOLDER.with(|cell| {
            if let Some(h) = cell.borrow().as_ref() {
                let panel_frame = h.panel.frame();

                let (origin_x, origin_y) = if let Some(rect) = tray_rect {
                    // tray_icon 的 rect 已经过逻辑/物理换算，但在多屏或顶部菜单栏坐标系下：
                    // macOS 屏幕原点 (0,0) 在左下角，主屏幕顶部 y 为 screen_h。
                    // 若 rect 物理坐标过大，根据屏幕可视区域适配。
                    let screen_frame = NSScreen::mainScreen(MainThreadMarker::from(&*h.panel))
                        .map(|s| s.visibleFrame())
                        .unwrap_or(NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(1440.0, 900.0)));

                    let mut x = rect.position.x + (rect.size.width as f64 / 2.0) - (panel_frame.size.width / 2.0);
                    // 托盘在顶部，浮窗紧贴菜单栏底边缘
                    let mut y = screen_frame.origin.y + screen_frame.size.height - panel_frame.size.height - 4.0;

                    // 边界限制
                    let max_x = screen_frame.origin.x + screen_frame.size.width - panel_frame.size.width - 8.0;
                    let min_x = screen_frame.origin.x + 8.0;
                    if x > max_x {
                        x = max_x;
                    }
                    if x < min_x {
                        x = min_x;
                    }
                    if y < screen_frame.origin.y + 8.0 {
                        y = screen_frame.origin.y + 8.0;
                    }
                    (x, y)
                } else {
                    let screen_frame = NSScreen::mainScreen(MainThreadMarker::from(&*h.panel))
                        .map(|s| s.visibleFrame())
                        .unwrap_or(NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(1440.0, 900.0)));
                    let x = screen_frame.origin.x + screen_frame.size.width - panel_frame.size.width - 24.0;
                    let y = screen_frame.origin.y + screen_frame.size.height - panel_frame.size.height - 4.0;
                    (x, y)
                };

                unsafe {
                    h.panel.setFrameOrigin(NSPoint::new(origin_x, origin_y));
                    let mtm = MainThreadMarker::from(&*h.panel);
                    let app = NSApplication::sharedApplication(mtm);
                    #[allow(deprecated)]
                    app.activateIgnoringOtherApps(true);

                    h.panel.makeKeyAndOrderFront(None);
                }

                Self::ensure_global_dismiss_monitor();
            }
        });
    }

    pub fn shared_hide() {
        HOLDER.with(|cell| {
            if let Some(h) = cell.borrow().as_ref() {
                h.panel.orderOut(None);
            }
        });
    }

    fn ensure_global_dismiss_monitor() {
        if GLOBAL_MONITOR_RUNNING.swap(true, Ordering::SeqCst) {
            return;
        }
        std::thread::spawn(|| {
            loop {
                std::thread::sleep(std::time::Duration::from_millis(150));
                let visible = HOLDER.with(|cell| {
                    cell.borrow()
                        .as_ref()
                        .map(|h| h.panel.isVisible())
                        .unwrap_or(false)
                });

                if visible {
                    let mouse = unsafe { NSEvent::mouseLocation() };
                    let buttons = unsafe { NSEvent::pressedMouseButtons() };
                    if buttons != 0 {
                        HOLDER.with(|cell| {
                            if let Some(h) = cell.borrow().as_ref() {
                                let f = h.panel.frame();
                                let in_x = mouse.x >= f.origin.x && mouse.x <= f.origin.x + f.size.width;
                                let in_y = mouse.y >= f.origin.y && mouse.y <= f.origin.y + f.size.height;
                                if !in_x || !in_y {
                                    PopoverPanel::shared_hide();
                                }
                            }
                        });
                    }
                }
            }
        });
    }

    // ---- 事件分发逻辑 ----

    fn dispatch_dpi(dpi: u16) {
        std::thread::spawn(move || {
            if let Ok(dev) = crate::device::get_conn(2) {
                if let Ok(actual) = controller::set_dpi_confirmed(&dev, dpi) {
                    let _ = crate::config::update(|cfg| {
                        cfg.desired_mode = crate::config::DesiredMode::Host;
                        cfg.desired_dpi = Some(actual);
                    });
                }
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
        let cfg = crate::config::load().unwrap_or_default();
        match zone_seg {
            2 => cfg.led_zones.get("logo").cloned().unwrap_or_else(|| {
                crate::config::LedSpec::new("breathing", [34, 68, 255], 100, Some("2000"))
            }),
            _ => cfg.led_zones.get("primary").cloned().unwrap_or_else(|| {
                crate::config::LedSpec::new("breathing", [255, 0, 0], 100, Some("2000"))
            }),
        }
    }

    fn dispatch_zone(_zone_idx: usize) {
        Self::sync_ui_from_config();
    }

    fn dispatch_effect(effect_seg: usize) {
        HOLDER.with(|cell| {
            if let Some(h) = cell.borrow().as_ref() {
                let zone_seg = unsafe { h.zone_control.selectedSegment() } as usize;
                let target = Self::current_target_zone(zone_seg);
                let mut spec = Self::current_spec(zone_seg);

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
        });
    }

    fn dispatch_color(color_idx: usize) {
        if let Some((_name, hex)) = crate::menubar::LED_COLORS.get(color_idx) {
            let rgb = crate::menubar::hex_rgb(hex);
            HOLDER.with(|cell| {
                if let Some(h) = cell.borrow().as_ref() {
                    let zone_seg = unsafe { h.zone_control.selectedSegment() } as usize;
                    let target = Self::current_target_zone(zone_seg);
                    let mut spec = Self::current_spec(zone_seg);

                    spec.turn_on();
                    spec.rgb = rgb;
                    if spec.effect == "off" || spec.effect == "cycle" {
                        spec.effect = "breathing".into();
                        unsafe { h.effect_control.setSelectedSegment(3); }
                    }

                    controller::schedule_live_led(target, spec);
                }
            });
        }
    }

    fn dispatch_brightness(val: u8) {
        HOLDER.with(|cell| {
            if let Some(h) = cell.borrow().as_ref() {
                let zone_seg = unsafe { h.zone_control.selectedSegment() } as usize;
                let target = Self::current_target_zone(zone_seg);
                let mut spec = Self::current_spec(zone_seg);

                spec.brightness = val;
                if val > 0 && spec.off {
                    spec.turn_on();
                }

                unsafe {
                    h.brightness_label.setStringValue(&NSString::from_str(&format!("{val}%")));
                }

                controller::schedule_live_led(target, spec);
            }
        });
    }

    fn dispatch_rate(val: u16) {
        HOLDER.with(|cell| {
            if let Some(h) = cell.borrow().as_ref() {
                let zone_seg = unsafe { h.zone_control.selectedSegment() } as usize;
                let target = Self::current_target_zone(zone_seg);
                let mut spec = Self::current_spec(zone_seg);

                spec.rate = Some(val.to_string());

                unsafe {
                    let sec = val as f32 / 1000.0;
                    h.rate_label.setStringValue(&NSString::from_str(&format!("{sec:.1}s")));
                }

                controller::schedule_live_led(target, spec);
            }
        });
    }

    pub fn sync_status(battery_text: &str, mode_text: &str, dpi: Option<u16>) {
        HOLDER.with(|cell| {
            if let Some(h) = cell.borrow().as_ref() {
                unsafe {
                    h.battery_label.setStringValue(&NSString::from_str(battery_text));
                    h.mode_label.setStringValue(&NSString::from_str(mode_text));
                    if let Some(d) = dpi {
                        let dpis = [400u16, 800, 1600, 3200];
                        if let Some(idx) = dpis.iter().position(|&x| x == d) {
                            h.dpi_control.setSelectedSegment(idx as isize);
                        }
                    }
                }
            }
        });
    }

    pub fn sync_ui_from_config() {
        HOLDER.with(|cell| {
            if let Some(h) = cell.borrow().as_ref() {
                let zone_seg = unsafe { h.zone_control.selectedSegment() } as usize;
                let spec = Self::current_spec(zone_seg);

                unsafe {
                    let effect_idx = if spec.off {
                        0
                    } else {
                        match spec.effect.as_str() {
                            "solid" => 1,
                            "cycle" => 2,
                            _ => 3, // breathing
                        }
                    };
                    h.effect_control.setSelectedSegment(effect_idx);

                    h.brightness_slider.setDoubleValue(spec.brightness as f64);
                    h.brightness_label.setStringValue(&NSString::from_str(&format!("{}%", spec.brightness)));

                    let period = led::rate_period_ms(spec.rate.as_deref()).unwrap_or(2000);
                    h.rate_slider.setDoubleValue(period as f64);
                    let sec = period as f32 / 1000.0;
                    h.rate_label.setStringValue(&NSString::from_str(&format!("{sec:.1}s")));
                }
            }
        });
    }
}
