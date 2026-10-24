#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Renders the main window offscreen and writes PNG files, so the look can be
//! checked without a person at the screen.
//!
//!     RZ_SHOT_DIR=/some/dir cargo run --release -p rustzip-gui --example screenshots
//!
//! It uses the real renderer (Metal on macOS), not a mock, so what it writes is what the
//! window draws. Fonts and the window frame of the operating system are not included.

use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::test::TestWindowExt;
use gpui_kit::{
    AppContext, Bounds, Entity, HeadlessAppContext, WindowHandle, assets::Assets, base::Root, px,
    size,
};
use rustzip_gui::view::{MainView, load_archive};
use rz_app::settings::Settings;
use rz_app::tasks::JobSpec;
use rz_core::{CancelToken, CreateOptions, NoProgress};
use rz_engine::{ExtractOptions, compress};
use rz_formats::OutputFormat;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const WIDTH: f32 = 900.;
const HEIGHT: f32 = 580.;

fn make_archive(dir: &Path) -> PathBuf {
    let src = dir.join("项目文档");
    for d in ["设计稿", "源码/核心", "源码/界面", "图片"] {
        fs::create_dir_all(src.join(d)).unwrap();
    }
    let files: [(&str, usize); 14] = [
        ("README.md", 2_400),
        ("需求说明书.docx", 482_000),
        ("设计稿/主窗口.png", 1_240_000),
        ("设计稿/新建压缩.png", 880_000),
        ("源码/核心/engine.rs", 36_000),
        ("源码/核心/formats.rs", 58_000),
        ("源码/界面/view.rs", 71_000),
        ("源码/界面/brand.rs", 6_100),
        ("图片/logo.svg", 4_100),
        ("图片/截图 01.jpg", 812_000),
        ("预算表.xlsx", 96_000),
        ("数据备份.sqlite", 9_800_000),
        ("演示.mp4", 24_000_000),
        ("notes.txt", 900),
    ];
    for (name, len) in files {
        let data: Vec<u8> = (0..len).map(|i| ((i * 31 + len) % 251) as u8).collect();
        fs::write(src.join(name), data).unwrap();
    }
    let out = dir.join("项目文档.zip");
    compress(
        &[src],
        &out,
        OutputFormat::Zip,
        &CreateOptions::default(),
        &CancelToken::new(),
        &mut NoProgress,
    )
    .unwrap();
    out
}

type Ctx = HeadlessAppContext;

fn window(
    cx: &mut Ctx,
    mode: ThemeMode,
    recent: Vec<PathBuf>,
) -> (WindowHandle<Root>, Entity<MainView>) {
    cx.update(|cx| {
        let (handle, content) = gpui_kit::open_window(
            gpui_kit::WindowOptions {
                window_bounds: Some(gpui_kit::WindowBounds::Windowed(Bounds {
                    origin: Default::default(),
                    size: size(px(WIDTH), px(HEIGHT)),
                })),
                focus: false,
                show: false,
                ..Default::default()
            },
            cx,
            move |window, cx| {
                Theme::change(mode, Some(window), cx);
                rustzip_gui::brand::apply(cx);
                let settings = Settings {
                    recent,
                    ..Settings::default()
                };
                cx.new(|cx| MainView::new(settings, None, window, cx))
            },
        )
        .unwrap();
        (handle.downcast::<Root>().unwrap(), content)
    })
}

fn shoot(cx: &mut Ctx, handle: WindowHandle<Root>, dir: &Path, name: &str) {
    cx.update_window(handle.into(), |_, window, cx| {
        window.refresh();
        window.render_frame(cx);
    })
    .unwrap();
    let image = cx.capture_screenshot(handle.into()).unwrap();
    let path = dir.join(format!("{name}.png"));
    image.save(&path).unwrap();
    println!("wrote {}", path.display());
}

fn act(
    cx: &mut Ctx,
    handle: WindowHandle<Root>,
    view: &Entity<MainView>,
    f: impl FnOnce(&mut MainView, &mut gpui_kit::Window, &mut gpui_kit::Context<MainView>),
) {
    cx.update_window(handle.into(), |_, window, cx| {
        view.update(cx, |v, cx| f(v, window, cx))
    })
    .unwrap();
}

fn main() {
    let Some(out_dir) = std::env::var_os("RZ_SHOT_DIR").map(PathBuf::from) else {
        eprintln!("set RZ_SHOT_DIR to a folder for the PNG files");
        return;
    };
    fs::create_dir_all(&out_dir).unwrap();
    let work = tempfile::tempdir().unwrap();
    let archive = make_archive(work.path());

    for (mode, tag) in [(ThemeMode::Light, "light"), (ThemeMode::Dark, "dark")] {
        let mut cx = HeadlessAppContext::with_platform(
            gpui_kit::platform::current_platform(true).text_system(),
            Arc::new(Assets),
            gpui_kit::platform::current_headless_renderer,
        );
        cx.update(gpui_kit::init);

        // 1. Nothing open yet.
        let (handle, view) = window(&mut cx, mode, vec![archive.clone()]);
        shoot(&mut cx, handle, &out_dir, &format!("{tag}-1-empty"));

        // 2. An archive open at its top level.
        let loaded = load_archive(&archive, None);
        let path = archive.clone();
        act(&mut cx, handle, &view, move |v, w, cx| {
            v.finish_open(path, None, loaded, w, cx);
        });
        shoot(&mut cx, handle, &out_dir, &format!("{tag}-2-browse"));

        // 3. Inside a folder with a row selected.
        act(&mut cx, handle, &view, |v, _, cx| {
            v.navigate("项目文档", cx);
        });
        let table = view.read_with(&cx, |v, _| v.table.clone());
        cx.update_window(handle.into(), |_, _, cx| {
            table.update(cx, |t, cx| t.set_selected_row(2, cx));
        })
        .unwrap();
        shoot(&mut cx, handle, &out_dir, &format!("{tag}-3-selected"));

        // 4. The compress form.
        let src = work.path().join("项目文档");
        act(&mut cx, handle, &view, move |v, w, cx| {
            v.open_compress(vec![src], w, cx);
        });
        shoot(&mut cx, handle, &out_dir, &format!("{tag}-4-compress"));
        act(&mut cx, handle, &view, |v, _, cx| v.close_overlay(cx));

        // 5. A task in progress (held so it stays visible) and a finished one.
        let archive2 = archive.clone();
        let dest = work.path().join("out");
        act(&mut cx, handle, &view, move |v, _, cx| {
            v.tasks.submit_paused(JobSpec::Extract {
                archive: archive2,
                dest,
                opts: ExtractOptions::default(),
                smart: false,
                encoding: None,
            });
            v.refresh_tasks(cx);
        });
        shoot(&mut cx, handle, &out_dir, &format!("{tag}-5-tasks"));

        // 6. The password prompt.
        act(&mut cx, handle, &view, |v, w, cx| {
            v.ask_password(
                rustzip_gui::view::PasswordTarget::Open("/x/secret.7z".into()),
                true,
                w,
                cx,
            );
        });
        shoot(&mut cx, handle, &out_dir, &format!("{tag}-6-password"));

        // 7. Settings and the first-run notice.
        act(&mut cx, handle, &view, |v, _, cx| {
            v.close_overlay(cx);
            v.open_settings(cx);
        });
        shoot(&mut cx, handle, &out_dir, &format!("{tag}-7-settings"));
        act(&mut cx, handle, &view, |v, _, cx| {
            v.close_overlay(cx);
            v.show_first_run_notice(cx);
        });
        shoot(&mut cx, handle, &out_dir, &format!("{tag}-8-privacy"));
    }
}
