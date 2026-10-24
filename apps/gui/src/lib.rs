//! The RustZIP desktop application.
//!
//! Everything that is not drawing lives in `rz-app`; this crate only turns that
//! into windows. Colours come from `brand`, never from the views.

pub mod assets;
pub mod brand;
pub mod launch;
pub mod recent;
pub mod rows;
pub mod services;
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
        ExtractAll,
        OpenSettings,
        // The right-click menu of the file list, and select-all.
        RowOpen,
        RowExtractTo,
        RowExtractHere,
        RowExtractNamed,
        RowCopyPath,
        RowAddHere,
        RowRename,
        RowDelete,
        SelectAllRows,
        // Right click on the empty part of the list.
        BlankAdd
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
        KeyBinding::new("secondary-,", OpenSettings, None),
        // In the window's own context, so a text box keeps its own select-all.
        KeyBinding::new("secondary-a", SelectAllRows, Some("RustZip")),
    ]);
    cx.set_menus(vec![
        Menu {
            name: "RustZIP".into(),
            items: vec![
                MenuItem::action("设置…", OpenSettings),
                MenuItem::separator(),
                MenuItem::action("退出 RustZIP", Quit),
            ],
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

/// The running main window, so later requests (a second launch, Finder's "Open With")
/// can be handed to it.
#[derive(Clone)]
struct MainRef {
    window: AnyWindowHandle,
    view: WeakEntity<MainView>,
}

impl Global for MainRef {}

/// Hands paths to the window that is already open; false when there is none yet.
pub fn open_in_running_window(paths: Vec<PathBuf>, cx: &mut App) -> bool {
    let Some(main) = cx.try_global::<MainRef>().cloned() else {
        return false;
    };
    let _ = main.window.update(cx, |_, window, cx| {
        if let Some(view) = main.view.upgrade() {
            view.update(cx, |m, cx| {
                for p in paths {
                    m.open_path(p, None, window, cx);
                }
            });
        }
        window.activate_window();
    });
    true
}

/// Starts forwarding `file://` addresses received from the system (macOS "Open With",
/// double click, drop on the Dock icon) to the main window.
pub fn listen_for_open_urls(app: &Application) -> std::sync::mpsc::Receiver<Vec<PathBuf>> {
    let (tx, rx) = std::sync::mpsc::channel::<Vec<PathBuf>>();
    app.on_open_urls(move |urls| {
        let paths: Vec<PathBuf> = urls
            .iter()
            .filter_map(|u| launch::file_url_to_path(u))
            .collect();
        if !paths.is_empty() {
            let _ = tx.send(paths);
        }
    });
    rx
}

/// Opens what [`listen_for_open_urls`] received. The system callback has no access to the
/// app, so the window picks the paths up from this polling task.
pub fn forward_open_urls(rx: std::sync::mpsc::Receiver<Vec<PathBuf>>, cx: &mut App) {
    cx.spawn(async move |cx| {
        loop {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(200))
                .await;
            while let Ok(paths) = rx.try_recv() {
                cx.update(|cx| {
                    if !open_in_running_window(paths.clone(), cx) {
                        open_main_window(
                            launch::Launch {
                                open: paths,
                                ..Default::default()
                            },
                            cx,
                        );
                    }
                });
            }
        }
    })
    .detach();
}

/// Opens the main window and does what the launch asked for.
pub fn open_main_window(launch: launch::Launch, cx: &mut App) {
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            size(px(900.), px(580.)),
            cx,
        ))),
        window_min_size: Some(size(px(640.), px(420.))),
        titlebar: Some(TitlebarOptions {
            title: Some("RustZIP".into()),
            // On macOS the tab strip sits in the title bar next to the window buttons.
            // Windows and Linux are unverified, so they keep the system title bar.
            appears_transparent: view::TABS_IN_TITLEBAR,
            traffic_light_position: view::TABS_IN_TITLEBAR.then(|| point(px(14.), px(13.))),
        }),
        ..WindowOptions::default()
    };
    let opened = gpui_kit::open_window(options, cx, move |window, cx| {
        view::init_window(window, cx);
        let path = settings_path();
        let settings = path.as_deref().map(Settings::load).unwrap_or_default();
        let main = cx.new(|cx| MainView::new(settings, path, window, cx));
        cx.set_global(MainRef {
            window: window.window_handle(),
            view: main.downgrade(),
        });
        main.update(cx, |m, cx| {
            let pick = m.settings.theme;
            brand::set_accent(&m.settings.accent, cx);
            brand::choose(pick, window, cx);
            // Passwords go to the system keychain when it works; if not, nothing is kept.
            if rz_services::KeychainStore::available() {
                m.vault = std::sync::Arc::new(rz_services::KeychainStore);
            }
            m.show_first_run_notice(cx);
            m.start_background_services(cx);
        });
        for p in launch.open {
            main.update(cx, |m, cx| m.open_path(p, None, window, cx));
        }
        for p in launch.extract_here {
            main.update(cx, |m, cx| m.extract_here(p, cx));
        }
        if !launch.compress.is_empty() {
            main.update(cx, |m, cx| m.open_compress(launch.compress, window, cx));
        }
        cx.new(|cx| Root::new(main, window, cx))
    });
    if let Err(e) = opened {
        eprintln!("RustZIP could not open its window: {e}");
        cx.quit();
    }
}
