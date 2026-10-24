//! The RustZIP desktop application.
//!
//! Everything that is not drawing lives in `rz-app`; this crate only turns that
//! into windows. Colours come from `brand`, never from the views.

pub mod brand;
pub mod rows;
pub mod view;

use gpui_kit::component::Root;
use gpui_kit::*;
use rz_app::settings::{Settings, config_dir};
use std::path::PathBuf;
use view::MainView;

actions!(
    rustzip,
    [
        Quit,
        OpenArchiveDialog,
        NewArchiveDialog,
        FocusSearch,
        CloseTab,
        ExtractAll
    ]
);

/// Key bindings and the menu bar. `secondary` is Cmd on macOS and Ctrl elsewhere.
pub fn install_app_actions(cx: &mut App) {
    cx.on_action(|_: &Quit, cx| cx.quit());
    cx.bind_keys([
        KeyBinding::new("secondary-q", Quit, None),
        KeyBinding::new("secondary-o", OpenArchiveDialog, None),
        KeyBinding::new("secondary-n", NewArchiveDialog, None),
        KeyBinding::new("secondary-f", FocusSearch, None),
        KeyBinding::new("secondary-w", CloseTab, None),
        KeyBinding::new("secondary-e", ExtractAll, None),
    ]);
    cx.set_menus(vec![
        Menu {
            name: "RustZIP".into(),
            items: vec![MenuItem::action("退出 RustZIP", Quit)],
            disabled: false,
        },
        Menu {
            name: "文件".into(),
            items: vec![
                MenuItem::action("打开压缩包…", OpenArchiveDialog),
                MenuItem::action("新建压缩…", NewArchiveDialog),
                MenuItem::separator(),
                MenuItem::action("解压全部…", ExtractAll),
                MenuItem::action("在压缩包内搜索", FocusSearch),
                MenuItem::separator(),
                MenuItem::action("关闭标签页", CloseTab),
            ],
            disabled: false,
        },
    ]);
    // Closing the last window ends the program, as users expect from a tool window.
    cx.on_window_closed(|cx, _| {
        if cx.windows().is_empty() {
            cx.quit();
        }
    })
    .detach();
}

/// A modification time as the user's clock would show it.
pub fn local_time(unix: i64) -> String {
    use chrono::{Local, Offset, TimeZone};
    let offset = Local
        .timestamp_opt(unix, 0)
        .single()
        .map_or(0, |t| t.offset().fix().local_minus_utc());
    rz_app::browse::format_time_at(unix, offset)
}

/// Where the settings file lives, when the system has a config directory.
pub fn settings_path() -> Option<PathBuf> {
    config_dir().map(|d| d.join("settings.json"))
}

/// Opens the main window. `initial` are archives to open right away.
pub fn open_main_window(initial: Vec<PathBuf>, cx: &mut App) {
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            size(px(900.), px(580.)),
            cx,
        ))),
        window_min_size: Some(size(px(640.), px(420.))),
        titlebar: Some(TitlebarOptions {
            title: Some("RustZIP".into()),
            ..TitlebarOptions::default()
        }),
        ..WindowOptions::default()
    };
    let opened = gpui_kit::open_window(options, cx, move |window, cx| {
        view::init_window(window, cx);
        let path = settings_path();
        let settings = path.as_deref().map(Settings::load).unwrap_or_default();
        let main = cx.new(|cx| MainView::new(settings, path, window, cx));
        for p in initial {
            main.update(cx, |m, cx| m.open_path(p, None, window, cx));
        }
        cx.new(|cx| Root::new(main, window, cx))
    });
    if let Err(e) = opened {
        eprintln!("RustZIP could not open its window: {e}");
        cx.quit();
    }
}
