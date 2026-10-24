//! The frame every dialog is built in: an icon tile and a title on top, the content, and
//! a footer strip with the buttons. Only the content differs from dialog to dialog.

use crate::brand::Palette;
use gpui_kit::component::{Icon, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

/// What a dialog is about, which sets the colour of its icon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    /// Something that cannot be undone.
    Danger,
    /// An ordinary question or form.
    Neutral,
}

pub struct Dialog {
    pub tone: Tone,
    pub icon: Icon,
    pub title: String,
    /// One line under the title that says what the dialog is about.
    pub lead: Option<String>,
    /// The dialog's own content, below the heading.
    pub body: Option<AnyElement>,
    /// Buttons, left to right; the main one last.
    pub buttons: Vec<AnyElement>,
}

impl Dialog {
    pub fn render(self, p: &Palette) -> AnyElement {
        let (tint, ink) = match self.tone {
            Tone::Danger => (p.danger.opacity(0.12), p.danger),
            Tone::Neutral => (p.accent_soft, p.accent),
        };
        v_flex()
            .child(
                v_flex()
                    .gap_4()
                    .px_5()
                    .pt_5()
                    .pb_5()
                    .child(
                        h_flex()
                            .gap_4()
                            .items_start()
                            .child(
                                div()
                                    .size(px(40.))
                                    .flex_none()
                                    .rounded(px(12.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .bg(tint)
                                    .text_color(ink)
                                    .child(self.icon.size_5()),
                            )
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .gap_1()
                                    .child(
                                        div().text_size(px(16.)).text_color(p.fg).child(self.title),
                                    )
                                    .when_some(self.lead, |d, lead| {
                                        d.child(
                                            div()
                                                .text_size(px(13.))
                                                .line_height(px(20.))
                                                .text_color(p.fg2)
                                                .child(lead),
                                        )
                                    }),
                            ),
                    )
                    .when_some(self.body, |d, body| d.child(body)),
            )
            .child(
                h_flex()
                    .justify_end()
                    .gap_2()
                    .px_5()
                    .py_3()
                    .bg(p.panel)
                    .border_t_1()
                    .border_color(p.line)
                    .children(self.buttons),
            )
            .into_any_element()
    }
}
