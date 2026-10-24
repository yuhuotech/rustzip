#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Headless tests of the main window: a real window, real tables and inputs,
//! real archives. They check what the user could observe through state and layout;
//! pixels and the operating system's input methods need a person.

use gpui_kit::component::table::TableEvent;
use gpui_kit::component::{ActiveTheme, Theme, ThemeMode};
use gpui_kit::test::TestWindowExt;
use gpui_kit::{AppContext, Entity, TestAppContext, WindowHandle, base::Root};
use rustzip_gui::brand::{self, Palette};
use rustzip_gui::view::{Format, MainView, Notice, Overlay, PasswordTarget, Preset, load_archive};
use rz_app::settings::Settings;
use rz_app::tasks::{ErrorCode, JobSpec, TaskState};
use rz_core::{CancelToken, CreateOptions, NoProgress};
use rz_engine::{ExtractOptions, compress};
use rz_formats::OutputFormat;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

const WAIT: Duration = Duration::from_secs(30);

fn open_window(
    cx: &mut TestAppContext,
    settings_path: Option<PathBuf>,
) -> (WindowHandle<Root>, Entity<MainView>) {
    cx.update(gpui_kit::init);
    cx.update(|cx| {
        let (window, content) = gpui_kit::open_window(
            gpui_kit::WindowOptions {
                window_bounds: Some(gpui_kit::WindowBounds::Windowed(gpui_kit::Bounds {
                    origin: gpui_kit::Point::default(),
                    size: gpui_kit::size(gpui_kit::px(1180.), gpui_kit::px(760.)),
                })),
                ..Default::default()
            },
            cx,
            move |window, cx| {
                rustzip_gui::view::init_window(window, cx);
                let settings = settings_path
                    .as_deref()
                    .map(Settings::load)
                    .unwrap_or_default();
                cx.new(|cx| MainView::new(settings, settings_path, window, cx))
            },
        )
        .expect("open window");
        (window.downcast::<Root>().expect("Root"), content)
    })
}

/// Runs `f` against the view with a window, then draws one frame.
fn with_view<R>(
    cx: &mut TestAppContext,
    handle: WindowHandle<Root>,
    view: &Entity<MainView>,
    f: impl FnOnce(&mut MainView, &mut gpui_kit::Window, &mut gpui_kit::Context<MainView>) -> R,
) -> R {
    let out = cx
        .update_window(handle.into(), |_, window, cx| {
            view.update(cx, |v, cx| f(v, window, cx))
        })
        .unwrap();
    draw(cx, handle);
    out
}

fn draw(cx: &mut TestAppContext, handle: WindowHandle<Root>) {
    cx.update_window(handle.into(), |_, window, cx| {
        window.refresh();
        window.draw(cx).clear(cx);
    })
    .unwrap();
}

fn make_archive(dir: &Path, name: &str, password: Option<&str>) -> PathBuf {
    let src = dir.join("proj");
    fs::create_dir_all(src.join("docs")).unwrap();
    fs::create_dir_all(src.join("src")).unwrap();
    fs::write(src.join("readme.txt"), "hello rustzip").unwrap();
    fs::write(src.join("docs/说明.md"), "# 中文").unwrap();
    fs::write(src.join("src/main.rs"), "fn main() {}").unwrap();
    fs::write(src.join("src/lib.rs"), "pub fn x() {}").unwrap();
    let out = dir.join(name);
    let opts = CreateOptions {
        password: password.map(str::to_string),
        threads: 2,
        ..CreateOptions::default()
    };
    compress(
        &[src],
        &out,
        OutputFormat::from_path(&out).unwrap(),
        &opts,
        &CancelToken::new(),
        &mut NoProgress,
    )
    .unwrap();
    out
}

fn open_sync(
    cx: &mut TestAppContext,
    handle: WindowHandle<Root>,
    view: &Entity<MainView>,
    path: &Path,
    password: Option<&str>,
) {
    let result = load_archive(path, password);
    let (path, password) = (path.to_path_buf(), password.map(str::to_string));
    with_view(cx, handle, view, |v, window, cx| {
        v.finish_open(path, password, result, window, cx);
    });
}

fn row_names(cx: &mut TestAppContext, view: &Entity<MainView>) -> Vec<String> {
    cx.update(|cx| {
        view.read(cx)
            .table
            .read(cx)
            .delegate()
            .rows()
            .iter()
            .map(|r| r.name.clone())
            .collect()
    })
}

fn wait_for_tasks(cx: &mut TestAppContext, handle: WindowHandle<Root>, view: &Entity<MainView>) {
    let ids: Vec<_> = cx.update(|cx| {
        view.read(cx)
            .tasks
            .snapshot()
            .iter()
            .map(|t| t.id)
            .collect()
    });
    for id in ids {
        cx.update(|cx| view.read(cx).tasks.wait_for(id, WAIT));
    }
    with_view(cx, handle, view, |v, _, cx| v.refresh_tasks(cx));
}

#[gpui_kit::test]
fn the_empty_window_draws(cx: &mut TestAppContext) {
    let (handle, view) = open_window(cx, None);
    draw(cx, handle);
    cx.update(|cx| {
        let v = view.read(cx);
        assert!(v.current().is_none());
        assert!(v.overlay.is_none());
    });
    assert!(row_names(cx, &view).is_empty());
}

#[gpui_kit::test]
fn opening_an_archive_adds_a_tab_and_lists_the_top_level(cx: &mut TestAppContext) {
    let tmp = tempfile::tempdir().unwrap();
    let archive = make_archive(tmp.path(), "a.zip", None);
    let (handle, view) = open_window(cx, None);
    open_sync(cx, handle, &view, &archive, None);
    cx.update(|cx| {
        let v = view.read(cx);
        assert_eq!(v.archives.len(), 1);
        assert_eq!(v.current().unwrap().format, "zip");
        assert!(v.loading.is_none());
    });
    assert_eq!(row_names(cx, &view), ["proj"]);
}

#[gpui_kit::test]
fn folders_open_on_double_click_and_the_way_back_works(cx: &mut TestAppContext) {
    let tmp = tempfile::tempdir().unwrap();
    let archive = make_archive(tmp.path(), "a.7z", None);
    let (handle, view) = open_window(cx, None);
    open_sync(cx, handle, &view, &archive, None);

    let table = cx.update(|cx| view.read(cx).table.clone());
    cx.update_window(handle.into(), |_, _, cx| {
        table.update(cx, |_, cx| cx.emit(TableEvent::DoubleClickedRow(0)));
    })
    .unwrap();
    draw(cx, handle);
    assert_eq!(row_names(cx, &view), ["docs", "src", "readme.txt"]);
    cx.update(|cx| assert_eq!(view.read(cx).current().unwrap().cwd, "proj"));

    with_view(cx, handle, &view, |v, _, cx| v.navigate("proj/src", cx));
    assert_eq!(row_names(cx, &view), ["lib.rs", "main.rs"]);

    with_view(cx, handle, &view, |v, _, cx| v.go_up(cx));
    cx.update(|cx| assert_eq!(view.read(cx).current().unwrap().cwd, "proj"));
    with_view(cx, handle, &view, |v, _, cx| v.go_up(cx));
    with_view(cx, handle, &view, |v, _, cx| v.go_up(cx)); // already at the top: stays
    assert_eq!(row_names(cx, &view), ["proj"]);
}

#[gpui_kit::test]
fn a_picture_is_previewed_and_other_files_are_not(cx: &mut TestAppContext) {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("pics");
    fs::create_dir_all(&src).unwrap();
    fs::write(src.join("note.txt"), "text").unwrap();
    image::RgbaImage::from_pixel(8, 8, image::Rgba([10, 150, 130, 255]))
        .save(src.join("pic.png"))
        .unwrap();
    let archive = tmp.path().join("p.zip");
    compress(
        &[src],
        &archive,
        OutputFormat::from_path(&archive).unwrap(),
        &CreateOptions::default(),
        &CancelToken::new(),
        &mut NoProgress,
    )
    .unwrap();
    let (handle, view) = open_window(cx, None);
    open_sync(cx, handle, &view, &archive, None);
    with_view(cx, handle, &view, |v, _, cx| v.navigate("pics", cx));
    let table = cx.update(|cx| view.read(cx).table.clone());
    let select = |cx: &mut TestAppContext, ix: usize| {
        cx.update_window(handle.into(), |_, _, cx| {
            table.update(cx, |_, cx| cx.emit(TableEvent::SelectRow(ix)));
        })
        .unwrap();
        cx.run_until_parked();
        draw(cx, handle);
    };

    // Rows are note.txt, pic.png.
    select(cx, 1);
    cx.update(|cx| {
        let pv = view.read(cx).preview.as_ref().expect("a preview");
        assert_eq!(pv.path, "pics/pic.png");
    });
    select(cx, 0);
    cx.update(|cx| assert!(view.read(cx).preview.is_none()));
}

#[gpui_kit::test]
fn a_finished_test_marks_the_archive_as_verified(cx: &mut TestAppContext) {
    let tmp = tempfile::tempdir().unwrap();
    let archive = make_archive(tmp.path(), "a.zip", None);
    let (handle, view) = open_window(cx, None);
    open_sync(cx, handle, &view, &archive, None);
    cx.update(|cx| assert_eq!(view.read(cx).current().unwrap().verified, None));
    with_view(cx, handle, &view, |v, _, cx| v.start_test(cx));
    wait_for_tasks(cx, handle, &view);
    cx.update(|cx| assert_eq!(view.read(cx).current().unwrap().verified, Some(true)));
}

#[gpui_kit::test]
fn selecting_a_row_fills_the_details_and_double_clicking_a_file_does_nothing(
    cx: &mut TestAppContext,
) {
    let tmp = tempfile::tempdir().unwrap();
    let archive = make_archive(tmp.path(), "a.zip", None);
    let (handle, view) = open_window(cx, None);
    open_sync(cx, handle, &view, &archive, None);
    with_view(cx, handle, &view, |v, _, cx| v.navigate("proj", cx));

    let table = cx.update(|cx| view.read(cx).table.clone());
    // Row 2 is readme.txt (folders first).
    cx.update_window(handle.into(), |_, _, cx| {
        table.update(cx, |_, cx| cx.emit(TableEvent::SelectRow(2)));
    })
    .unwrap();
    draw(cx, handle);
    cx.update(|cx| {
        let v = view.read(cx);
        let row = v.selected_row().unwrap();
        assert_eq!(row.name, "readme.txt");
        assert_eq!(row.size, 13);
    });

    cx.update_window(handle.into(), |_, _, cx| {
        table.update(cx, |_, cx| cx.emit(TableEvent::DoubleClickedRow(2)));
    })
    .unwrap();
    cx.update(|cx| assert_eq!(view.read(cx).current().unwrap().cwd, "proj"));

    // Moving to another folder clears the selection.
    with_view(cx, handle, &view, |v, _, cx| v.navigate("proj/src", cx));
    cx.update(|cx| assert!(view.read(cx).selected_row().is_none()));
}

#[gpui_kit::test]
fn typing_in_the_search_box_filters_the_list(cx: &mut TestAppContext) {
    let tmp = tempfile::tempdir().unwrap();
    let archive = make_archive(tmp.path(), "a.zip", None);
    let (handle, view) = open_window(cx, None);
    open_sync(cx, handle, &view, &archive, None);

    let select_all = if cfg!(target_os = "macos") {
        "cmd-a"
    } else {
        "ctrl-a"
    };

    // Type the way a user does: focus the box, then send keystrokes.
    cx.update_window(handle.into(), |_, window, cx| {
        window.click("search", cx);
        window.input("main", cx);
    })
    .unwrap();
    cx.run_until_parked();
    draw(cx, handle);
    assert_eq!(row_names(cx, &view), ["main.rs"]);

    cx.update_window(handle.into(), |_, window, cx| {
        window.press(select_all, cx);
        window.input("说明", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(row_names(cx, &view), ["说明.md"]);

    cx.update_window(handle.into(), |_, window, cx| {
        window.press(select_all, cx);
        window.press("backspace", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(row_names(cx, &view), ["proj"]);
}

#[gpui_kit::test]
fn extracting_runs_a_task_and_reports_the_result(cx: &mut TestAppContext) {
    let tmp = tempfile::tempdir().unwrap();
    let archive = make_archive(tmp.path(), "a.zip", None);
    let out = tmp.path().join("out");
    let (handle, view) = open_window(cx, None);
    open_sync(cx, handle, &view, &archive, None);

    with_view(cx, handle, &view, |v, _, cx| {
        v.settings.open_folder_when_done = false;
        v.start_extract(out.clone(), None, cx);
    });
    wait_for_tasks(cx, handle, &view);
    cx.update(|cx| {
        let v = view.read(cx);
        assert_eq!(v.task_view.len(), 1);
        assert_eq!(v.task_view[0].state, TaskState::Done);
        let Some(Notice::Info(msg)) = &v.notice else {
            panic!("expected a success message, got {:?}", v.notice);
        };
        assert!(msg.contains("解压完成") && msg.contains('4'), "{msg}");
    });
    assert_eq!(
        fs::read_to_string(out.join("proj/readme.txt")).unwrap(),
        "hello rustzip"
    );
}

#[gpui_kit::test]
fn extracting_only_the_selection_leaves_the_rest(cx: &mut TestAppContext) {
    let tmp = tempfile::tempdir().unwrap();
    let archive = make_archive(tmp.path(), "a.7z", None);
    let out = tmp.path().join("out");
    let (handle, view) = open_window(cx, None);
    open_sync(cx, handle, &view, &archive, None);
    with_view(cx, handle, &view, |v, _, cx| {
        v.settings.open_folder_when_done = false;
        v.start_extract(out.clone(), Some(vec!["proj/src".into()]), cx);
    });
    wait_for_tasks(cx, handle, &view);
    assert!(out.join("proj/src/main.rs").exists());
    assert!(!out.join("proj/readme.txt").exists());
}

#[gpui_kit::test]
fn a_password_protected_archive_asks_and_then_extracts(cx: &mut TestAppContext) {
    let tmp = tempfile::tempdir().unwrap();
    let archive = make_archive(tmp.path(), "secret.zip", Some("pw1"));
    let out = tmp.path().join("out");
    let (handle, view) = open_window(cx, None);

    // Listing a zip needs no password; extracting does.
    open_sync(cx, handle, &view, &archive, None);
    with_view(cx, handle, &view, |v, _, cx| {
        v.settings.open_folder_when_done = false;
        v.start_extract(out.clone(), None, cx);
    });
    wait_for_tasks(cx, handle, &view);
    let failed = cx.update(|cx| {
        let t = view.read(cx).task_view[0].clone();
        assert_eq!(t.state, TaskState::Failed);
        assert_eq!(t.error.as_ref().map(|e| e.code), Some(ErrorCode::Password));
        assert!(matches!(view.read(cx).notice, Some(Notice::Error(_))));
        t.id
    });

    with_view(cx, handle, &view, |v, window, cx| {
        v.ask_password(PasswordTarget::Task(failed), false, window, cx);
    });
    // An empty password is not submitted.
    with_view(cx, handle, &view, |v, window, cx| {
        v.submit_password(window, cx)
    });
    cx.update(|cx| assert!(matches!(view.read(cx).overlay, Some(Overlay::Password(_)))));

    let input = cx.update(|cx| match &view.read(cx).overlay {
        Some(Overlay::Password(p)) => p.input.clone(),
        _ => panic!("no password prompt"),
    });
    cx.update_window(handle.into(), |_, window, cx| {
        input.update(cx, |s, cx| s.set_value("pw1", window, cx));
    })
    .unwrap();
    with_view(cx, handle, &view, |v, window, cx| {
        v.submit_password(window, cx)
    });
    cx.update(|cx| assert!(view.read(cx).overlay.is_none()));
    wait_for_tasks(cx, handle, &view);
    cx.update(|cx| {
        let tasks = &view.read(cx).task_view;
        assert_eq!(tasks.len(), 2);
        assert_eq!(tasks[1].state, TaskState::Done, "{:?}", tasks[1].error);
    });
    assert!(out.join("proj/readme.txt").exists());
}

#[gpui_kit::test]
fn an_archive_with_encrypted_names_prompts_before_opening(cx: &mut TestAppContext) {
    let tmp = tempfile::tempdir().unwrap();
    let archive = make_archive(tmp.path(), "hidden.7z", Some("pw2"));
    let (handle, view) = open_window(cx, None);

    open_sync(cx, handle, &view, &archive, None);
    cx.update(|cx| {
        let v = view.read(cx);
        assert!(v.archives.is_empty());
        assert!(matches!(
            &v.overlay,
            Some(Overlay::Password(p)) if !p.wrong && p.target == PasswordTarget::Open(archive.clone())
        ));
    });

    // A wrong password asks again and says so.
    open_sync(cx, handle, &view, &archive, Some("nope"));
    cx.update(|cx| {
        assert!(matches!(
            &view.read(cx).overlay,
            Some(Overlay::Password(p)) if p.wrong
        ));
    });

    open_sync(cx, handle, &view, &archive, Some("pw2"));
    cx.update(|cx| {
        let v = view.read(cx);
        assert!(v.overlay.is_none());
        assert_eq!(v.archives.len(), 1);
        assert_eq!(v.current().unwrap().password.as_deref(), Some("pw2"));
    });
    assert_eq!(row_names(cx, &view), ["proj"]);
}

#[gpui_kit::test]
fn a_remembered_password_opens_the_archive_without_asking(cx: &mut TestAppContext) {
    let tmp = tempfile::tempdir().unwrap();
    let archive = make_archive(tmp.path(), "hidden.7z", Some("pw2"));
    let (handle, view) = open_window(cx, None);

    // With the setting off (the default) nothing is kept.
    open_sync(cx, handle, &view, &archive, Some("pw2"));
    cx.run_until_parked();
    cx.update(|cx| assert!(view.read(cx).vault.labels().is_empty()));
    with_view(cx, handle, &view, |v, _, cx| v.close_tab(0, cx));

    with_view(cx, handle, &view, |v, _, _| {
        v.settings.remember_passwords = true
    });
    open_sync(cx, handle, &view, &archive, Some("pw2"));
    cx.run_until_parked();
    cx.update(|cx| {
        let vault = &view.read(cx).vault;
        assert_eq!(vault.labels(), ["hidden.7z"]);
        assert_eq!(vault.get("hidden.7z").as_deref(), Some("pw2"));
    });
    with_view(cx, handle, &view, |v, _, cx| v.close_tab(0, cx));

    // Opening again needs no typing.
    open_sync(cx, handle, &view, &archive, None);
    cx.run_until_parked();
    draw(cx, handle);
    cx.update(|cx| {
        let v = view.read(cx);
        assert!(
            v.overlay.is_none(),
            "the saved password should have been tried"
        );
        assert_eq!(v.archives.len(), 1);
    });

    // A stale saved password falls back to asking.
    with_view(cx, handle, &view, |v, _, cx| {
        v.close_tab(0, cx);
        v.forget_all_passwords(cx);
    });
    cx.run_until_parked();
    cx.update(|cx| assert!(view.read(cx).vault.labels().is_empty()));
    open_sync(cx, handle, &view, &archive, None);
    cx.run_until_parked();
    cx.update(|cx| assert!(matches!(view.read(cx).overlay, Some(Overlay::Password(_)))));
}

#[gpui_kit::test]
fn a_file_that_is_not_an_archive_shows_an_error_and_no_tab(cx: &mut TestAppContext) {
    let tmp = tempfile::tempdir().unwrap();
    let junk = tmp.path().join("junk.zip");
    fs::write(&junk, "not an archive").unwrap();
    let (handle, view) = open_window(cx, None);
    open_sync(cx, handle, &view, &junk, None);
    cx.update(|cx| {
        let v = view.read(cx);
        assert!(v.archives.is_empty());
        assert!(matches!(&v.notice, Some(Notice::Error(m)) if m.contains("无法识别")));
    });
}

#[gpui_kit::test]
fn several_archives_keep_their_own_folder_and_tabs_close_cleanly(cx: &mut TestAppContext) {
    let tmp = tempfile::tempdir().unwrap();
    let a = make_archive(tmp.path(), "a.zip", None);
    let b = tmp.path().join("b.tar.gz");
    compress(
        &[tmp.path().join("proj/docs")],
        &b,
        OutputFormat::from_path(&b).unwrap(),
        &CreateOptions::default(),
        &CancelToken::new(),
        &mut NoProgress,
    )
    .unwrap();
    let (handle, view) = open_window(cx, None);
    open_sync(cx, handle, &view, &a, None);
    with_view(cx, handle, &view, |v, _, cx| v.navigate("proj/src", cx));
    open_sync(cx, handle, &view, &b, None);
    assert_eq!(row_names(cx, &view), ["docs"]);

    with_view(cx, handle, &view, |v, _, cx| v.activate_tab(0, cx));
    assert_eq!(row_names(cx, &view), ["lib.rs", "main.rs"]);

    with_view(cx, handle, &view, |v, _, cx| v.close_tab(0, cx));
    cx.update(|cx| assert_eq!(view.read(cx).archives.len(), 1));
    assert_eq!(row_names(cx, &view), ["docs"]);
    with_view(cx, handle, &view, |v, _, cx| v.close_tab(0, cx));
    assert!(row_names(cx, &view).is_empty());
    with_view(cx, handle, &view, |v, _, cx| v.close_tab(5, cx)); // out of range: ignored
}

#[gpui_kit::test]
fn the_compress_form_builds_the_right_job(cx: &mut TestAppContext) {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("data");
    fs::create_dir_all(&src).unwrap();
    fs::write(src.join("a.txt"), "a".repeat(5000)).unwrap();
    let (handle, view) = open_window(cx, None);

    with_view(cx, handle, &view, |v, window, cx| {
        v.open_compress(vec![src.clone()], window, cx);
    });
    cx.update(|cx| {
        let v = view.read(cx);
        let Some(Overlay::Compress(f)) = &v.overlay else {
            panic!("no compress form");
        };
        assert_eq!(f.name.read(cx).value().as_ref(), "data");
        assert_eq!(f.format, Format::Zip);
        assert_eq!(f.preset, Preset::Balanced);
    });

    with_view(cx, handle, &view, |v, _, cx| {
        v.set_format(Format::SevenZ, cx);
        v.set_preset(Preset::Smallest, cx);
    });
    with_view(cx, handle, &view, |v, _, cx| v.submit_compress(cx));
    wait_for_tasks(cx, handle, &view);
    let out = tmp.path().join("data.7z");
    assert!(out.exists());
    cx.update(|cx| {
        let v = view.read(cx);
        assert!(v.overlay.is_none());
        assert_eq!(v.task_view[0].state, TaskState::Done);
    });
    let (tree, format) = load_archive(&out, None).unwrap();
    assert_eq!(format, "7z");
    assert_eq!(tree.total_files(), 1);
}

#[gpui_kit::test]
fn the_compress_form_refuses_bad_names_and_existing_files(cx: &mut TestAppContext) {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("data");
    fs::create_dir_all(&src).unwrap();
    fs::write(src.join("a.txt"), "a").unwrap();
    fs::write(tmp.path().join("data.zip"), "already here").unwrap();
    let (handle, view) = open_window(cx, None);
    with_view(cx, handle, &view, |v, window, cx| {
        v.open_compress(vec![src.clone()], window, cx);
    });

    // "data.zip" exists next to the source.
    with_view(cx, handle, &view, |v, _, cx| v.submit_compress(cx));
    cx.update(|cx| {
        let v = view.read(cx);
        assert!(matches!(&v.notice, Some(Notice::Error(m)) if m.contains("已存在")));
        assert!(
            matches!(v.overlay, Some(Overlay::Compress(_))),
            "form stays open"
        );
        assert!(v.tasks.snapshot().is_empty());
    });
    assert_eq!(
        fs::read_to_string(tmp.path().join("data.zip")).unwrap(),
        "already here"
    );

    for bad in ["", "a/b", "..", "x:y", ".hidden"] {
        let name = cx.update(|cx| match &view.read(cx).overlay {
            Some(Overlay::Compress(f)) => f.name.clone(),
            _ => panic!("no form"),
        });
        cx.update_window(handle.into(), |_, window, cx| {
            name.update(cx, |s, cx| s.set_value(bad, window, cx));
        })
        .unwrap();
        with_view(cx, handle, &view, |v, _, cx| v.submit_compress(cx));
        cx.update(|cx| {
            let v = view.read(cx);
            assert!(matches!(&v.notice, Some(Notice::Error(_))), "{bad:?}");
            assert!(v.tasks.snapshot().is_empty(), "{bad:?}");
        });
    }
}

#[gpui_kit::test]
fn the_password_field_is_ignored_for_formats_that_cannot_encrypt(cx: &mut TestAppContext) {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("data");
    fs::create_dir_all(&src).unwrap();
    fs::write(src.join("a.txt"), "a").unwrap();
    let (handle, view) = open_window(cx, None);
    with_view(cx, handle, &view, |v, window, cx| {
        v.open_compress(vec![src.clone()], window, cx);
        v.set_format(Format::TarZst, cx);
    });
    let pw = cx.update(|cx| match &view.read(cx).overlay {
        Some(Overlay::Compress(f)) => f.password.clone(),
        _ => panic!("no form"),
    });
    cx.update_window(handle.into(), |_, window, cx| {
        pw.update(cx, |s, cx| s.set_value("secret", window, cx));
    })
    .unwrap();
    with_view(cx, handle, &view, |v, _, cx| v.submit_compress(cx));
    wait_for_tasks(cx, handle, &view);
    // Typed into a hidden field before switching format: must not make the job fail.
    let out = tmp.path().join("data.tar.zst");
    assert!(out.exists());
    assert!(load_archive(&out, None).is_ok());
}

#[gpui_kit::test]
fn dropping_files_opens_archives_and_offers_to_compress_the_rest(cx: &mut TestAppContext) {
    let tmp = tempfile::tempdir().unwrap();
    let archive = make_archive(tmp.path(), "a.zip", None);
    let plain = tmp.path().join("notes.txt");
    fs::write(&plain, "hi").unwrap();
    let (handle, view) = open_window(cx, None);

    with_view(cx, handle, &view, |v, window, cx| {
        v.handle_drop(vec![archive.clone()], window, cx);
    });
    cx.run_until_parked();
    cx.update(|cx| assert_eq!(view.read(cx).archives.len(), 1));

    with_view(cx, handle, &view, |v, window, cx| {
        v.handle_drop(vec![plain.clone()], window, cx);
    });
    cx.update(|cx| match &view.read(cx).overlay {
        Some(Overlay::Compress(f)) => assert_eq!(f.sources, vec![plain.clone()]),
        _ => panic!("expected the compress form"),
    });
}

#[gpui_kit::test]
fn opened_archives_are_remembered_across_launches(cx: &mut TestAppContext) {
    let tmp = tempfile::tempdir().unwrap();
    let archive = make_archive(tmp.path(), "a.zip", None);
    let settings = tmp.path().join("cfg/settings.json");
    let (handle, view) = open_window(cx, Some(settings.clone()));
    open_sync(cx, handle, &view, &archive, None);
    assert_eq!(Settings::load(&settings).recent, vec![archive.clone()]);

    // A second window starts with the saved list.
    let (_handle2, view2) = open_window(cx, Some(settings));
    cx.update(|cx| assert_eq!(view2.read(cx).settings.recent, vec![archive]));
}

#[gpui_kit::test]
fn modified_clicks_build_a_multi_selection_that_navigation_clears(cx: &mut TestAppContext) {
    let tmp = tempfile::tempdir().unwrap();
    let archive = make_archive(tmp.path(), "a.zip", None);
    let (handle, view) = open_window(cx, None);
    open_sync(cx, handle, &view, &archive, None);
    let table = cx.update(|cx| view.read(cx).table.clone());
    let marked = |cx: &mut TestAppContext| cx.update(|cx| table.read(cx).delegate().marked());
    // Inside "proj" there are two folders, docs and src.
    with_view(cx, handle, &view, |v, _, cx| v.navigate("proj", cx));

    cx.update(|cx| {
        table.update(cx, |t, _| {
            let d = t.delegate_mut();
            d.mark_only("proj/docs");
            d.toggle(1);
        })
    });
    assert_eq!(marked(cx).len(), 2);
    // Toggling a picked row again takes it out.
    cx.update(|cx| table.update(cx, |t, _| t.delegate_mut().toggle(1)));
    assert_eq!(marked(cx), vec!["proj/docs".to_string()]);
    cx.update(|cx| table.update(cx, |t, _| t.delegate_mut().mark_range(0, 1)));
    assert_eq!(marked(cx).len(), 2);

    with_view(cx, handle, &view, |v, _, cx| v.navigate("proj/docs", cx));
    assert!(marked(cx).is_empty());
}

#[gpui_kit::test]
fn settings_changes_are_saved_and_the_theme_follows_the_choice(cx: &mut TestAppContext) {
    use rz_app::settings::{OnConflict, Theme as Pick};
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("cfg/settings.json");
    let (handle, view) = open_window(cx, Some(path.clone()));
    with_view(cx, handle, &view, |v, _, cx| v.open_settings(cx));
    cx.update(|cx| assert!(matches!(view.read(cx).overlay, Some(Overlay::Settings))));

    with_view(cx, handle, &view, |v, window, cx| {
        v.set_theme(Pick::Dark, window, cx);
        v.change_settings(|s| s.on_conflict = OnConflict::Skip, cx);
        v.change_settings(|s| s.smart_extract = false, cx);
    });
    cx.update(|cx| assert!(cx.theme().is_dark()));
    let saved = Settings::load(&path);
    assert_eq!(saved.theme, Pick::Dark);
    assert_eq!(saved.on_conflict, OnConflict::Skip);
    assert!(!saved.smart_extract);

    // A fixed choice is not undone by the system; choosing "system" hands control back.
    with_view(cx, handle, &view, |v, window, cx| {
        v.set_theme(Pick::Light, window, cx);
    });
    cx.update(|cx| assert!(!cx.theme().is_dark()));
    assert_eq!(
        cx.update(|cx| cx.theme().primary),
        Palette::themed(false).accent
    );
}

#[gpui_kit::test]
fn the_accent_colour_is_saved_and_repaints_the_theme(cx: &mut TestAppContext) {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("cfg/settings.json");
    let (handle, view) = open_window(cx, Some(path.clone()));
    let before = cx.update(|cx| cx.theme().primary);

    with_view(cx, handle, &view, |v, _, cx| v.set_accent("rose", cx));
    let after = cx.update(|cx| cx.theme().primary);
    assert_ne!(before, after);
    assert_eq!(Settings::load(&path).accent, "rose");

    // A hand-written colour works too, and a bad one falls back to the default.
    with_view(cx, handle, &view, |v, _, cx| v.set_accent("#336699", cx));
    assert_ne!(cx.update(|cx| cx.theme().primary), after);
    with_view(cx, handle, &view, |v, _, cx| {
        v.set_accent("not a colour", cx)
    });
    assert_eq!(cx.update(|cx| cx.theme().primary), before);
}

#[gpui_kit::test]
fn the_privacy_notice_shows_once_and_records_the_answer(cx: &mut TestAppContext) {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("settings.json");
    let (handle, view) = open_window(cx, Some(path.clone()));
    with_view(cx, handle, &view, |v, _, cx| v.show_first_run_notice(cx));
    cx.update(|cx| assert!(matches!(view.read(cx).overlay, Some(Overlay::Privacy))));

    with_view(cx, handle, &view, |v, _, cx| v.answer_privacy(false, cx));
    cx.update(|cx| assert!(view.read(cx).overlay.is_none()));
    let saved = Settings::load(&path);
    assert!(saved.privacy_notice_seen);
    assert!(!saved.send_install_stats);

    // The next start does not ask again.
    let (handle2, view2) = open_window(cx, Some(path));
    with_view(cx, handle2, &view2, |v, _, cx| v.show_first_run_notice(cx));
    cx.update(|cx| assert!(view2.read(cx).overlay.is_none()));
}

#[gpui_kit::test]
fn a_task_can_be_paused_cancelled_and_cleared_from_the_window(cx: &mut TestAppContext) {
    let tmp = tempfile::tempdir().unwrap();
    let archive = make_archive(tmp.path(), "a.zip", None);
    let (handle, view) = open_window(cx, None);
    open_sync(cx, handle, &view, &archive, None);

    // A held task keeps its place in the list without doing anything.
    let held = cx.update(|cx| {
        view.read(cx).tasks.submit_paused(JobSpec::Extract {
            archive: archive.clone(),
            dest: tmp.path().join("held"),
            opts: ExtractOptions::default(),
            smart: false,
            encoding: None,
        })
    });
    with_view(cx, handle, &view, |v, _, cx| v.refresh_tasks(cx));
    cx.update(|cx| assert_eq!(view.read(cx).task_view[0].state, TaskState::Paused));

    with_view(cx, handle, &view, |v, _, cx| v.cancel_task(held, cx));
    wait_for_tasks(cx, handle, &view);
    cx.update(|cx| assert_eq!(view.read(cx).task_view[0].state, TaskState::Cancelled));
    assert!(!tmp.path().join("held").exists());

    with_view(cx, handle, &view, |v, _, cx| v.clear_finished(cx));
    cx.update(|cx| assert!(view.read(cx).task_view.is_empty()));
}

#[gpui_kit::test]
fn the_brand_colours_survive_a_switch_between_light_and_dark(cx: &mut TestAppContext) {
    let (handle, _view) = open_window(cx, None);
    for (mode, palette) in [
        (ThemeMode::Dark, Palette::themed(true)),
        (ThemeMode::Light, Palette::themed(false)),
        (ThemeMode::Dark, Palette::themed(true)),
    ] {
        cx.update_window(handle.into(), |_, window, cx| {
            Theme::change(mode, Some(window), cx);
            brand::apply(cx);
            let t = cx.theme();
            assert_eq!(t.primary, palette.accent, "{mode:?}");
            assert_eq!(t.background, palette.win, "{mode:?}");
            assert_eq!(t.border, palette.line, "{mode:?}");
            assert_eq!(t.list_active, palette.accent_soft, "{mode:?}");
        })
        .unwrap();
        draw(cx, handle);
    }
}

#[gpui_kit::test]
fn a_huge_archive_lists_and_scrolls_without_trouble(cx: &mut TestAppContext) {
    use rz_core::{Entry, EntryKind};
    let entries: Vec<Entry> = (0..200_000)
        .map(|i| Entry {
            index: i,
            path: format!("dir{}/file_{i:06}.txt", i % 50),
            kind: EntryKind::File,
            size: (i as u64 * 31) % 100_000,
            packed_size: Some((i as u64 * 7) % 50_000),
            modified: Some(1_700_000_000 + i as i64),
            encrypted: false,
            unix_mode: None,
        })
        .collect();
    let tree = rz_app::browse::ArchiveTree::build(entries);
    let (handle, view) = open_window(cx, None);
    let started = std::time::Instant::now();
    with_view(cx, handle, &view, |v, window, cx| {
        v.finish_open(
            "/big.zip".into(),
            None,
            Ok((tree, "zip".into())),
            window,
            cx,
        );
        v.navigate("dir7", cx);
    });
    let rows = cx.update(|cx| view.read(cx).table.read(cx).delegate().rows().len());
    assert_eq!(rows, 4000);
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "listing took {:?}",
        started.elapsed()
    );
}
