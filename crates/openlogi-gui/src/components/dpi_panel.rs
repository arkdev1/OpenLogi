//! DPI slider for the right-side config column.
//!
//! Just a label + numeric value + horizontal slider that writes to
//! [`AppState::dpi`]. The earlier "preview dot" was Phase 2 scaffolding
//! to validate `with_animation`; once the rest of the UI shipped it
//! added perpetual motion to a settings surface that should sit still.
//!
//! Wiring the DPI value to the hardware (HID++ `AdjustableDpi` feature
//! 0x2201) is a separate task — today the slider only mutates the in-
//! process [`AppState`], so other panels can react but the mouse itself
//! doesn't change DPI.

use gpui::{
    AnyElement, AppContext as _, BorrowAppContext as _, Context, Entity, InteractiveElement,
    IntoElement, ParentElement, Render, StatefulInteractiveElement as _, Styled, Subscription,
    Window, div, px, rgb,
};
use gpui_component::{
    ActiveTheme, h_flex,
    slider::{Slider, SliderEvent, SliderState},
    v_flex,
};
use tracing::debug;

use crate::hardware::write_dpi_in_background;
use crate::state::AppState;
use crate::theme::{ACCENT_BLUE, BORDER, SURFACE, SURFACE_HOVER, TEXT_MUTED, TEXT_PRIMARY};

/// Identifies which physical device the slider should write DPI to.
/// `receiver_uid` is the Bolt receiver's unique id (so we route writes
/// correctly when more than one receiver is plugged in); `slot` is the
/// device's pairing slot on that receiver.
#[derive(Debug, Clone)]
pub struct DpiTarget {
    pub receiver_uid: String,
    pub slot: u8,
}

/// Slider column width. Matches the right-column layout in `app.rs`.
const PANEL_W: f32 = 300.;

const MIN_DPI: f32 = 200.;
const MAX_DPI: f32 = 6400.;
const STEP_DPI: f32 = 50.;

pub struct DpiPanel {
    slider_state: Entity<SliderState>,
    _slider_sub: Subscription,
    _state_obs: Subscription,
}

impl DpiPanel {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let initial_dpi = dpi_to_f32(
            cx.try_global::<AppState>()
                .map_or(crate::state::DEFAULT_DPI, |s| s.dpi),
        );

        // Order matters: `SliderState` defaults to max=100, and `.min(N)`
        // clamps the value against the current max. Setting max=6400
        // first keeps the intermediate state coherent.
        let slider_state = cx.new(|_| {
            SliderState::new()
                .max(MAX_DPI)
                .min(MIN_DPI)
                .step(STEP_DPI)
                .default_value(initial_dpi)
        });

        let slider_sub =
            cx.subscribe(
                &slider_state,
                |_panel, _slider, event: &SliderEvent, cx| match event {
                    // Continuous Change drives the in-process state so the
                    // numeric label tracks the drag. The HID write happens
                    // once on Release to keep us from spamming the device
                    // with intermediate values.
                    SliderEvent::Change(value) => {
                        let dpi = clamp_dpi(value.start());
                        debug!(dpi, "slider change → AppState.dpi");
                        cx.update_global::<AppState, _>(|state, _| state.dpi = dpi);
                        cx.notify();
                    }
                    SliderEvent::Release(value) => {
                        let dpi = clamp_dpi(value.start());
                        // Resolve the target from AppState at fire-time so
                        // carousel-driven device switches route the write to
                        // the now-current device, not whichever was active
                        // when the panel was constructed.
                        let target = cx
                            .try_global::<AppState>()
                            .and_then(|s| s.current_record().and_then(|r| r.dpi_target.clone()));
                        write_dpi_in_background(target, dpi);
                    }
                },
            );

        // Repaint when the carousel switches devices so the label tracks
        // the new device's last DPI (the slider thumb stays put — sliding
        // to a different value will write to the now-current device).
        let state_obs = cx.observe_global::<AppState>(|_panel, cx| cx.notify());

        Self {
            slider_state,
            _slider_sub: slider_sub,
            _state_obs: state_obs,
        }
    }
}

impl Render for DpiPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (dpi, presets) = cx
            .try_global::<AppState>()
            .map(|s| (s.dpi, s.dpi_presets()))
            .unwrap_or((crate::state::DEFAULT_DPI, Vec::new()));
        let theme = cx.theme();

        let preset_chips: Vec<AnyElement> = presets
            .iter()
            .enumerate()
            .map(|(idx, value)| preset_chip(idx, *value, *value == dpi, presets.clone()))
            .collect();

        v_flex()
            .gap_3()
            .w(px(PANEL_W))
            .child(
                h_flex()
                    .justify_between()
                    .items_baseline()
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .child("DPI"),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(ACCENT_BLUE))
                            .child(format!("{dpi}")),
                    ),
            )
            .child(Slider::new(&self.slider_state).horizontal())
            .child(
                v_flex()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(TEXT_MUTED))
                            .child("PRESETS"),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .flex_wrap()
                            .children(preset_chips)
                            .child(add_preset_chip()),
                    ),
            )
    }
}

const CHIP_H: f32 = 28.;

/// One DPI preset rendered as a chip. Clicking the chip writes that DPI to
/// the device and updates `AppState.dpi`; the small × removes the preset.
fn preset_chip(idx: usize, value: u32, active: bool, presets: Vec<u32>) -> AnyElement {
    let presets_for_remove = presets.clone();
    h_flex()
        .id(("dpi-preset-chip", idx))
        .h(px(CHIP_H))
        .px_2()
        .gap_2()
        .items_center()
        .rounded_md()
        .border_1()
        .border_color(rgb(if active { ACCENT_BLUE } else { BORDER }))
        .bg(rgb(if active { SURFACE_HOVER } else { SURFACE }))
        .hover(|s| s.bg(rgb(SURFACE_HOVER)))
        .child(
            div()
                .id(("dpi-preset-apply", idx))
                .text_sm()
                .text_color(rgb(if active { ACCENT_BLUE } else { TEXT_PRIMARY }))
                .child(format!("{value}"))
                .on_click(move |_event, _window, cx| {
                    let target = cx
                        .try_global::<AppState>()
                        .and_then(|s| s.current_record().and_then(|r| r.dpi_target.clone()));
                    cx.update_global::<AppState, _>(|state, _| state.dpi = value);
                    write_dpi_in_background(target, value);
                    cx.refresh_windows();
                }),
        )
        .child(
            div()
                .id(("dpi-preset-remove", idx))
                .text_xs()
                .text_color(rgb(TEXT_MUTED))
                .child("×")
                .on_click(move |_event, _window, cx| {
                    let mut next = presets_for_remove.clone();
                    if idx < next.len() {
                        next.remove(idx);
                    }
                    cx.update_global::<AppState, _>(|state, _| state.commit_dpi_presets(next));
                    cx.refresh_windows();
                }),
        )
        .into_any_element()
}

/// "+" chip that snapshots `AppState.dpi` as a new preset.
fn add_preset_chip() -> AnyElement {
    h_flex()
        .id("dpi-preset-add")
        .h(px(CHIP_H))
        .px_3()
        .items_center()
        .rounded_md()
        .border_1()
        .border_color(rgb(BORDER))
        .bg(rgb(SURFACE))
        .hover(|s| s.bg(rgb(SURFACE_HOVER)))
        .child(
            div()
                .text_sm()
                .text_color(rgb(TEXT_MUTED))
                .child("+ Add"),
        )
        .on_click(|_event, _window, cx| {
            // Append the current DPI to the active device's preset list.
            // Duplicates are allowed — the user might want the same value
            // appearing at multiple cycle positions for muscle-memory reasons.
            cx.update_global::<AppState, _>(|state, _| {
                let mut presets = state.dpi_presets();
                presets.push(state.dpi);
                state.commit_dpi_presets(presets);
            });
            cx.refresh_windows();
        })
        .into_any_element()
}

/// Snap a raw slider read to the discrete DPI step and clamp into range.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "value is rounded and clamped into [MIN_DPI, MAX_DPI] above the cast"
)]
fn clamp_dpi(raw: f32) -> u32 {
    raw.clamp(MIN_DPI, MAX_DPI).round() as u32
}

/// Widen a DPI count into f32 for slider math. DPI is ≤ 6400 so it fits
/// comfortably in f32's mantissa with no precision loss.
#[allow(
    clippy::cast_precision_loss,
    reason = "DPI ≤ 6400 — well below f32 mantissa precision"
)]
fn dpi_to_f32(dpi: u32) -> f32 {
    dpi as f32
}
