use gpui_kit::Render;
use gpui_kit::component::*;
use gpui_kit::*;

pub struct VideoView {}

impl VideoView {
    pub fn new() -> Self {
        VideoView {}
    }
}

impl Render for VideoView {
    fn render(
        &mut self,
        window: &mut gpui_kit::Window,
        cx: &mut gpui_kit::prelude::Context<Self>,
    ) -> impl gpui_kit::prelude::IntoElement {
        div().child("hi")
    }
}
