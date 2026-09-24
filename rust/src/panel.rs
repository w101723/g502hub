//! G502 LIGHTSPEED 原生控制中心窗口。

use crate::controller::{self, TargetZone};
use crate::features::led;
use crate::mouse_canvas::{CanvasMode, G502MouseCanvas};
use objc2::mutability::MainThreadOnly;
use objc2::rc::Retained;
use objc2::runtime::NSObject;
use objc2::{declare_class, msg_send_id, sel, ClassType, DeclaredClass};
use objc2_app_kit::{
    NSAppearance, NSAppearanceCustomization, NSAppearanceNameAqua, NSApplication,
    NSBackingStoreType, NSBezelStyle, NSBezierPath, NSBox, NSBoxType, NSButton, NSButtonType,
    NSCellImagePosition, NSColor, NSControl, NSFloatingWindowLevel, NSFont, NSFontWeightBold,
    NSFontWeightMedium, NSImage, NSLayoutAttribute, NSLayoutConstraintOrientation, NSPanel,
    NSScreen, NSScrollView, NSSegmentStyle, NSSegmentSwitchTracking, NSSegmentedControl, NSSlider,
    NSStackView, NSStackViewDistribution, NSTextAlignment, NSTextField,
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
        #[method(onMainTabChanged:)]
        fn on_main_tab_changed(&self, sender: Option<&NSSegmentedControl>) {
            if let Some(ctrl) = sender {
                let seg = unsafe { ctrl.selectedSegment() } as usize;
                PopoverPanel::switch_main_tab(seg);
            }
        }

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

        #[method(onDpiPresetChanged:)]
        fn on_dpi_preset_changed(&self, sender: Option<&NSSegmentedControl>) {
            if let Some(ctrl) = sender {
                let seg = unsafe { ctrl.selectedSegment() } as usize;
                if let Some(&dpi) = PopoverPanel::DPI_PRESETS.get(seg) {
                    PopoverPanel::dispatch_dpi_preset(dpi);
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

        #[method(onToggleMacroEngine:)]
        fn on_toggle_macro_engine(&self, _sender: Option<&NSButton>) {
            crate::menubar::dispatch_menu_action("macro:toggle");
            PopoverPanel::sync_macro_ui();
        }

        #[method(onTextInputEnded:)]
        fn on_text_input_ended(&self, sender: Option<&NSTextField>) {
            if let Some(tf) = sender {
                let tag = unsafe { tf.tag() } as usize;
                if let Some(gk) = crate::macro_engine::G_KEYS.get(tag) {
                    PopoverPanel::select_gkey(gk.id, false);
                    let text = unsafe { tf.stringValue() }.to_string();
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
    battery_label: Retained<NSTextField>,
    mode_label: Retained<NSTextField>,
    sidebar_buttons: Vec<Retained<NSButton>>,
    current_tab: Cell<usize>,
    dpi_stack: Retained<NSStackView>,
    rgb_stack: Retained<NSStackView>,
    macro_stack: Retained<NSStackView>,

    // 性能与 DPI
    dpi_value_label: Retained<NSTextField>,
    dpi_slider: Retained<NSSlider>,
    dpi_presets: Retained<NSSegmentedControl>,
    active_dpi: Cell<u16>,

    // RGB 灯效
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
    macro_toggle_btn: Retained<NSButton>,
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

pub struct PopoverPanel;

impl PopoverPanel {
    pub const DPI_PRESETS: &'static [u16] = &[400, 800, 1600, 3200, 6400];
    pub const PANEL_WIDTH: f64 = 760.0;
    pub const PANEL_HEIGHT: f64 = 520.0;
    pub const SIDEBAR_WIDTH: f64 = 68.0;

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

    /// 系统简约深色主文字 (#1D1D1F) - 浅色底板下清晰易读
    pub fn color_primary_text() -> Retained<NSColor> {
        unsafe { NSColor::colorWithSRGBRed_green_blue_alpha(0.114, 0.114, 0.122, 1.0) }
    }

    /// 系统中性灰辅助文字 (#6E6E73)
    pub fn color_secondary_text() -> Retained<NSColor> {
        unsafe { NSColor::colorWithSRGBRed_green_blue_alpha(0.431, 0.431, 0.451, 1.0) }
    }

    /// 罗技 / macOS 活力蓝 (#0071E3)
    pub fn color_accent_cyan() -> Retained<NSColor> {
        unsafe { NSColor::colorWithSRGBRed_green_blue_alpha(0.0, 0.443, 0.890, 1.0) }
    }

    /// 翡翠薄荷绿 (#34C759)
    pub fn color_success_green() -> Retained<NSColor> {
        unsafe { NSColor::colorWithSRGBRed_green_blue_alpha(0.204, 0.780, 0.349, 1.0) }
    }

    /// 战术亮橙色 (#FF9500)
    pub fn color_warning_orange() -> Retained<NSColor> {
        unsafe { NSColor::colorWithSRGBRed_green_blue_alpha(1.0, 0.584, 0.0, 1.0) }
    }

    pub fn init(mtm: MainThreadMarker) {
        let panel_width = Self::PANEL_WIDTH;
        let panel_height = Self::PANEL_HEIGHT;
        let sidebar_width = Self::SIDEBAR_WIDTH;
        let right_width = panel_width - sidebar_width;
        let right_content_width = right_width - 24.0;
        let drawer_inner_width = right_content_width - 28.0;

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
            panel.setTitle(&NSString::from_str("G HUB · G502 LIGHTSPEED"));
            panel.setTitlebarAppearsTransparent(true);
            panel.setTitleVisibility(NSWindowTitleVisibility::NSWindowTitleHidden);
            panel.setAppearance(NSAppearance::appearanceNamed(NSAppearanceNameAqua).as_deref());
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

        // 主窗口简约浅色底板 (macOS 极简浅灰 #F6F6F9 + 细边框 #D9D9E0 + 12pt 圆角)
        let main_box = unsafe { NSBox::new(mtm) };
        unsafe {
            main_box.setBoxType(NSBoxType::NSBoxCustom);
            main_box.setCornerRadius(12.0);
            main_box.setBorderWidth(1.0);
            let border_c = NSColor::colorWithSRGBRed_green_blue_alpha(0.85, 0.85, 0.88, 1.0);
            main_box.setBorderColor(&border_c);
            let bg_c = NSColor::colorWithSRGBRed_green_blue_alpha(0.965, 0.965, 0.975, 1.0);
            main_box.setFillColor(&bg_c);
            main_box.setContentViewMargins(NSSize::new(0.0, 0.0));
            panel.setContentView(Some(&main_box));
        }

        // 水平根容器：左侧 G 垂直导航条 (68pt) + 右侧主工作区 (692pt)
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
        // 1. 左侧垂直 G 导航条 (Sidebar: 68pt 宽, 系统中性浅灰 #EEEEF2)
        // ==================================================================== //
        let sidebar_box = unsafe { NSBox::new(mtm) };
        unsafe {
            sidebar_box.setBoxType(NSBoxType::NSBoxCustom);
            sidebar_box.setCornerRadius(0.0);
            sidebar_box.setBorderWidth(1.0);
            let border_c = NSColor::colorWithSRGBRed_green_blue_alpha(0.88, 0.88, 0.90, 1.0);
            sidebar_box.setBorderColor(&border_c);
            let bg_c = NSColor::colorWithSRGBRed_green_blue_alpha(0.935, 0.935, 0.945, 1.0);
            sidebar_box.setFillColor(&bg_c);
            sidebar_box.setContentViewMargins(NSSize::new(4.0, 12.0));
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
            sidebar_stack.setSpacing(10.0);
            sidebar_box.setContentView(Some(&sidebar_stack));
        }

        // 顶端避让系统红点 (Traffic Light)
        let close_spacer = unsafe { NSView::new(mtm) };
        unsafe {
            close_spacer
                .widthAnchor()
                .constraintEqualToConstant(48.0)
                .setActive(true);
            close_spacer
                .heightAnchor()
                .constraintEqualToConstant(20.0)
                .setActive(true);
            sidebar_stack.addArrangedSubview(&close_spacer);
        }

        // 罗技 G 标志 (活力蓝 #0071E3)
        let g_logo = unsafe { NSTextField::labelWithString(&NSString::from_str("G"), mtm) };
        unsafe {
            let font = NSFont::boldSystemFontOfSize(26.0);
            g_logo.setFont(Some(&font));
            let blue = Self::color_accent_cyan();
            g_logo.setTextColor(Some(&blue));
            g_logo.setAlignment(NSTextAlignment::Center);
            sidebar_stack.addArrangedSubview(&g_logo);
        }

        let sidebar_divider = unsafe { Self::create_separator(mtm, 44.0) };
        unsafe {
            sidebar_stack.addArrangedSubview(&sidebar_divider);
        }

        // 3 个核心导航模式按钮：🎯 DPI | 💡 灯效 | ⌨️ 宏/文字 (单行完整显示)
        let tab_specs = [("🎯 DPI", 0), ("💡 灯效", 1), ("⌨️ 宏/文字", 2)];
        let mut sidebar_buttons = Vec::new();
        for (label, tag) in tab_specs {
            let btn = unsafe {
                NSButton::buttonWithTitle_target_action(
                    &NSString::from_str(label),
                    Some(&dispatcher),
                    Some(sel!(onSidebarTabClicked:)),
                    mtm,
                )
            };
            #[allow(deprecated)]
            unsafe {
                btn.setFont(Some(&NSFont::boldSystemFontOfSize(11.0)));
                btn.setTag(tag);
                btn.setButtonType(NSButtonType::PushOnPushOff);
                btn.setBezelStyle(NSBezelStyle::RegularSquare);
                btn.widthAnchor()
                    .constraintEqualToConstant(58.0)
                    .setActive(true);
                btn.heightAnchor()
                    .constraintEqualToConstant(40.0)
                    .setActive(true);
                if tag == 0 {
                    btn.setState(objc2_app_kit::NSControlStateValueOn);
                    btn.highlight(true);
                } else {
                    btn.setState(objc2_app_kit::NSControlStateValueOff);
                    btn.highlight(false);
                }
                sidebar_stack.addArrangedSubview(&btn);
            }
            sidebar_buttons.push(btn);
        }

        let sidebar_bottom_spacer = unsafe { NSView::new(mtm) };
        unsafe {
            sidebar_bottom_spacer.setContentHuggingPriority_forOrientation(
                1.0,
                NSLayoutConstraintOrientation::Vertical,
            );
            sidebar_stack.addArrangedSubview(&sidebar_bottom_spacer);
        }

        // ==================================================================== //
        // 2. 右侧主工作区 (692pt 宽, 520pt 高)
        // ==================================================================== //
        let content_stack = unsafe { NSStackView::new(mtm) };
        unsafe {
            content_stack.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
            content_stack.setAlignment(NSLayoutAttribute::CenterX);
            content_stack.setSpacing(6.0);
            content_stack.setEdgeInsets(NSEdgeInsets {
                top: 8.0,
                left: 12.0,
                bottom: 8.0,
                right: 12.0,
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

        // 2.1 顶部设备状态条 (Header Row)
        let header_row = unsafe { NSStackView::new(mtm) };
        unsafe {
            header_row.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
            header_row.setAlignment(NSLayoutAttribute::CenterY);
            header_row.setSpacing(8.0);
            header_row
                .widthAnchor()
                .constraintEqualToConstant(right_content_width)
                .setActive(true);
            header_row
                .heightAnchor()
                .constraintEqualToConstant(28.0)
                .setActive(true);
        }

        let title_col = unsafe { NSStackView::new(mtm) };
        unsafe {
            title_col.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
            title_col.setSpacing(1.0);
        }
        let title_label =
            unsafe { NSTextField::labelWithString(&NSString::from_str("G502 LIGHTSPEED"), mtm) };
        unsafe {
            title_label.setFont(Some(&NSFont::boldSystemFontOfSize(13.5)));
            title_label.setTextColor(Some(&Self::color_primary_text()));
            title_col.addArrangedSubview(&title_label);
        }
        let subtitle_label = unsafe {
            NSTextField::labelWithString(&NSString::from_str("HERO 25K SENSOR · 25600 DPI"), mtm)
        };
        unsafe {
            subtitle_label.setFont(Some(&NSFont::systemFontOfSize(9.5)));
            subtitle_label.setTextColor(Some(&Self::color_secondary_text()));
            title_col.addArrangedSubview(&subtitle_label);
            header_row.addArrangedSubview(&title_col);
        }

        let header_spacer = unsafe { Self::create_horizontal_spacer(mtm) };
        unsafe {
            header_row.addArrangedSubview(&header_spacer);
        }

        // 鼠标视角切换：[ 顶部视角 (G7..G11) ] | [ 侧面视角 (G4..G6) ]
        let view_labels = NSArray::from_vec(vec![
            NSString::from_str("顶部视角 (G7..G11)"),
            NSString::from_str("侧面视角 (G4..G6)"),
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
            mouse_view_control.setSegmentStyle(NSSegmentStyle::TexturedRounded);
            header_row.addArrangedSubview(&mouse_view_control);
        }

        // 状态胶囊岛：电池与模式
        let status_pill_stack = unsafe { NSStackView::new(mtm) };
        unsafe {
            status_pill_stack.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
            status_pill_stack.setAlignment(NSLayoutAttribute::CenterY);
            status_pill_stack.setSpacing(6.0);
        }
        let battery_label =
            unsafe { NSTextField::labelWithString(&NSString::from_str("🔋 --%"), mtm) };
        unsafe {
            battery_label.setFont(Some(&NSFont::boldSystemFontOfSize(11.0)));
            battery_label.setTextColor(Some(&Self::color_primary_text()));
            status_pill_stack.addArrangedSubview(&battery_label);
        }
        let mode_label =
            unsafe { NSTextField::labelWithString(&NSString::from_str("主机控制"), mtm) };
        unsafe {
            mode_label.setFont(Some(&NSFont::systemFontOfSize(9.5)));
            mode_label.setTextColor(Some(&Self::color_secondary_text()));
            status_pill_stack.addArrangedSubview(&mode_label);
        }

        let status_pill = unsafe { Self::create_glass_pill(mtm, &status_pill_stack) };
        unsafe {
            header_row.addArrangedSubview(&status_pill);
            content_stack.addArrangedSubview(&header_row);
        }

        // 2.2 中央 Hero 鼠标透视画布 (大幅面 668 × 175 pt, 居中自适应轮廓与发光热点)
        let mouse_canvas = G502MouseCanvas::new(
            NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(right_content_width, 175.0),
            ),
            mtm,
        );
        unsafe {
            mouse_canvas
                .widthAnchor()
                .constraintEqualToConstant(right_content_width)
                .setActive(true);
            mouse_canvas
                .heightAnchor()
                .constraintEqualToConstant(175.0)
                .setActive(true);
            content_stack.addArrangedSubview(&mouse_canvas);
        }

        // 2.3 底部专业功能抽屉容器 (高度 275pt, 简约纯白卡片 #FFFFFF + 浅灰边框 #E2E2E8)
        let drawer_box = unsafe { NSBox::new(mtm) };
        unsafe {
            drawer_box.setBoxType(NSBoxType::NSBoxCustom);
            drawer_box.setBorderWidth(1.0);
            let border_c = NSColor::colorWithSRGBRed_green_blue_alpha(0.88, 0.88, 0.90, 1.0);
            drawer_box.setBorderColor(&border_c);
            let fill_c = NSColor::colorWithSRGBRed_green_blue_alpha(1.0, 1.0, 1.0, 1.0);
            drawer_box.setFillColor(&fill_c);
            drawer_box.setCornerRadius(10.0);
            drawer_box.setContentViewMargins(NSSize::new(14.0, 10.0));
            drawer_box
                .widthAnchor()
                .constraintEqualToConstant(right_content_width)
                .setActive(true);
            drawer_box
                .heightAnchor()
                .constraintEqualToConstant(275.0)
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
        // 抽屉 0: DPI 灵敏度与轮询率 (dpi_stack)
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

        // DPI 顶栏：标题 + 实时档位超大罗技青数值
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
        let dpi_title = unsafe {
            NSTextField::labelWithString(&NSString::from_str("灵敏度 (DPI) 标尺"), mtm)
        };
        unsafe {
            dpi_title.setFont(Some(&NSFont::boldSystemFontOfSize(13.0)));
            dpi_title.setTextColor(Some(&Self::color_primary_text()));
            dpi_header_row.addArrangedSubview(&dpi_title);
        }
        let dpi_spacer = unsafe { Self::create_horizontal_spacer(mtm) };
        unsafe {
            dpi_header_row.addArrangedSubview(&dpi_spacer);
        }
        let dpi_tip_label = unsafe {
            NSTextField::labelWithString(&NSString::from_str("当前活动档位:"), mtm)
        };
        unsafe {
            dpi_tip_label.setFont(Some(&NSFont::systemFontOfSize(11.0)));
            dpi_tip_label.setTextColor(Some(&Self::color_secondary_text()));
            dpi_header_row.addArrangedSubview(&dpi_tip_label);
        }
        let dpi_value_label =
            unsafe { NSTextField::labelWithString(&NSString::from_str("1600 DPI"), mtm) };
        unsafe {
            let font = NSFont::monospacedDigitSystemFontOfSize_weight(18.0, NSFontWeightBold);
            dpi_value_label.setFont(Some(&font));
            dpi_value_label.setTextColor(Some(&Self::color_accent_cyan()));
            dpi_header_row.addArrangedSubview(&dpi_value_label);
            dpi_stack.addArrangedSubview(&dpi_header_row);
        }

        // G HUB 标志性 5 档预设标尺节点 (400, 800, 1600, 3200, 6400)
        let dpi_preset_labels = NSArray::from_vec(vec![
            NSString::from_str("400"),
            NSString::from_str("800"),
            NSString::from_str("1600"),
            NSString::from_str("3200"),
            NSString::from_str("6400"),
        ]);
        let dpi_presets = unsafe {
            NSSegmentedControl::segmentedControlWithLabels_trackingMode_target_action(
                &dpi_preset_labels,
                NSSegmentSwitchTracking::SelectOne,
                Some(&dispatcher),
                Some(sel!(onDpiPresetChanged:)),
                mtm,
            )
        };
        unsafe {
            dpi_presets.setSelectedSegment(2);
            dpi_presets.setSegmentStyle(NSSegmentStyle::TexturedRounded);
            dpi_presets
                .widthAnchor()
                .constraintEqualToConstant(drawer_inner_width)
                .setActive(true);
            dpi_presets
                .heightAnchor()
                .constraintEqualToConstant(26.0)
                .setActive(true);
            dpi_stack.addArrangedSubview(&dpi_presets);
        }

        // 无级平滑微调滑块 (100 ~ 25600)
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
            dpi_min_label.setFont(Some(&NSFont::systemFontOfSize(10.0)));
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
            unsafe { NSTextField::labelWithString(&NSString::from_str("25600"), mtm) };
        unsafe {
            dpi_max_label.setFont(Some(&NSFont::systemFontOfSize(10.0)));
            dpi_max_label.setTextColor(Some(&Self::color_secondary_text()));
            dpi_slider_row.addArrangedSubview(&dpi_max_label);
            dpi_stack.addArrangedSubview(&dpi_slider_row);
        }

        let dpi_sep = unsafe { Self::create_separator(mtm, drawer_inner_width) };
        unsafe {
            dpi_stack.addArrangedSubview(&dpi_sep);
        }

        // 报告率 (轮询率) 控制行
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
        let rate_heading = unsafe {
            NSTextField::labelWithString(&NSString::from_str("报告率 (每秒报告次数)"), mtm)
        };
        unsafe {
            rate_heading.setFont(Some(&NSFont::boldSystemFontOfSize(12.0)));
            rate_heading.setTextColor(Some(&Self::color_primary_text()));
            rate_control_row.addArrangedSubview(&rate_heading);
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
            polling_rate_control.setSegmentStyle(NSSegmentStyle::TexturedRounded);
            rate_control_row.addArrangedSubview(&polling_rate_control);
            dpi_stack.addArrangedSubview(&rate_control_row);
        }

        // 灵敏度快捷提示
        let dpi_hint = unsafe {
            NSTextField::labelWithString(
                &NSString::from_str("💡 提示：使用鼠标左侧 G8/G7 键可在档位间即时加减切换，点击标尺节点亦可直接选定。"),
                mtm,
            )
        };
        unsafe {
            dpi_hint.setFont(Some(&NSFont::systemFontOfSize(10.0)));
            dpi_hint.setTextColor(Some(&Self::color_secondary_text()));
            dpi_stack.addArrangedSubview(&dpi_hint);
        }

        // ==================================================================== //
        // 抽屉 1: LIGHTSYNC RGB 专业灯效 (rgb_stack)
        // ==================================================================== //
        let rgb_stack = unsafe { NSStackView::new(mtm) };
        unsafe {
            rgb_stack.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
            rgb_stack.setAlignment(NSLayoutAttribute::CenterX);
            rgb_stack.setSpacing(10.0);
            rgb_stack
                .widthAnchor()
                .constraintEqualToConstant(drawer_inner_width)
                .setActive(true);
            rgb_stack.setHidden(true);
            drawer_container.addArrangedSubview(&rgb_stack);
        }

        // 分区与效果模式并排顶栏 (双段选择器，充分利用宽幅)
        let rgb_top_row = unsafe { NSStackView::new(mtm) };
        unsafe {
            rgb_top_row.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
            rgb_top_row.setDistribution(NSStackViewDistribution::FillEqually);
            rgb_top_row.setSpacing(16.0);
            rgb_top_row
                .widthAnchor()
                .constraintEqualToConstant(drawer_inner_width)
                .setActive(true);
        }

        // 灯效分区
        let zone_col = unsafe { NSStackView::new(mtm) };
        unsafe {
            zone_col.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
            zone_col.setAlignment(NSLayoutAttribute::Leading);
            zone_col.setSpacing(4.0);
        }
        let zone_title =
            unsafe { NSTextField::labelWithString(&NSString::from_str("灯效分区"), mtm) };
        unsafe {
            zone_title.setFont(Some(&NSFont::boldSystemFontOfSize(11.5)));
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
            zone_col.addArrangedSubview(&zone_control);
            rgb_top_row.addArrangedSubview(&zone_col);
        }

        // 效果模式
        let effect_col = unsafe { NSStackView::new(mtm) };
        unsafe {
            effect_col.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
            effect_col.setAlignment(NSLayoutAttribute::Leading);
            effect_col.setSpacing(4.0);
        }
        let effect_title =
            unsafe { NSTextField::labelWithString(&NSString::from_str("效果模式"), mtm) };
        unsafe {
            effect_title.setFont(Some(&NSFont::boldSystemFontOfSize(11.5)));
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
            effect_col.addArrangedSubview(&effect_control);
            rgb_top_row.addArrangedSubview(&effect_col);
            rgb_stack.addArrangedSubview(&rgb_top_row);
        }

        // 预设色彩矩阵 (10 颗宝石级色彩 Swatch)
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
        let color_title = unsafe {
            NSTextField::labelWithString(&NSString::from_str("预设色彩 (LIGHTSYNC 宝石色调)"), mtm)
        };
        unsafe {
            color_title.setFont(Some(&NSFont::boldSystemFontOfSize(11.5)));
            color_title.setTextColor(Some(&Self::color_primary_text()));
            color_header.addArrangedSubview(&color_title);
        }
        let color_spacer = unsafe { Self::create_horizontal_spacer(mtm) };
        unsafe {
            color_header.addArrangedSubview(&color_spacer);
        }
        let color_label =
            unsafe { NSTextField::labelWithString(&NSString::from_str("选定色彩: --"), mtm) };
        unsafe {
            let font = NSFont::systemFontOfSize(11.0);
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

        // 亮度与速率调节 (宽幅双列并排)
        let sliders_row = unsafe { NSStackView::new(mtm) };
        unsafe {
            sliders_row.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
            sliders_row.setDistribution(NSStackViewDistribution::FillEqually);
            sliders_row.setSpacing(20.0);
            sliders_row
                .widthAnchor()
                .constraintEqualToConstant(drawer_inner_width)
                .setActive(true);
        }

        // 亮度列
        let bright_col = unsafe { NSStackView::new(mtm) };
        unsafe {
            bright_col.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
            bright_col.setAlignment(NSLayoutAttribute::CenterY);
            bright_col.setSpacing(8.0);
        }
        let bright_title =
            unsafe { NSTextField::labelWithString(&NSString::from_str("亮度:"), mtm) };
        unsafe {
            bright_title.setFont(Some(&NSFont::systemFontOfSize(11.5)));
            bright_title.setTextColor(Some(&Self::color_primary_text()));
            bright_col.addArrangedSubview(&bright_title);
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
            bright_col.addArrangedSubview(&brightness_slider);
        }
        let brightness_label =
            unsafe { NSTextField::labelWithString(&NSString::from_str("100%"), mtm) };
        unsafe {
            let font = NSFont::monospacedDigitSystemFontOfSize_weight(11.0, NSFontWeightMedium);
            brightness_label.setFont(Some(&font));
            brightness_label.setTextColor(Some(&Self::color_accent_cyan()));
            bright_col.addArrangedSubview(&brightness_label);
            sliders_row.addArrangedSubview(&bright_col);
        }

        // 速率列
        let rate_col = unsafe { NSStackView::new(mtm) };
        unsafe {
            rate_col.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
            rate_col.setAlignment(NSLayoutAttribute::CenterY);
            rate_col.setSpacing(8.0);
        }
        let rate_title =
            unsafe { NSTextField::labelWithString(&NSString::from_str("速率:"), mtm) };
        unsafe {
            rate_title.setFont(Some(&NSFont::systemFontOfSize(11.5)));
            rate_title.setTextColor(Some(&Self::color_primary_text()));
            rate_col.addArrangedSubview(&rate_title);
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
            rate_col.addArrangedSubview(&rate_slider);
        }
        let rate_label =
            unsafe { NSTextField::labelWithString(&NSString::from_str("2.0s (中)"), mtm) };
        unsafe {
            let font = NSFont::monospacedDigitSystemFontOfSize_weight(11.0, NSFontWeightMedium);
            rate_label.setFont(Some(&font));
            rate_label.setTextColor(Some(&Self::color_accent_cyan()));
            rate_col.addArrangedSubview(&rate_label);
            sliders_row.addArrangedSubview(&rate_col);
            rgb_stack.addArrangedSubview(&sliders_row);
        }

        // ==================================================================== //
        // 抽屉 2: ASSIGNMENTS 按键指派与宏 (macro_stack)
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

        // 顶栏：引擎开关 + 状态提示
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
        let macro_toggle_btn = unsafe {
            NSButton::buttonWithTitle_target_action(
                &NSString::from_str("● 宏引擎已开启"),
                Some(&dispatcher),
                Some(sel!(onToggleMacroEngine:)),
                mtm,
            )
        };
        unsafe {
            macro_toggle_btn.setFont(Some(&NSFont::boldSystemFontOfSize(11.0)));
            macro_header_row.addArrangedSubview(&macro_toggle_btn);
        }
        let macro_spacer = unsafe { Self::create_horizontal_spacer(mtm) };
        unsafe {
            macro_header_row.addArrangedSubview(&macro_spacer);
        }
        let macro_status_label = unsafe {
            NSTextField::labelWithString(
                &NSString::from_str("在下方按键输入框直接输入文字（即时生效），或点击「录制」快捷键"),
                mtm,
            )
        };
        unsafe {
            macro_status_label.setFont(Some(&NSFont::systemFontOfSize(10.5)));
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
                    NSSize::new(drawer_inner_width, 155.0),
                ),
            )
        };
        unsafe {
            macro_scroll.setHasVerticalScroller(true);
            macro_scroll.setDrawsBackground(false);
            macro_scroll.setDocumentView(Some(&macro_rows_stack));
            macro_scroll
                .widthAnchor()
                .constraintEqualToConstant(drawer_inner_width)
                .setActive(true);
            macro_scroll
                .heightAnchor()
                .constraintEqualToConstant(155.0)
                .setActive(true);
            macro_stack.addArrangedSubview(&macro_scroll);
        }

        let mut gkey_rows = Vec::new();
        let row_width = drawer_inner_width - 16.0; // 避开滚动条
        for (i, gk) in crate::macro_engine::G_KEYS.iter().enumerate() {
            let row = unsafe { NSStackView::new(mtm) };
            unsafe {
                row.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
                row.setAlignment(NSLayoutAttribute::CenterY);
                row.setSpacing(6.0);
                row.widthAnchor()
                    .constraintEqualToConstant(row_width)
                    .setActive(true);
            }

            // G-Key 硬件键帽徽章 (统一宽度 82pt, 浅蓝微光徽章)
            let badge_box = unsafe { NSBox::new(mtm) };
            unsafe {
                badge_box.setBoxType(NSBoxType::NSBoxCustom);
                badge_box.setCornerRadius(6.0);
                badge_box.setBorderWidth(1.0);
                let border_c = NSColor::colorWithSRGBRed_green_blue_alpha(0.82, 0.86, 0.92, 1.0);
                badge_box.setBorderColor(&border_c);
                let bg_c = NSColor::colorWithSRGBRed_green_blue_alpha(0.94, 0.96, 0.99, 1.0);
                badge_box.setFillColor(&bg_c);
                badge_box.setContentViewMargins(NSSize::new(2.0, 2.0));
                badge_box
                    .widthAnchor()
                    .constraintEqualToConstant(88.0)
                    .setActive(true);
            }
            let badge_title = match gk.name {
                "G4" => "G4 · 后退",
                "G5" => "G5 · 前进",
                "G6" => "G6 · 瞄准",
                "G7" => "G7 · DPI-",
                "G8" => "G8 · DPI+",
                "G9" => "G9 · ⚡️电量",
                "G10" => "G10 · 滚轮左",
                "G11" => "G11 · 滚轮右",
                _ => gk.name,
            };
            let name_label =
                unsafe { NSTextField::labelWithString(&NSString::from_str(badge_title), mtm) };
            unsafe {
                name_label.setFont(Some(&NSFont::boldSystemFontOfSize(10.0)));
                name_label.setTextColor(Some(&Self::color_accent_cyan()));
                name_label.setAlignment(NSTextAlignment::Center);
                badge_box.setContentView(Some(&name_label));
                row.addArrangedSubview(&badge_box);
            }

            let editor_stack = unsafe { NSStackView::new(mtm) };
            unsafe {
                editor_stack.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
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
                text_input.setPlaceholderString(Some(&NSString::from_str("在此输入自动打字文字 (输入即生效)...")));
                text_input.setTextColor(Some(&Self::color_primary_text()));
                let input_bg = NSColor::colorWithSRGBRed_green_blue_alpha(1.0, 1.0, 1.0, 1.0);
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
                macro_rows_stack.addArrangedSubview(&row);
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

        // 底部快捷操作条 (按键序列录制、完成、取消)
        let macro_action_row = unsafe { NSStackView::new(mtm) };
        unsafe {
            macro_action_row.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
            macro_action_row.setDistribution(NSStackViewDistribution::FillEqually);
            macro_action_row.setSpacing(8.0);
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
            macro_rec_seq_btn.setFont(Some(&NSFont::systemFontOfSize(10.5)));
            macro_action_row.addArrangedSubview(&macro_rec_seq_btn);
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
            macro_rec_finish_btn.setFont(Some(&NSFont::systemFontOfSize(10.5)));
            macro_action_row.addArrangedSubview(&macro_rec_finish_btn);
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
            macro_rec_cancel_btn.setFont(Some(&NSFont::systemFontOfSize(10.5)));
            macro_action_row.addArrangedSubview(&macro_rec_cancel_btn);
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
            battery_label,
            mode_label,
            sidebar_buttons,
            current_tab: Cell::new(0),
            dpi_stack,
            rgb_stack,
            macro_stack,
            dpi_value_label,
            dpi_slider,
            dpi_presets,
            active_dpi: Cell::new(1600),
            zone_control,
            effect_control,
            color_label,
            color_buttons,
            brightness_slider,
            brightness_label,
            rate_slider,
            rate_label,
            active_rgb: Cell::new([0, 200, 255]),
            macro_toggle_btn,
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
    }

    unsafe fn create_separator(mtm: MainThreadMarker, width: f64) -> Retained<NSBox> {
        let sep = NSBox::new(mtm);
        sep.setBoxType(NSBoxType::NSBoxSeparator);
        sep.widthAnchor()
            .constraintEqualToConstant(width)
            .setActive(true);
        sep
    }

    /// 顶部状态胶囊岛：用于 Header 电池与模式状态
    unsafe fn create_glass_pill(mtm: MainThreadMarker, content: &NSStackView) -> Retained<NSBox> {
        let pill = NSBox::new(mtm);
        pill.setBoxType(NSBoxType::NSBoxCustom);
        pill.setCornerRadius(8.0);
        pill.setBorderWidth(1.0);
        let border_c = NSColor::colorWithSRGBRed_green_blue_alpha(0.85, 0.85, 0.88, 1.0);
        pill.setBorderColor(&border_c);
        let fill_c = NSColor::colorWithSRGBRed_green_blue_alpha(1.0, 1.0, 1.0, 1.0);
        pill.setFillColor(&fill_c);
        pill.setContentViewMargins(NSSize::new(10.0, 4.0));
        pill.setContentView(Some(content));
        pill
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
            // 外层选中指示对焦环 (罗技电竞青发光圈)
            let ring_rect = NSRect::new(NSPoint::new(1.0, 1.0), NSSize::new(24.0, 24.0));
            let ring = NSBezierPath::bezierPathWithOvalInRect(ring_rect);
            let ring_color = NSColor::colorWithSRGBRed_green_blue_alpha(0.0, 0.80, 1.0, 1.0);
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

                unsafe {
                    h.dpi_stack.setHidden(tab_idx != 0);
                    h.rgb_stack.setHidden(tab_idx != 1);
                    h.macro_stack.setHidden(tab_idx != 2);

                    for (i, btn) in h.sidebar_buttons.iter().enumerate() {
                        let is_active = i == tab_idx;
                        btn.setState(if is_active {
                            objc2_app_kit::NSControlStateValueOn
                        } else {
                            objc2_app_kit::NSControlStateValueOff
                        });
                        btn.highlight(is_active);
                    }
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
                    h.panel.makeKeyAndOrderFront(None);
                }
            });
        }
    }

    pub fn show_at(tray_rect: Option<tray_icon::Rect>) {
        Self::sync_ui_from_config();
        HOLDER.with(|cell| {
            if let Some(h) = cell.borrow().as_ref() {
                let panel_frame = h.panel.frame();

                let (origin_x, origin_y) = if let Some(rect) = tray_rect {
                    let screen_frame = NSScreen::mainScreen(MainThreadMarker::from(&*h.panel))
                        .map(|s| s.visibleFrame())
                        .unwrap_or(NSRect::new(
                            NSPoint::new(0.0, 0.0),
                            NSSize::new(1440.0, 900.0),
                        ));

                    let mut x = rect.position.x + (rect.size.width as f64 / 2.0)
                        - (panel_frame.size.width / 2.0);
                    let mut y = screen_frame.origin.y + screen_frame.size.height
                        - panel_frame.size.height
                        - 4.0;

                    let max_x = screen_frame.origin.x + screen_frame.size.width
                        - panel_frame.size.width
                        - 8.0;
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
                        .unwrap_or(NSRect::new(
                            NSPoint::new(0.0, 0.0),
                            NSSize::new(1440.0, 900.0),
                        ));
                    let x = screen_frame.origin.x + screen_frame.size.width
                        - panel_frame.size.width
                        - 24.0;
                    let y = screen_frame.origin.y + screen_frame.size.height
                        - panel_frame.size.height
                        - 4.0;
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
            }
        });
    }

    pub(crate) fn on_text_field_changed(obj: &objc2::runtime::AnyObject) {
        HOLDER.with(|cell| {
            if let Some(h) = cell.borrow().as_ref() {
                for row in &h.gkey_rows {
                    let tf_ptr = Retained::as_ptr(&row.text_input) as *const objc2::runtime::AnyObject;
                    let obj_ptr = obj as *const objc2::runtime::AnyObject;
                    if tf_ptr == obj_ptr {
                        let text = if let Some(editor) = Self::active_editor(&row.text_input) {
                            unsafe { editor.string() }.to_string()
                        } else {
                            unsafe { row.text_input.stringValue() }.to_string()
                        };
                        let trimmed = text.trim();
                        if trimmed.is_empty() {
                            let _ = crate::macro_engine::clear_binding(row.key_id);
                            unsafe {
                                row.summary_label.setStringValue(&NSString::from_str("未绑定"));
                                row.summary_label.setTextColor(Some(&Self::color_secondary_text()));
                                row.clear_btn.setEnabled(false);
                            }
                        } else {
                            let _ = crate::macro_engine::save_text_binding(row.key_id, trimmed);
                            unsafe {
                                row.summary_label.setStringValue(&NSString::from_str("文字 · 自动输入"));
                                row.summary_label.setTextColor(Some(&Self::color_accent_cyan()));
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
                    let text = if let Some(editor) = Self::active_editor(&row.text_input) {
                        unsafe { editor.string() }.to_string()
                    } else {
                        unsafe { row.text_input.stringValue() }.to_string()
                    };
                    let trimmed = text.trim();
                    if !trimmed.is_empty() {
                        let _ = crate::macro_engine::save_text_binding(row.key_id, trimmed);
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

    // ---- 事件分发逻辑 ----

    fn dispatch_dpi_slider(t: f64) {
        HOLDER.with(|cell| {
            if let Some(h) = cell.borrow().as_ref() {
                let dpi = Self::slider_to_dpi(t);
                h.active_dpi.set(dpi);
                unsafe {
                    h.dpi_value_label
                        .setStringValue(&NSString::from_str(&format!("{dpi} DPI")));
                    if let Some(idx) = Self::DPI_PRESETS.iter().position(|&p| p == dpi) {
                        h.dpi_presets.setSelectedSegment(idx as isize);
                    } else {
                        h.dpi_presets.setSelectedSegment(-1);
                    }
                }
                controller::schedule_live_dpi(dpi);
            }
        });
    }

    fn dispatch_dpi_preset(dpi: u16) {
        HOLDER.with(|cell| {
            if let Some(h) = cell.borrow().as_ref() {
                h.active_dpi.set(dpi);
                unsafe {
                    h.dpi_value_label
                        .setStringValue(&NSString::from_str(&format!("{dpi} DPI")));
                    h.dpi_slider.setDoubleValue(Self::dpi_to_slider(dpi));
                    if let Some(idx) = Self::DPI_PRESETS.iter().position(|&p| p == dpi) {
                        h.dpi_presets.setSelectedSegment(idx as isize);
                    }
                }
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

    fn dispatch_zone(zone_idx: usize) {
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
                    h.brightness_label
                        .setTextColor(Some(&Self::color_accent_cyan()));
                }

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
                    h.rate_label
                        .setTextColor(Some(&Self::color_accent_cyan()));
                }

                Self::collect_and_schedule(h);
            }
        });
    }

    pub fn sync_status(battery_text: &str, mode_text: &str, dpi: Option<u16>) {
        HOLDER.with(|cell| {
            if let Some(h) = cell.borrow().as_ref() {
                unsafe {
                    h.battery_label
                        .setStringValue(&NSString::from_str(battery_text));
                    h.battery_label
                        .setTextColor(Some(&Self::color_primary_text()));
                    h.mode_label.setStringValue(&NSString::from_str(mode_text));
                    h.mode_label
                        .setTextColor(Some(&Self::color_secondary_text()));
                    if let Some(d) = dpi {
                        let clamped = d.clamp(100, 25600);
                        h.active_dpi.set(clamped);
                        h.dpi_value_label
                            .setStringValue(&NSString::from_str(&format!("{clamped} DPI")));
                        h.dpi_value_label
                            .setTextColor(Some(&Self::color_accent_cyan()));
                        h.dpi_slider.setDoubleValue(Self::dpi_to_slider(clamped));
                        if let Some(idx) = Self::DPI_PRESETS.iter().position(|&p| p == clamped) {
                            h.dpi_presets.setSelectedSegment(idx as isize);
                        } else {
                            h.dpi_presets.setSelectedSegment(-1);
                        }
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
                        .setStringValue(&NSString::from_str(&format!("{clamped_dpi} DPI")));
                    h.dpi_value_label
                        .setTextColor(Some(&Self::color_accent_cyan()));
                    h.dpi_slider
                        .setDoubleValue(Self::dpi_to_slider(clamped_dpi));
                    if let Some(idx) = Self::DPI_PRESETS.iter().position(|&p| p == clamped_dpi) {
                        h.dpi_presets.setSelectedSegment(idx as isize);
                    } else {
                        h.dpi_presets.setSelectedSegment(-1);
                    }
                }

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
                    h.brightness_label
                        .setTextColor(Some(&Self::color_accent_cyan()));

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
                    h.rate_label
                        .setTextColor(Some(&Self::color_accent_cyan()));
                }
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
                    if running {
                        h.macro_toggle_btn
                            .setTitle(&NSString::from_str("● 宏引擎已开启"));
                    } else {
                        h.macro_toggle_btn
                            .setTitle(&NSString::from_str("○ 宏引擎已停用"));
                    }

                    for row in &h.gkey_rows {
                        if Self::active_editor(&row.text_input).is_some() {
                            continue;
                        }
                        let binding = crate::macro_engine::get_binding_for_key(row.key_id);
                        if let Some(b) = binding {
                            if let Some(name) = Self::gkey_name_for_id(row.key_id) {
                                bound_keys.insert(name.to_string());
                            }
                            if let Some(text) = crate::macro_engine::get_binding_text(&b) {
                                row.summary_label
                                    .setStringValue(&NSString::from_str("文字 · 自动输入"));
                                row.summary_label
                                    .setTextColor(Some(&Self::color_accent_cyan()));
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
                                row.text_input.setEditable(false);
                                row.text_input.setStringValue(&NSString::from_str(""));
                                row.text_input
                                    .setPlaceholderString(Some(&NSString::from_str(
                                        "非文字动作；清空后可输入文字",
                                    )));
                                row.text_input
                                    .setTextColor(Some(&Self::color_secondary_text()));
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
                                ("● 宏引擎就绪 (支持组合键与指定文字)".to_string(), false)
                            } else {
                                ("○ 宏引擎已停用 (点击开启以启用按键拦截)".to_string(), false)
                            }
                        }
                        crate::macro_engine::RecordingPhase::AwaitMouse(
                            crate::macro_engine::RecordingKind::Shortcut,
                        ) => ("👉 请按目标侧键 (G4..G11)...".to_string(), true),
                        crate::macro_engine::RecordingPhase::AwaitMouse(
                            crate::macro_engine::RecordingKind::Sequence,
                        ) => ("👉 请按目标侧键开始录制序列...".to_string(), true),
                        crate::macro_engine::RecordingPhase::Shortcut { button } => {
                            let name = crate::macro_engine::get_gkey_by_button(*button)
                                .map(|gk| format!("{}({})", gk.name, gk.desc))
                                .unwrap_or_else(|| format!("button{button}"));
                            (format!("⌨️ 正在录制 {name}: 请按一次键盘快捷键..."), true)
                        }
                        crate::macro_engine::RecordingPhase::Sequence { button, events } => {
                            let name = crate::macro_engine::get_gkey_by_button(*button)
                                .map(|gk| format!("{}({})", gk.name, gk.desc))
                                .unwrap_or_else(|| format!("button{button}"));
                            (
                                format!("🔴 录制中 {name}: 已捕获 {events} 个按键事件"),
                                true,
                            )
                        }
                    };

                    h.macro_status_label
                        .setStringValue(&NSString::from_str(&status_text));
                    if is_warn {
                        h.macro_status_label
                            .setTextColor(Some(&Self::color_warning_orange()));
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
}
