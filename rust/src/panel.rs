//! G502 LIGHTSPEED 原生控制中心 Popover 面板 (macOS 控制中心卡片风格)

use crate::controller::{self, TargetZone};
use crate::features::led;
use crate::mouse_canvas::{CanvasMode, G502MouseCanvas};
use objc2::mutability::MainThreadOnly;
use objc2::rc::Retained;
use objc2::runtime::NSObject;
use objc2::{declare_class, msg_send_id, sel, ClassType, DeclaredClass};
use objc2_app_kit::{
    NSApplication, NSBackingStoreType, NSBox, NSBoxType, NSButton, NSColor, NSControl, NSEvent,
    NSFont, NSPanel, NSPopUpMenuWindowLevel, NSScreen, NSScrollView, NSSegmentSwitchTracking,
    NSSegmentedControl, NSSlider, NSStackView, NSTextField, NSUserInterfaceLayoutOrientation,
    NSVisualEffectBlendingMode, NSVisualEffectMaterial, NSVisualEffectState, NSVisualEffectView,
    NSWindowButton, NSWindowCollectionBehavior, NSWindowStyleMask, NSWindowTitleVisibility,
};
use objc2_foundation::{
    MainThreadMarker, NSArray, NSEdgeInsets, NSNotification, NSNotificationCenter, NSPoint, NSRect,
    NSSize, NSString,
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

        #[method(onMouseViewChanged:)]
        fn on_mouse_view_changed(&self, sender: Option<&NSSegmentedControl>) {
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
                    let _ = crate::macro_engine::save_text_binding(gk.id, &text);
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

        #[method(onWindowResignKey:)]
        fn on_window_resign_key(&self, _notification: Option<&NSNotification>) {
            PopoverPanel::shared_hide();
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
    _tab_control: Retained<NSSegmentedControl>,
    perf_stack: Retained<NSStackView>,
    macro_stack: Retained<NSStackView>,

    // 性能与灯效
    dpi_value_label: Retained<NSTextField>,
    dpi_slider: Retained<NSSlider>,
    dpi_presets: Retained<NSSegmentedControl>,
    active_dpi: Cell<u16>,
    zone_control: Retained<NSSegmentedControl>,
    effect_control: Retained<NSSegmentedControl>,
    color_label: Retained<NSTextField>,
    color_buttons: Vec<Retained<NSButton>>,
    brightness_slider: Retained<NSSlider>,
    brightness_label: Retained<NSTextField>,
    rate_slider: Retained<NSSlider>,
    rate_label: Retained<NSTextField>,
    active_rgb: Cell<[u8; 3]>,

    // 侧键宏
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
static GLOBAL_MONITOR_RUNNING: AtomicBool = AtomicBool::new(false);

pub struct PopoverPanel;

impl PopoverPanel {
    pub const DPI_PRESETS: &'static [u16] = &[400, 800, 1600, 3200, 6400];

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

    pub fn init(mtm: MainThreadMarker) {
        let panel_width = 340.0;
        let panel_height = 620.0;
        let frame = NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new(panel_width, panel_height),
        );
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
            if let Some(btn) = panel.standardWindowButton(NSWindowButton::NSWindowMiniaturizeButton)
            {
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
            root_stack.setSpacing(8.0);
            root_stack.setEdgeInsets(NSEdgeInsets {
                top: 14.0,
                left: 16.0,
                bottom: 14.0,
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
        let title_label =
            unsafe { NSTextField::labelWithString(&NSString::from_str("G502 LIGHTSPEED"), mtm) };
        unsafe {
            title_label.setFont(Some(&NSFont::boldSystemFontOfSize(14.0)));
            header_row.addArrangedSubview(&title_label);
        }
        let battery_label =
            unsafe { NSTextField::labelWithString(&NSString::from_str("🔋 --%"), mtm) };
        unsafe {
            battery_label.setFont(Some(&NSFont::systemFontOfSize(12.0)));
            header_row.addArrangedSubview(&battery_label);
            root_stack.addArrangedSubview(&header_row);
        }

        // 模式状态
        let mode_label = unsafe {
            NSTextField::labelWithString(&NSString::from_str("控制模式: 主机控制"), mtm)
        };
        unsafe {
            mode_label.setFont(Some(&NSFont::systemFontOfSize(11.0)));
            mode_label.setTextColor(Some(&NSColor::secondaryLabelColor()));
            root_stack.addArrangedSubview(&mode_label);
        }

        // 顶部选项卡：[ ⚡️ 性能与灯效 ] | [ 🖱️ 侧键宏 (G4-G11) ]
        let tab_labels = NSArray::from_vec(vec![
            NSString::from_str("⚡️ 性能与灯效"),
            NSString::from_str("🖱️ 侧键宏 (G4-G11)"),
        ]);
        let tab_control = unsafe {
            NSSegmentedControl::segmentedControlWithLabels_trackingMode_target_action(
                &tab_labels,
                NSSegmentSwitchTracking::SelectOne,
                Some(&dispatcher),
                Some(sel!(onMainTabChanged:)),
                mtm,
            )
        };
        unsafe {
            tab_control.setSelectedSegment(0);
            root_stack.addArrangedSubview(&tab_control);
            root_stack.addArrangedSubview(&Self::create_separator(mtm));
        }

        // ==================================================================== //
        // TAB 1: 性能与灯效堆叠 (perf_stack)
        // ==================================================================== //
        let perf_stack = unsafe { NSStackView::new(mtm) };
        unsafe {
            perf_stack.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
            perf_stack.setSpacing(8.0);
            root_stack.addArrangedSubview(&perf_stack);
        }

        // 2. DPI 调节卡片 (100 ~ 25600 滑块 + 实时数值 + 常用预设)
        let dpi_header_row = unsafe { NSStackView::new(mtm) };
        unsafe {
            dpi_header_row.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
            dpi_header_row.setSpacing(8.0);
        }
        let dpi_title =
            unsafe { NSTextField::labelWithString(&NSString::from_str("灵敏度 DPI"), mtm) };
        unsafe {
            dpi_title.setFont(Some(&NSFont::boldSystemFontOfSize(12.0)));
            dpi_header_row.addArrangedSubview(&dpi_title);
        }
        let dpi_value_label =
            unsafe { NSTextField::labelWithString(&NSString::from_str("1600 DPI"), mtm) };
        unsafe {
            dpi_value_label.setFont(Some(&NSFont::boldSystemFontOfSize(12.0)));
            dpi_value_label.setTextColor(Some(&NSColor::systemBlueColor()));
            dpi_header_row.addArrangedSubview(&dpi_value_label);
            perf_stack.addArrangedSubview(&dpi_header_row);
        }

        let dpi_slider_row = unsafe { NSStackView::new(mtm) };
        unsafe {
            dpi_slider_row.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
            dpi_slider_row.setSpacing(8.0);
        }
        let dpi_min_label =
            unsafe { NSTextField::labelWithString(&NSString::from_str("100"), mtm) };
        unsafe {
            dpi_min_label.setFont(Some(&NSFont::systemFontOfSize(10.0)));
            dpi_min_label.setTextColor(Some(&NSColor::secondaryLabelColor()));
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
            dpi_slider_row.addArrangedSubview(&dpi_slider);
        }
        let dpi_max_label =
            unsafe { NSTextField::labelWithString(&NSString::from_str("25600"), mtm) };
        unsafe {
            dpi_max_label.setFont(Some(&NSFont::systemFontOfSize(10.0)));
            dpi_max_label.setTextColor(Some(&NSColor::secondaryLabelColor()));
            dpi_slider_row.addArrangedSubview(&dpi_max_label);
            perf_stack.addArrangedSubview(&dpi_slider_row);
        }

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
            dpi_presets.setSelectedSegment(2); // 1600
            perf_stack.addArrangedSubview(&dpi_presets);
            perf_stack.addArrangedSubview(&Self::create_separator(mtm));
        }

        // 3. RGB 分区选择卡片
        let rgb_title =
            unsafe { NSTextField::labelWithString(&NSString::from_str("RGB 分区控制"), mtm) };
        unsafe {
            rgb_title.setFont(Some(&NSFont::boldSystemFontOfSize(12.0)));
            perf_stack.addArrangedSubview(&rgb_title);
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
            perf_stack.addArrangedSubview(&zone_control);
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
            perf_stack.addArrangedSubview(&effect_control);
        }

        // 5. 颜色预设网格 (10 个颜色按钮 + 选定回显)
        let color_header = unsafe { NSStackView::new(mtm) };
        unsafe {
            color_header.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
            color_header.setSpacing(6.0);
        }
        let color_title =
            unsafe { NSTextField::labelWithString(&NSString::from_str("预设色彩"), mtm) };
        unsafe {
            color_title.setFont(Some(&NSFont::boldSystemFontOfSize(12.0)));
            color_header.addArrangedSubview(&color_title);
        }
        let color_label =
            unsafe { NSTextField::labelWithString(&NSString::from_str("选定色彩: --"), mtm) };
        unsafe {
            color_label.setFont(Some(&NSFont::systemFontOfSize(11.0)));
            color_label.setTextColor(Some(&NSColor::secondaryLabelColor()));
            color_header.addArrangedSubview(&color_label);
            perf_stack.addArrangedSubview(&color_header);
        }

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
                btn.setButtonType(objc2_app_kit::NSButtonType::PushOnPushOff);
                btn.setTag(i as isize);
                btn.setFont(Some(&NSFont::systemFontOfSize(11.0)));
            }
            if i < 5 {
                unsafe {
                    color_row1.addArrangedSubview(&btn);
                }
            } else {
                unsafe {
                    color_row2.addArrangedSubview(&btn);
                }
            }
            color_buttons.push(btn);
        }
        unsafe {
            perf_stack.addArrangedSubview(&color_row1);
            perf_stack.addArrangedSubview(&color_row2);
            perf_stack.addArrangedSubview(&Self::create_separator(mtm));
        }

        // 6. 亮度调节滑块
        let bright_row = unsafe { NSStackView::new(mtm) };
        unsafe {
            bright_row.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
            bright_row.setSpacing(8.0);
        }
        let bright_title =
            unsafe { NSTextField::labelWithString(&NSString::from_str("亮度:"), mtm) };
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
        let brightness_label =
            unsafe { NSTextField::labelWithString(&NSString::from_str("100%"), mtm) };
        unsafe {
            brightness_label.setFont(Some(&NSFont::systemFontOfSize(11.0)));
            bright_row.addArrangedSubview(&brightness_label);
            perf_stack.addArrangedSubview(&bright_row);
        }

        // 7. 速率调节滑块 (1000ms~10000ms / 1.0s~10.0s)
        let rate_row = unsafe { NSStackView::new(mtm) };
        unsafe {
            rate_row.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
            rate_row.setSpacing(8.0);
        }
        let rate_title =
            unsafe { NSTextField::labelWithString(&NSString::from_str("速率:"), mtm) };
        unsafe {
            rate_title.setFont(Some(&NSFont::systemFontOfSize(12.0)));
            rate_row.addArrangedSubview(&rate_title);
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
            rate_row.addArrangedSubview(&rate_slider);
        }
        let rate_label =
            unsafe { NSTextField::labelWithString(&NSString::from_str("2.0s (中)"), mtm) };
        unsafe {
            rate_label.setFont(Some(&NSFont::systemFontOfSize(11.0)));
            rate_row.addArrangedSubview(&rate_label);
            perf_stack.addArrangedSubview(&rate_row);
        }

        // ==================================================================== //
        // TAB 2: 侧键宏堆叠 (macro_stack)
        // ==================================================================== //
        let macro_stack = unsafe { NSStackView::new(mtm) };
        unsafe {
            macro_stack.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
            macro_stack.setSpacing(6.0);
            macro_stack.setHidden(true); // 默认显示 Tab 0
            root_stack.addArrangedSubview(&macro_stack);
        }

        // 宏头部 (标题 + 引擎开关)
        let macro_header_row = unsafe { NSStackView::new(mtm) };
        unsafe {
            macro_header_row.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
            macro_header_row.setSpacing(8.0);
        }
        let macro_title =
            unsafe { NSTextField::labelWithString(&NSString::from_str("侧键宏设置"), mtm) };
        unsafe {
            macro_title.setFont(Some(&NSFont::boldSystemFontOfSize(12.0)));
            macro_header_row.addArrangedSubview(&macro_title);
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
            macro_toggle_btn.setFont(Some(&NSFont::systemFontOfSize(11.0)));
            macro_header_row.addArrangedSubview(&macro_toggle_btn);
            macro_stack.addArrangedSubview(&macro_header_row);
        }

        // 鼠标透视选择器: [ 顶部按键 (G7..G11) ] | [ 侧面按键 (G4..G6) ]
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
            macro_stack.addArrangedSubview(&mouse_view_control);
        }

        // 原生交互矢量画布:悬停/点击可识别 G4-G11,不依赖静态图片。
        let mouse_canvas = G502MouseCanvas::new(
            NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(panel_width - 32.0, 142.0),
            ),
            mtm,
        );
        unsafe {
            macro_stack.addArrangedSubview(&mouse_canvas);
            macro_stack.addArrangedSubview(&Self::create_separator(mtm));
        }

        // G4 到 G11 独立按键行列表。使用滚动区，避免矢量图与八行配置挤出面板。
        let macro_rows_stack = unsafe { NSStackView::new(mtm) };
        unsafe {
            macro_rows_stack.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
            macro_rows_stack.setSpacing(5.0);
            macro_rows_stack.setFrame(NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(panel_width - 48.0, 360.0),
            ));
        }
        let macro_scroll = unsafe {
            NSScrollView::initWithFrame(
                mtm.alloc(),
                NSRect::new(
                    NSPoint::new(0.0, 0.0),
                    NSSize::new(panel_width - 32.0, 210.0),
                ),
            )
        };
        unsafe {
            macro_scroll.setHasVerticalScroller(true);
            macro_scroll.setDrawsBackground(false);
            macro_scroll.setDocumentView(Some(&macro_rows_stack));
            macro_stack.addArrangedSubview(&macro_scroll);
        }

        let mut gkey_rows = Vec::new();
        for (i, gk) in crate::macro_engine::G_KEYS.iter().enumerate() {
            let row = unsafe { NSStackView::new(mtm) };
            unsafe {
                row.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
                row.setSpacing(4.0);
            }

            let label_str = format!("{}:", gk.name);
            let name_label =
                unsafe { NSTextField::labelWithString(&NSString::from_str(&label_str), mtm) };
            unsafe {
                name_label.setFont(Some(&NSFont::boldSystemFontOfSize(11.0)));
                row.addArrangedSubview(&name_label);
            }

            let editor_stack = unsafe { NSStackView::new(mtm) };
            unsafe {
                editor_stack.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
                editor_stack.setSpacing(1.0);
            }

            let summary_label =
                unsafe { NSTextField::labelWithString(&NSString::from_str("未绑定"), mtm) };
            unsafe {
                summary_label.setFont(Some(&NSFont::systemFontOfSize(9.5)));
                summary_label.setTextColor(Some(&NSColor::secondaryLabelColor()));
                editor_stack.addArrangedSubview(&summary_label);
            }

            let text_input = unsafe { NSTextField::new(mtm) };
            unsafe {
                text_input.setEditable(true);
                text_input.setSelectable(true);
                text_input.setBezeled(true);
                text_input.setFont(Some(&NSFont::systemFontOfSize(11.0)));
                text_input.setPlaceholderString(Some(&NSString::from_str("输入自动打字文本...")));
                text_input.setTarget(Some(&dispatcher));
                text_input.setAction(Some(sel!(onTextInputEnded:)));
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
                record_btn.setFont(Some(&NSFont::systemFontOfSize(10.5)));
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
                    btn.setFont(Some(&NSFont::systemFontOfSize(10.5)));
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
                clear_btn.setFont(Some(&NSFont::systemFontOfSize(10.5)));
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

        unsafe {
            macro_stack.addArrangedSubview(&Self::create_separator(mtm));
        }

        // 宏状态提示
        let macro_status_label = unsafe {
            NSTextField::labelWithString(
                &NSString::from_str("● 宏引擎就绪 (支持组合键与指定文字)"),
                mtm,
            )
        };
        unsafe {
            macro_status_label.setFont(Some(&NSFont::systemFontOfSize(10.5)));
            macro_status_label.setTextColor(Some(&NSColor::secondaryLabelColor()));
            macro_stack.addArrangedSubview(&macro_status_label);
        }

        // 宏录制辅助控制条
        let macro_action_row = unsafe { NSStackView::new(mtm) };
        unsafe {
            macro_action_row.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
            macro_action_row.setSpacing(6.0);
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
        let macro_rec_finish_btn = unsafe {
            NSButton::buttonWithTitle_target_action(
                &NSString::from_str("完成保存"),
                Some(&dispatcher),
                Some(sel!(onFinishRecording:)),
                mtm,
            )
        };
        unsafe {
            macro_rec_finish_btn.setFont(Some(&NSFont::systemFontOfSize(11.0)));
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
            macro_rec_cancel_btn.setFont(Some(&NSFont::systemFontOfSize(11.0)));
            macro_action_row.addArrangedSubview(&macro_rec_cancel_btn);
            macro_stack.addArrangedSubview(&macro_action_row);
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
            _tab_control: tab_control,
            perf_stack,
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

    unsafe fn create_separator(mtm: MainThreadMarker) -> Retained<NSBox> {
        let sep = NSBox::new(mtm);
        sep.setBoxType(NSBoxType::NSBoxSeparator);
        sep
    }

    pub fn switch_main_tab(tab_idx: usize) {
        HOLDER.with(|cell| {
            if let Some(h) = cell.borrow().as_ref() {
                if tab_idx == 0 {
                    h.perf_stack.setHidden(false);
                    h.macro_stack.setHidden(true);
                } else {
                    h.perf_stack.setHidden(true);
                    h.macro_stack.setHidden(false);
                }
            }
        });
        if tab_idx == 1 {
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
                    if let Some(row) = h.gkey_rows.iter().find(|row| row.key_id == gk.id) {
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
        std::thread::spawn(|| loop {
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
                            let in_x =
                                mouse.x >= f.origin.x && mouse.x <= f.origin.x + f.size.width;
                            let in_y =
                                mouse.y >= f.origin.y && mouse.y <= f.origin.y + f.size.height;
                            if !in_x || !in_y {
                                PopoverPanel::shared_hide();
                            }
                        }
                    });
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
            let name = crate::menubar::LED_COLORS[i].0;
            let is_selected = Some(i) == matching_idx && color_enabled;
            let title = if is_selected {
                format!("✓ {name}")
            } else {
                name.to_string()
            };
            unsafe {
                btn.setTitle(&NSString::from_str(&title));
                btn.setEnabled(color_enabled);
                btn.setState(if is_selected {
                    objc2_app_kit::NSControlStateValueOn
                } else {
                    objc2_app_kit::NSControlStateValueOff
                });
                btn.highlight(is_selected);
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
                    h.mode_label.setStringValue(&NSString::from_str(mode_text));
                    if let Some(d) = dpi {
                        let clamped = d.clamp(100, 25600);
                        h.active_dpi.set(clamped);
                        h.dpi_value_label
                            .setStringValue(&NSString::from_str(&format!("{clamped} DPI")));
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
                        let binding = crate::macro_engine::get_binding_for_key(row.key_id);
                        if let Some(b) = binding {
                            if let Some(name) = Self::gkey_name_for_id(row.key_id) {
                                bound_keys.insert(name.to_string());
                            }
                            if let Some(text) = crate::macro_engine::get_binding_text(&b) {
                                row.summary_label
                                    .setStringValue(&NSString::from_str("文字 · 自动输入"));
                                row.summary_label
                                    .setTextColor(Some(&NSColor::systemBlueColor()));
                                row.text_input.setEditable(true);
                                row.text_input.setStringValue(&NSString::from_str(&text));
                                row.text_input
                                    .setPlaceholderString(Some(&NSString::from_str(
                                        "输入自动打字文本...",
                                    )));
                                row.text_input.setTextColor(Some(&NSColor::labelColor()));
                            } else {
                                let summary = crate::macro_engine::format_binding_summary(&b);
                                row.summary_label
                                    .setStringValue(&NSString::from_str(&summary));
                                row.summary_label
                                    .setTextColor(Some(&NSColor::systemGreenColor()));
                                row.text_input.setEditable(false);
                                row.text_input.setStringValue(&NSString::from_str(""));
                                row.text_input
                                    .setPlaceholderString(Some(&NSString::from_str(
                                        "非文字动作；清空后可输入文字",
                                    )));
                                row.text_input
                                    .setTextColor(Some(&NSColor::secondaryLabelColor()));
                            }
                            row.clear_btn.setEnabled(true);
                        } else {
                            row.summary_label
                                .setStringValue(&NSString::from_str("未绑定"));
                            row.summary_label
                                .setTextColor(Some(&NSColor::secondaryLabelColor()));
                            row.text_input.setEditable(true);
                            row.text_input.setStringValue(&NSString::from_str(""));
                            row.text_input
                                .setPlaceholderString(Some(&NSString::from_str(
                                    "输入自动打字文本...",
                                )));
                            row.text_input.setTextColor(Some(&NSColor::labelColor()));
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
                            .setTextColor(Some(&NSColor::systemOrangeColor()));
                    } else {
                        h.macro_status_label
                            .setTextColor(Some(&NSColor::secondaryLabelColor()));
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
