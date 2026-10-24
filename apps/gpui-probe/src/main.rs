//! Throwaway validation program for GPUI Kit (see docs/decisions.md, D3).
//!
//! It exists to answer three questions on real machines:
//!   1. Does Chinese / Japanese input (IME) work in a text box and a password box?
//!   2. Is a 1,000,000-row table smooth to scroll, and how much memory does it take?
//!   3. Does the window render correctly on this GPU / driver / OS?
//!
//! Run:  cargo run --release -p gpui-probe
//! Then follow the checklist printed in the window and on stdout.

use gpui_kit::component::button::*;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::table::{Column, DataTable, TableDelegate, TableState};
use gpui_kit::component::{Root, h_flex, v_flex};
use gpui_kit::*;
use std::time::Instant;

const ROWS: usize = 1_000_000;

struct Rows {
    columns: Vec<Column>,
}

impl Rows {
    fn new() -> Self {
        Self {
            columns: vec![
                Column::new("name", "名称").width(px(420.)),
                Column::new("size", "大小").width(px(140.)),
                Column::new("time", "修改时间").width(px(220.)),
            ],
        }
    }

    fn cell(row: usize, col: usize) -> String {
        match col {
            0 => format!("项目文档/测试文件_{row:07}.txt"),
            1 => format!("{} KB", (row * 7919) % 100_000 / 10),
            _ => format!(
                "2026-{:02}-{:02} {:02}:{:02}",
                row % 12 + 1,
                row % 28 + 1,
                row % 24,
                row % 60
            ),
        }
    }
}

impl TableDelegate for Rows {
    fn columns_count(&self, _: &App) -> usize {
        self.columns.len()
    }

    fn rows_count(&self, _: &App) -> usize {
        ROWS
    }

    fn column(&self, ix: usize, _: &App) -> Column {
        self.columns[ix].clone()
    }

    fn render_td(
        &mut self,
        row: usize,
        col: usize,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        Self::cell(row, col)
    }
}

/// Counts frames per second of the window as seen by this view.
struct Fps {
    frames: u32,
    since: Instant,
    shown: u32,
}

impl Fps {
    fn tick(&mut self) -> u32 {
        self.frames += 1;
        if self.since.elapsed().as_secs_f32() >= 1.0 {
            self.shown = self.frames;
            self.frames = 0;
            self.since = Instant::now();
            println!("[probe] {} frames in the last second", self.shown);
        }
        self.shown
    }
}

struct Probe {
    name: Entity<InputState>,
    password: Entity<InputState>,
    table: Entity<TableState<Rows>>,
    fps: Fps,
    stress: bool,
}

impl Probe {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let name =
            cx.new(|cx| InputState::new(window, cx).placeholder("在这里输入中文、日文或韩文"));
        let password = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("密码框")
                .masked(true)
        });
        let table = cx.new(|cx| TableState::new(Rows::new(), window, cx));
        // Re-render the echo line whenever either box changes.
        for state in [&name, &password] {
            cx.subscribe_in(state, window, |_, _, _: &InputEvent, _, cx| cx.notify())
                .detach();
        }
        Self {
            name,
            password,
            table,
            fps: Fps {
                frames: 0,
                since: Instant::now(),
                shown: 0,
            },
            stress: false,
        }
    }
}

impl Render for Probe {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let fps = self.fps.tick();
        if self.stress {
            // Ask for another frame right away so the frame rate is visible while scrolling.
            window.request_animation_frame();
        }
        let typed = self.name.read(cx).value();
        let secret_len = self.password.read(cx).value().chars().count();

        v_flex()
            .size_full()
            .p_4()
            .gap_3()
            .child(
                div()
                    .child("GPUI Kit 验证:1) 输入法  2) 百万行列表  3) 渲染")
                    .text_lg(),
            )
            .child("① 请用拼音、五笔、日文输入法输入,确认候选框位置正确、上屏文字正确、退格正常。")
            .child(Input::new(&self.name))
            .child(format!(
                "回显: {typed}   (共 {} 个字符)",
                typed.chars().count()
            ))
            .child("② 密码框:同样用输入法输入,确认字符被遮住,而且可以用眼睛按钮显示。")
            .child(Input::new(&self.password).mask_toggle())
            .child(format!("密码框字符数: {secret_len}"))
            .child("③ 下面是 100 万行的表格。拖动滚动条、滚轮、按 PageDown,观察是否流畅、内存是否合理。")
            .child(
                h_flex()
                    .gap_3()
                    .child(
                        Button::new("stress")
                            .label(if self.stress {
                                "停止持续重绘"
                            } else {
                                "开始持续重绘(测帧率)"
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.stress = !this.stress;
                                cx.notify();
                            })),
                    )
                    .child(format!("帧率: {fps} 帧/秒")),
            )
            .child(
                div()
                    .flex_1()
                    .min_h(px(200.))
                    .child(DataTable::new(&self.table)),
            )
    }
}

fn main() {
    println!("GPUI Kit probe. Checklist:");
    println!("  1. Type Chinese with pinyin / wubi and Japanese in the first box.");
    println!("  2. Type in the password box; toggle the eye button.");
    println!(
        "  3. Scroll the 1,000,000-row table; click 'start continuous redraw' and read the fps."
    );
    println!(
        "  4. Note: OS, GPU, driver, window system (X11 or Wayland), and anything that looks wrong."
    );

    gpui_kit::application().run(|cx| {
        gpui_kit::init(cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                None,
                size(px(1000.), px(760.)),
                cx,
            ))),
            ..WindowOptions::default()
        };
        let opened = gpui_kit::open_window(options, cx, |window, cx| {
            let view = cx.new(|cx| Probe::new(window, cx));
            cx.new(|cx| Root::new(view, window, cx))
        });
        if let Err(e) = opened {
            eprintln!("failed to open a window: {e}");
            cx.quit();
        }
        cx.activate(true);
    });
}
