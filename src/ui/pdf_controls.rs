use gpui_kit::assets::IconName;
use gpui_kit::component::button::*;
use gpui_kit::component::input::{InputEvent, InputState, NumberInput};
use gpui_kit::component::*;
use gpui_kit::*;

use super::controls::setting_toggle;
use super::pdf_view::{DEFAULT_ZOOM, MAX_ZOOM, MIN_ZOOM, ZOOM_STEP};
use crate::Settings;

pub enum PdfControlsBarEvent {
    /// The user entered this zoom level, clamped to the allowed range.
    ZoomChanged(f32),
    /// The user pressed Enter in the zoom input.
    ZoomConfirmed,
    /// The user pressed the fit to page button.
    FitPage,
    /// The user pressed the rotate page button.
    RotatePage,
}

/// The bar below the pages, with the zoom input and the page buttons. It only shows
/// the zoom level; `PdfView` owns it.
pub struct PdfControlsBar {
    input: Entity<InputState>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<PdfControlsBarEvent> for PdfControlsBar {}

impl PdfControlsBar {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(DEFAULT_ZOOM.to_string())
                .step(ZOOM_STEP as f64)
                .min(MIN_ZOOM as f64)
                .max(MAX_ZOOM as f64)
                .validate(|text, _| text.chars().all(|c| c.is_ascii_digit()))
        });
        let _subscriptions = vec![
            cx.subscribe_in(&input, window, Self::on_input_event),
            // Shows whether the video follows the slides.
            cx.observe_global::<Settings>(|_, cx| cx.notify()),
        ];

        Self {
            input,
            _subscriptions,
        }
    }

    /// Shows a zoom level set elsewhere (e.g. Ctrl+scroll), without emitting
    /// `ZoomChanged`.
    pub fn show_zoom(&mut self, zoom_percent: f32, window: &mut Window, cx: &mut Context<Self>) {
        // The input only takes whole numbers.
        let text = zoom_percent.round().to_string();
        // `set_value` doesn't emit `InputEvent::Change`, so this doesn't loop back.
        self.input
            .update(cx, |input, cx| input.set_value(text, window, cx));
    }

    fn on_input_event(
        &mut self,
        input: &Entity<InputState>,
        event: &InputEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            InputEvent::Change => {
                if let Ok(zoom) = input.read(cx).value().trim().parse::<f32>() {
                    cx.emit(PdfControlsBarEvent::ZoomChanged(
                        zoom.clamp(MIN_ZOOM, MAX_ZOOM),
                    ));
                }
            }
            InputEvent::PressEnter { .. } => cx.emit(PdfControlsBarEvent::ZoomConfirmed),
            _ => {}
        }
    }
}

impl Render for PdfControlsBar {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .flex_shrink_0()
            .justify_end()
            .items_center()
            .gap_2()
            .px_2()
            .py_1()
            .border_t_1()
            .border_color(cx.theme().border)
            .child("Zoom")
            .child(
                div()
                    .flex_none()
                    .w(px(120.))
                    .child(NumberInput::new(&self.input).small().suffix("%")),
            )
            .child(
                Button::new("fit-page")
                    .ghost()
                    .small()
                    .icon(IconName::Fullscreen)
                    .tooltip("Fit page")
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(PdfControlsBarEvent::FitPage))),
            )
            .child(
                Button::new("rotate-page")
                    .ghost()
                    .small()
                    .icon(IconName::RotateCw)
                    .tooltip("Rotate page clockwise")
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(PdfControlsBarEvent::RotatePage))),
            )
            .child(
                setting_toggle(
                    "video-follows-slides",
                    IconName::SquarePlay,
                    "Seek the video to the page scrolled to",
                    |settings| &mut settings.video_follows_slides,
                    cx,
                )
                .small(),
            )
    }
}
