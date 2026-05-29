//! Root view: header (device carousel), body (mouse model + side config),
//! footer (settings / version).
//!
//! The body is now arranged per UI.md §1: the [`MouseModelView`] sits on
//! the left, and the right column stacks the DPI panel + gesture pad
//! (placeholders for the eventual multi-tab config panel).

use gpui::{
    AnyElement, AppContext as _, Context, Entity, FontWeight, InteractiveElement, IntoElement,
    ParentElement, Render, StatefulInteractiveElement as _, Styled, Subscription, Window, div, px,
    rgb,
};
use gpui_component::{h_flex, v_flex};
use openlogi_core::config::Config;
use openlogi_core::device::DeviceInventory;
use tracing::{info, warn};

use crate::app_menu::{Minimize, Zoom};
use crate::asset::AssetCache;
use crate::components::device_carousel::DeviceCarousel;
use crate::components::dpi_panel::DpiPanel;
use crate::components::gesture_pad::GesturePad;
use crate::mouse_model::view::MouseModelView;
use crate::state::AppState;
use crate::theme::{self, FOOTER_H, HEADER_H, Palette};

pub struct AppView {
    carousel: Entity<DeviceCarousel>,
    mouse_model: Entity<MouseModelView>,
    dpi_panel: Entity<DpiPanel>,
    gesture_pad: Entity<GesturePad>,
    /// Keeps the OS-appearance observer alive for the window's lifetime.
    /// Set once by `main` right after the view is constructed (it needs the
    /// `Window`, which `new` doesn't have). Never read — held only so the
    /// subscription isn't dropped.
    #[allow(dead_code, reason = "held to keep the appearance observer alive")]
    appearance_obs: Option<Subscription>,
    /// Session-only "skip" for the Accessibility permission gate, so a user
    /// who only wants the HID++ features (DPI, SmartShift) isn't trapped on
    /// the gate. Resets on relaunch; ignored once permission is granted.
    accessibility_dismissed: bool,
}

impl AppView {
    pub fn new(inventories: &[DeviceInventory], cx: &mut Context<Self>) -> Self {
        // Load persisted config first so the initial AppState reflects any
        // saved bindings + the last-selected device. Malformed/unreadable
        // files fall back to defaults with a warning rather than crash.
        let config = match Config::load_or_default() {
            Ok(c) => c,
            Err(e) => {
                warn!(error = %e, "could not load config.toml — starting with defaults");
                Config::default()
            }
        };

        let cache = AssetCache::new();

        if !cx.has_global::<AppState>() {
            cx.set_global(AppState::with_runtime(config, inventories, &cache));
        }

        if let Some(state) = cx.try_global::<AppState>() {
            if let Some(record) = state.current_record() {
                info!(
                    device_key = %record.config_key,
                    display = %record.display_name,
                    "initial device selected"
                );
            } else {
                info!(
                    root = ?cache.cache_root(),
                    "no devices with HID++ model info — using synthetic silhouette"
                );
            }
        }

        let carousel = cx.new(|cx| DeviceCarousel::new(inventories, cx));
        let mouse_model = cx.new(MouseModelView::new);
        let dpi_panel = cx.new(DpiPanel::new);
        let gesture_pad = cx.new(GesturePad::new);
        Self {
            carousel,
            mouse_model,
            dpi_panel,
            gesture_pad,
            appearance_obs: None,
            accessibility_dismissed: false,
        }
    }

    /// Park the OS-appearance observer here so it outlives the call that
    /// created it. Called once from `main` after the window exists.
    pub fn set_appearance_obs(&mut self, sub: Subscription) {
        self.appearance_obs = Some(sub);
    }

    /// Full-window onboarding shown while macOS Accessibility is ungranted.
    /// Explains why the permission is needed, links to System Settings, and
    /// offers a session skip. The drain loop flips `accessibility_granted`
    /// (and repaints) the moment the user toggles the checkbox, so this
    /// disappears on its own.
    fn accessibility_gate(pal: Palette, cx: &mut Context<Self>) -> AnyElement {
        v_flex()
            .size_full()
            .bg(pal.bg)
            .text_color(pal.text_primary)
            .items_center()
            .justify_center()
            .gap_4()
            .p_8()
            .child(
                div()
                    .text_xl()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("需要「辅助功能」权限"),
            )
            .child(
                div()
                    .max_w(px(440.))
                    .text_sm()
                    .text_color(pal.text_muted)
                    .child(
                        "OpenLogi 通过系统的「辅助功能」权限捕获鼠标按键(后退 / 前进 / \
                         手势按钮)并执行你绑定的操作。DPI、SmartShift 等直接与设备通信的 \
                         功能不受影响。",
                    ),
            )
            .child(
                div()
                    .id("open-accessibility")
                    .px_4()
                    .py_2()
                    .rounded_md()
                    .bg(rgb(theme::ACCENT_BLUE))
                    .text_color(rgb(0x00ff_ffff))
                    .font_weight(FontWeight::MEDIUM)
                    .cursor_pointer()
                    .child("打开系统设置授权")
                    .on_click(|_, _, _| open_accessibility_settings()),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(pal.text_muted)
                    .child("授权后会自动生效,无需重启。"),
            )
            .child(
                div()
                    .id("skip-accessibility")
                    .text_xs()
                    .text_color(pal.text_muted)
                    .cursor_pointer()
                    .hover(|s| s.text_color(pal.text_primary))
                    .child("稍后再说(仅使用 DPI 等功能)")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.accessibility_dismissed = true;
                        cx.notify();
                    })),
            )
            .into_any_element()
    }
}

/// Raise the macOS Accessibility prompt (registers the app in the list) and
/// open the matching System Settings pane. macOS-only by construction — the
/// gate that calls this never shows on platforms where permission is implicit.
fn open_accessibility_settings() {
    openlogi_hook::Hook::prompt_accessibility();
    if let Err(e) = std::process::Command::new("open")
        .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility")
        .spawn()
    {
        warn!(error = %e, "could not open System Settings");
    }
}

impl Render for AppView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pal = theme::palette(cx);

        // Gate the whole UI on Accessibility until granted (or skipped this
        // session). Without it the OS hook can't capture any button, so
        // remapping silently does nothing — the gate explains why and links
        // straight to System Settings.
        let granted = cx
            .try_global::<AppState>()
            .is_none_or(|s| s.accessibility_granted);
        if !granted && !self.accessibility_dismissed {
            return Self::accessibility_gate(pal, cx);
        }

        v_flex()
            .size_full()
            .bg(pal.bg)
            .text_color(pal.text_primary)
            .on_action(|_: &Minimize, window, _| window.minimize_window())
            .on_action(|_: &Zoom, window, _| window.zoom_window())
            .child(header(&self.carousel, pal))
            .child(body(
                &self.mouse_model,
                &self.dpi_panel,
                &self.gesture_pad,
                pal,
            ))
            .child(footer(pal))
            .into_any_element()
    }
}

fn header(carousel: &Entity<DeviceCarousel>, pal: Palette) -> impl IntoElement {
    h_flex()
        .h(px(HEADER_H))
        .w_full()
        .px_5()
        .gap_4()
        .items_center()
        .border_b_1()
        .border_color(pal.border)
        .child(
            div()
                .text_lg()
                .font_weight(FontWeight::SEMIBOLD)
                .child("OpenLogi"),
        )
        .child(div().flex_1().min_w_0().child(carousel.clone()))
}

fn body(
    mouse_model: &Entity<MouseModelView>,
    dpi_panel: &Entity<DpiPanel>,
    gesture_pad: &Entity<GesturePad>,
    pal: Palette,
) -> impl IntoElement {
    h_flex()
        .flex_1()
        .w_full()
        .min_h_0()
        .items_start()
        .justify_center()
        .gap_6()
        .p_6()
        .child(mouse_model.clone())
        .child(
            v_flex()
                .gap_6()
                .child(dpi_panel.clone())
                .child(panel_label("Gestures", pal))
                .child(gesture_pad.clone()),
        )
}

fn panel_label(text: &'static str, pal: Palette) -> impl IntoElement {
    div().text_sm().text_color(pal.text_muted).child(text)
}

fn footer(pal: Palette) -> impl IntoElement {
    h_flex()
        .h(px(FOOTER_H))
        .w_full()
        .px_5()
        .gap_4()
        .items_center()
        .justify_between()
        .border_t_1()
        .border_color(pal.border)
        .child(
            div()
                .text_xs()
                .text_color(pal.text_muted)
                .child("Settings · About"),
        )
        .child(
            div()
                .text_xs()
                .text_color(pal.text_muted)
                .child(concat!("v", env!("CARGO_PKG_VERSION"))),
        )
}
