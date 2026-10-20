use gpui_kit::assets::IconName;
use gpui_kit::component::button::*;
use gpui_kit::component::input::{InputEvent, InputState, NumberInput};
use gpui_kit::component::*;
use gpui_kit::*;

pub const DEFAULT_ZOOM: f32 = 100.;
pub const MIN_ZOOM: f32 = 10.;
pub const MAX_ZOOM: f32 = 400.;
pub const ZOOM_STEP: f32 = 10.;

pub enum PdfControlsBarEvent {
    /// The zoom level changed to the provided value.
    ZoomChanged(f32),
    /// The user pressed Enter in the zoom input.
    ZoomConfirmed,
    /// The user pressed the fit to page button.
    FitPage,
    /// The user pressed the rotate page button.
    RotatePage,
}

/// The bottom bar with the zoom input and the page buttons.
pub struct PdfControlsBar {
    zoom_percent: f32,
    input: Entity<InputState>,
    _subscription: Subscription,
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
        let _subscription = cx.subscribe_in(&input, window, Self::on_input_event);

        Self {
            zoom_percent: DEFAULT_ZOOM,
            input,
            _subscription,
        }
    }

    pub fn zoom_percent(&self) -> f32 {
        self.zoom_percent
    }

    /// Shows a zoom level set elsewhere (e.g. Ctrl+scroll), without emitting `Changed`.
    pub fn set_zoom_percent(
        &mut self,
        zoom_percent: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.zoom_percent = zoom_percent;
        // The input only takes whole numbers; the exact value is kept in `zoom_percent`.
        let text = zoom_percent.round().to_string();
        // `set_value` doesn't emit `InputEvent::Change`, so this doesn't loop back.
        self.input
            .update(cx, |input, cx| input.set_value(text, window, cx));
        cx.notify();
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
                    self.zoom_percent = zoom.clamp(MIN_ZOOM, MAX_ZOOM);
                    cx.emit(PdfControlsBarEvent::ZoomChanged(self.zoom_percent));
                    cx.notify();
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
    }
}
