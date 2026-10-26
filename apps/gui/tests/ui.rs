#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Headless tests of the main window: a real window, real tables and inputs,
//! real archives. They check what the user could observe through state and layout;
//! pixels and the operating system's input methods need a person.

use gpui_kit::component::table::{TableDelegate, TableEvent};
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
                let mut settings = settings_path
                    .as_deref()
                    .map(Settings::load)
                    .unwrap_or_default();
                // Most UI assertions use Chinese copy; locale behavior is covered explicitly.
                settings.language = "zh-CN".into();
                let view = cx.new(|cx| MainView::new(settings, settings_path, window, cx));
                // A test must never start a real program on the machine running it.
                view.update(cx, |v, _| v.opener = std::sync::Arc::new(|_| {}));
                view
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
    // The source folder is kept apart, so that extracting next to the archive in a test
    // cannot be confused with files that were there already.
    let src = dir.join(format!("{name}.src")).join("proj");
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
    let (path, password) = (path.to_path_buf(), password.map(str::to_string));
    let (request_id, _) = with_view(cx, handle, view, |v, _, _| v.begin_open_request(&path));
    let result = load_archive(&path, password.as_deref());
    with_view(cx, handle, view, |v, window, cx| {
        v.finish_open(request_id, path, password, result, window, cx);
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
    for _ in 0..8 {
        let ids: Vec<_> = cx.update(|cx| {
            view.read(cx)
                .tasks
                .snapshot()
                .iter()
                .filter(|task| !task.state.is_finished())
                .map(|task| task.id)
                .collect()
        });
        if ids.is_empty() {
            with_view(cx, handle, view, |v, _, cx| v.refresh_tasks(cx));
            let pending = cx.update(|cx| {
                view.read(cx)
                    .tasks
                    .snapshot()
                    .iter()
                    .any(|task| !task.state.is_finished())
            });
            if !pending {
                return;
            }
            continue;
        }
        for id in ids {
            cx.update(|cx| view.read(cx).tasks.wait_for(id, WAIT));
        }
    }
    with_view(cx, handle, view, |v, _, cx| v.refresh_tasks(cx));
}

/// Lets background work run until `done` holds (or a few seconds pass).
fn wait_until(
    cx: &mut TestAppContext,
    handle: WindowHandle<Root>,
    done: impl Fn(&mut TestAppContext) -> bool,
) {
    for _ in 0..150 {
        cx.run_until_parked();
        if done(cx) {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    draw(cx, handle);
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
        table.update(cx, |_, cx| cx.emit(TableEvent::DoubleClickedRow(1)));
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
fn pictures_and_plain_text_are_previewed_in_the_details_panel(cx: &mut TestAppContext) {
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
            table.update(cx, |_, cx| cx.emit(TableEvent::SelectRow(ix + 1)));
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
        assert!(pv.image.is_some());
        assert!(pv.text.is_none());
    });
    select(cx, 0);
    cx.update(|cx| {
        let pv = view.read(cx).preview.as_ref().expect("text preview");
        assert_eq!(pv.path, "pics/note.txt");
        assert_eq!(pv.text.as_deref(), Some("text"));
        assert!(pv.image.is_none());
    });
}

#[gpui_kit::test]
fn the_recent_page_lists_archives_and_opening_one_returns_to_the_files(cx: &mut TestAppContext) {
    use rustzip_gui::view::Page;
    let tmp = tempfile::tempdir().unwrap();
    let first = make_archive(tmp.path(), "a.zip", None);
    let (handle, view) = open_window(cx, None);
    open_sync(cx, handle, &view, &first, None);
    cx.update(|cx| assert_eq!(view.read(cx).page, Page::Archive));

    with_view(cx, handle, &view, |v, _, cx| v.show_page(Page::Recent, cx));
    cx.update(|cx| assert_eq!(view.read(cx).page, Page::Recent));

    // Opening an archive, even one that is already open, goes back to its files.
    with_view(cx, handle, &view, |v, window, cx| {
        v.open_path(first.clone(), None, window, cx);
    });
    cx.update(|cx| assert_eq!(view.read(cx).page, Page::Archive));
}

#[gpui_kit::test]
fn deleting_and_adding_rewrite_the_archive_and_the_list_follows(cx: &mut TestAppContext) {
    use rustzip_gui::view::Overlay as O;
    let tmp = tempfile::tempdir().unwrap();
    let archive = make_archive(tmp.path(), "a.zip", None);
    let (handle, view) = open_window(cx, None);
    open_sync(cx, handle, &view, &archive, None);
    with_view(cx, handle, &view, |v, _, cx| v.navigate("proj", cx));
    assert_eq!(row_names(cx, &view), ["docs", "src", "readme.txt"]);

    // Pick readme.txt and ask to delete it: nothing happens until the user agrees.
    let table = cx.update(|cx| view.read(cx).table.clone());
    cx.update_window(handle.into(), |_, _, cx| {
        table.update(cx, |_, cx| cx.emit(TableEvent::SelectRow(3)));
    })
    .unwrap();
    with_view(cx, handle, &view, |v, _, cx| v.ask_delete(cx));
    cx.update(|cx| {
        assert!(
            matches!(&view.read(cx).overlay, Some(O::ConfirmDelete(p)) if p == &["proj/readme.txt"])
        );
    });
    assert_eq!(row_names(cx, &view), ["docs", "src", "readme.txt"]);

    with_view(cx, handle, &view, |v, _, cx| v.confirm_delete(cx));
    wait_for_tasks(cx, handle, &view);
    cx.run_until_parked();
    draw(cx, handle);
    assert_eq!(row_names(cx, &view), ["docs", "src"]);
    cx.update(|cx| assert!(view.read(cx).overlay.is_none()));

    // Adding puts the files into the folder being shown.
    let extra = tmp.path().join("extra.txt");
    fs::write(&extra, "more").unwrap();
    with_view(cx, handle, &view, |v, _, cx| v.add_files(vec![extra], cx));
    wait_for_tasks(cx, handle, &view);
    cx.run_until_parked();
    draw(cx, handle);
    assert_eq!(row_names(cx, &view), ["docs", "src", "extra.txt"]);
    // The file on disk really changed, not just the list.
    let (tree, _) = load_archive(&archive, None).unwrap();
    assert!(tree.find("proj/extra.txt").is_some());
    assert!(tree.find("proj/readme.txt").is_none());
}

#[gpui_kit::test]
fn editing_depends_on_what_the_file_is_not_on_its_name(cx: &mut TestAppContext) {
    let tmp = tempfile::tempdir().unwrap();
    let zip = make_archive(tmp.path(), "a.zip", None);
    let odd = tmp.path().join("library.jar");
    fs::rename(&zip, &odd).unwrap();
    let (handle, view) = open_window(cx, None);
    open_sync(cx, handle, &view, &odd, None);
    cx.update(|cx| assert!(view.read(cx).can_edit()));
}

#[gpui_kit::test]
fn archives_that_cannot_be_rewritten_refuse_to_be_edited(cx: &mut TestAppContext) {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("one.txt");
    fs::write(&file, "hello hello hello").unwrap();
    let gz = tmp.path().join("one.txt.gz");
    compress(
        &[file],
        &gz,
        OutputFormat::from_path(&gz).unwrap(),
        &CreateOptions::default(),
        &CancelToken::new(),
        &mut NoProgress,
    )
    .unwrap();
    let (handle, view) = open_window(cx, None);
    open_sync(cx, handle, &view, &gz, None);
    cx.update(|cx| assert!(!view.read(cx).can_edit()));
    with_view(cx, handle, &view, |v, _, cx| {
        v.add_files(vec![gz.clone()], cx)
    });
    cx.update(|cx| assert!(view.read(cx).task_view.is_empty()));
}

#[gpui_kit::test]
fn double_clicking_a_file_opens_it_and_saving_it_writes_it_back(cx: &mut TestAppContext) {
    use std::sync::{Arc, Mutex};
    let tmp = tempfile::tempdir().unwrap();
    let archive = make_archive(tmp.path(), "a.zip", None);
    let (handle, view) = open_window(cx, None);
    open_sync(cx, handle, &view, &archive, None);
    with_view(cx, handle, &view, |v, _, cx| v.navigate("proj", cx));

    // Instead of starting a program, remember what would have been opened.
    let opened: Arc<Mutex<Vec<(PathBuf, String)>>> = Arc::default();
    let sink = opened.clone();
    cx.update(|cx| {
        view.update(cx, |v, _| {
            v.opener = Arc::new(move |p: &Path| {
                let text = fs::read_to_string(p).unwrap_or_default();
                sink.lock().unwrap().push((p.to_path_buf(), text));
            });
        })
    });
    let table = cx.update(|cx| view.read(cx).table.clone());
    // Row 2 is readme.txt.
    cx.update_window(handle.into(), |_, _, cx| {
        table.update(cx, |_, cx| cx.emit(TableEvent::DoubleClickedRow(3)));
    })
    .unwrap();
    cx.executor().advance_clock(Duration::from_millis(50));
    cx.run_until_parked();
    let (scratch, text) = opened.lock().unwrap().first().cloned().expect("opened");
    assert_eq!(text, "hello rustzip");
    assert_eq!(scratch.file_name().unwrap(), "readme.txt");

    // The program saves a change: after it has settled, it goes back into the archive.
    fs::write(&scratch, "changed in the editor").unwrap();
    for _ in 0..6 {
        cx.executor().advance_clock(Duration::from_millis(1300));
        cx.run_until_parked();
        std::thread::sleep(Duration::from_millis(20));
    }
    wait_for_tasks(cx, handle, &view);
    cx.run_until_parked();
    let out = tmp.path().join("out");
    let mut reader = rz_formats::open(&archive).unwrap();
    rz_engine::extract(
        &mut *reader,
        &out,
        &ExtractOptions::default(),
        &CancelToken::new(),
        &mut NoProgress,
    )
    .unwrap();
    assert_eq!(
        fs::read_to_string(out.join("proj/readme.txt")).unwrap(),
        "changed in the editor"
    );
    assert_eq!(
        fs::read_to_string(out.join("proj/src/main.rs")).unwrap(),
        "fn main() {}"
    );
    let _ = fs::remove_dir_all(scratch.parent().and_then(Path::parent).unwrap());
}

#[gpui_kit::test]
fn notices_fade_and_disappear_when_the_user_moves_on(cx: &mut TestAppContext) {
    let tmp = tempfile::tempdir().unwrap();
    let archive = make_archive(tmp.path(), "a.zip", None);
    let (handle, view) = open_window(cx, None);
    open_sync(cx, handle, &view, &archive, None);

    // A result is shown for a few seconds and then goes by itself.
    with_view(cx, handle, &view, |v, _, cx| v.start_test(cx));
    wait_for_tasks(cx, handle, &view);
    cx.update(|cx| assert!(view.read(cx).notice.is_some()));
    cx.executor().advance_clock(Duration::from_secs(7));
    cx.run_until_parked();
    cx.update(|cx| assert!(view.read(cx).notice.is_none()));

    // Or as soon as the user goes to another folder, page or file.
    with_view(cx, handle, &view, |v, _, cx| v.start_test(cx));
    wait_for_tasks(cx, handle, &view);
    cx.update(|cx| assert!(view.read(cx).notice.is_some()));
    with_view(cx, handle, &view, |v, _, cx| v.navigate("proj", cx));
    cx.update(|cx| assert!(view.read(cx).notice.is_none()));

    with_view(cx, handle, &view, |v, _, cx| v.start_test(cx));
    wait_for_tasks(cx, handle, &view);
    with_view(cx, handle, &view, |v, _, cx| {
        v.show_page(rustzip_gui::view::Page::Recent, cx)
    });
    cx.update(|cx| assert!(view.read(cx).notice.is_none()));
}

#[gpui_kit::test]
fn the_recent_page_selects_sorts_and_acts_on_the_chosen_archive(cx: &mut TestAppContext) {
    use rustzip_gui::recent::RecentSort;
    use rustzip_gui::view::{Overlay as O, Page};
    let tmp = tempfile::tempdir().unwrap();
    let a = make_archive(tmp.path(), "alpha.zip", None);
    let b = make_archive(tmp.path(), "beta.zip", None);
    let gone = tmp.path().join("gone.zip");
    let settings_path = tmp.path().join("cfg/settings.json");
    let settings = Settings {
        recent: vec![a.clone(), b.clone(), gone.clone()],
        // The test platform cannot show a folder.
        open_folder_when_done: false,
        ..Settings::default()
    };
    settings.save(&settings_path).unwrap();
    let (handle, view) = open_window(cx, Some(settings_path.clone()));
    open_sync(cx, handle, &view, &a, None);
    with_view(cx, handle, &view, |v, _, cx| v.show_page(Page::Recent, cx));
    cx.run_until_parked();

    let names = |cx: &mut TestAppContext| -> Vec<String> {
        cx.update(|cx| {
            view.read(cx)
                .recent
                .items
                .iter()
                .map(|i| i.name.clone())
                .collect()
        })
    };
    assert_eq!(names(cx).len(), 3);
    // Missing files are listed, marked as gone, and come last by date.
    cx.update(|cx| {
        let items = &view.read(cx).recent.items;
        assert!(!items.iter().find(|i| i.name == "gone.zip").unwrap().exists);
        assert!(
            items
                .iter()
                .find(|i| i.name == "beta.zip")
                .unwrap()
                .size
                .is_some()
        );
    });
    with_view(cx, handle, &view, |v, _, cx| {
        v.sort_recent(RecentSort::Name, cx)
    });
    assert_eq!(names(cx), ["alpha.zip", "beta.zip", "gone.zip"]);
    with_view(cx, handle, &view, |v, _, cx| {
        v.sort_recent(RecentSort::Name, cx)
    });
    assert_eq!(names(cx), ["gone.zip", "beta.zip", "alpha.zip"]);

    // Picking an archive unpacks it next to itself, either loose or into a named folder.
    with_view(cx, handle, &view, |v, _, cx| v.select_recent(b.clone(), cx));
    with_view(cx, handle, &view, |v, _, cx| v.extract_recent(true, cx));
    wait_for_tasks(cx, handle, &view);
    assert!(tmp.path().join("beta/proj/readme.txt").is_file());
    with_view(cx, handle, &view, |v, _, cx| v.extract_recent(false, cx));
    wait_for_tasks(cx, handle, &view);
    assert!(tmp.path().join("proj/readme.txt").is_file());

    // A rename of the file on disk follows through the list, the tab and the settings.
    with_view(cx, handle, &view, |v, w, cx| v.ask_rename_file(w, cx));
    cx.update(|cx| assert!(matches!(&view.read(cx).overlay, Some(O::Rename(_)))));
    with_view(cx, handle, &view, |v, _, cx| v.close_overlay(cx));
    with_view(cx, handle, &view, |v, _, cx| v.select_recent(a.clone(), cx));
    with_view(cx, handle, &view, |v, w, cx| v.ask_rename_file(w, cx));
    let input = cx.update(|cx| match &view.read(cx).overlay {
        Some(O::Rename(p)) => p.input.clone(),
        _ => panic!("rename card"),
    });
    cx.update_window(handle.into(), |_, window, cx| {
        input.update(cx, |s, cx| s.set_value("renamed.zip", window, cx));
    })
    .unwrap();
    with_view(cx, handle, &view, |v, w, cx| v.submit_rename(w, cx));
    cx.run_until_parked();
    std::thread::sleep(Duration::from_millis(100));
    cx.run_until_parked();
    assert!(tmp.path().join("renamed.zip").is_file());
    assert!(!a.exists());
    cx.update(|cx| {
        let v = view.read(cx);
        assert_eq!(v.current().unwrap().path, tmp.path().join("renamed.zip"));
        assert!(v.settings.recent.contains(&tmp.path().join("renamed.zip")));
    });

    // Forgetting only removes the line; the file stays.
    with_view(cx, handle, &view, |v, _, cx| v.forget_recent(&b, cx));
    assert!(b.exists());
    cx.update(|cx| assert!(!view.read(cx).settings.recent.contains(&b)));
}

#[gpui_kit::test]
fn renaming_inside_an_archive_changes_the_entry_and_refuses_bad_names(cx: &mut TestAppContext) {
    use rustzip_gui::view::Overlay as O;
    let tmp = tempfile::tempdir().unwrap();
    let archive = make_archive(tmp.path(), "a.zip", None);
    let (handle, view) = open_window(cx, None);
    open_sync(cx, handle, &view, &archive, None);
    with_view(cx, handle, &view, |v, _, cx| v.navigate("proj", cx));
    let table = cx.update(|cx| view.read(cx).table.clone());
    cx.update_window(handle.into(), |_, _, cx| {
        table.update(cx, |_, cx| cx.emit(TableEvent::SelectRow(3)));
    })
    .unwrap();

    let ask = |cx: &mut TestAppContext, name: &str| {
        with_view(cx, handle, &view, |v, w, cx| v.ask_rename_entry(w, cx));
        let input = cx.update(|cx| match &view.read(cx).overlay {
            Some(O::Rename(p)) => p.input.clone(),
            _ => panic!("rename card"),
        });
        cx.update_window(handle.into(), |_, window, cx| {
            input.update(cx, |s, cx| s.set_value(name, window, cx));
        })
        .unwrap();
        with_view(cx, handle, &view, |v, w, cx| v.submit_rename(w, cx));
    };

    // A name with a slash is refused and the card stays open with the reason.
    ask(cx, "a/b.txt");
    cx.update(|cx| {
        assert!(matches!(&view.read(cx).overlay, Some(O::Rename(p)) if p.error.is_some()));
    });
    with_view(cx, handle, &view, |v, w, cx| {
        let Some(O::Rename(p)) = &v.overlay else {
            panic!()
        };
        let input = p.input.clone();
        input.update(cx, |s, cx| s.set_value("说明书.txt", w, cx));
        v.submit_rename(w, cx);
    });
    wait_for_tasks(cx, handle, &view);
    cx.run_until_parked();
    draw(cx, handle);
    assert_eq!(row_names(cx, &view), ["docs", "src", "说明书.txt"]);
    let (tree, _) = load_archive(&archive, None).unwrap();
    assert!(tree.find("proj/说明书.txt").is_some() && tree.find("proj/readme.txt").is_none());
}

#[gpui_kit::test]
fn the_row_menu_actions_work_on_the_whole_selection(cx: &mut TestAppContext) {
    use gpui_kit::Focusable as _;
    use rustzip_gui::{RowCopyPath, RowExtractHere, RowExtractNamed, SelectAllRows};
    let tmp = tempfile::tempdir().unwrap();
    let archive = make_archive(tmp.path(), "bundle.zip", None);
    let settings_path = tmp.path().join("cfg/settings.json");
    Settings {
        // The test platform cannot show a folder.
        open_folder_when_done: false,
        ..Settings::default()
    }
    .save(&settings_path)
    .unwrap();
    let (handle, view) = open_window(cx, Some(settings_path));
    open_sync(cx, handle, &view, &archive, None);
    with_view(cx, handle, &view, |v, _, cx| v.navigate("proj", cx));
    assert_eq!(row_names(cx, &view), ["docs", "src", "readme.txt"]);

    // Actions reach the window from whatever has the focus; here, the list.
    let table = cx.update(|cx| view.read(cx).table.clone());
    cx.update_window(handle.into(), |_, window, cx| {
        let focus = table.read(cx).focus_handle(cx);
        window.focus(&focus, cx);
    })
    .unwrap();

    // Select all, through the same action the keyboard shortcut sends.
    cx.dispatch_action(handle.into(), SelectAllRows);
    draw(cx, handle);
    let picked = cx.update(|cx| view.read(cx).picked_paths(cx));
    assert_eq!(picked, ["proj/docs", "proj/readme.txt", "proj/src"]);

    // Copy the paths of everything picked.
    cx.dispatch_action(handle.into(), RowCopyPath);
    let copied = cx.read_from_clipboard().and_then(|c| c.text());
    assert_eq!(
        copied.as_deref(),
        Some("proj/docs\nproj/readme.txt\nproj/src")
    );

    // Extract all of it into a folder named after the archive.
    cx.dispatch_action(handle.into(), RowExtractNamed);
    wait_for_tasks(cx, handle, &view);
    for f in [
        "bundle/proj/readme.txt",
        "bundle/proj/docs/说明.md",
        "bundle/proj/src/main.rs",
    ] {
        assert!(tmp.path().join(f).is_file(), "{f}");
    }

    // Only src and readme.txt picked: "extract here" next to the archive leaves docs out.
    with_view(cx, handle, &view, |v, _, cx| {
        v.table
            .update(cx, |t, _| t.delegate_mut().mark_only("proj/src"));
        // Row 2 is readme.txt.
        v.table.update(cx, |t, _| t.delegate_mut().toggle(3));
    });
    cx.dispatch_action(handle.into(), RowExtractHere);
    wait_for_tasks(cx, handle, &view);
    assert!(tmp.path().join("proj/src/lib.rs").is_file());
    assert!(tmp.path().join("proj/readme.txt").is_file());
    assert!(!tmp.path().join("proj/docs").exists());
}

#[gpui_kit::test]
fn a_right_click_on_the_empty_part_lets_go_of_the_selection(cx: &mut TestAppContext) {
    let tmp = tempfile::tempdir().unwrap();
    let archive = make_archive(tmp.path(), "a.zip", None);
    let (handle, view) = open_window(cx, None);
    open_sync(cx, handle, &view, &archive, None);
    with_view(cx, handle, &view, |v, _, cx| v.navigate("proj", cx));
    let table = cx.update(|cx| view.read(cx).table.clone());
    cx.update_window(handle.into(), |_, _, cx| {
        table.update(cx, |t, cx| t.set_selected_row(3, cx));
    })
    .unwrap();
    draw(cx, handle);
    cx.update(|cx| {
        assert!(view.read(cx).selected_row().is_some());
        assert_eq!(view.read(cx).picked_paths(cx), ["proj/readme.txt"]);
    });

    // What the list's menu does for the empty part: report that nothing is selected.
    cx.update_window(handle.into(), |_, _, cx| {
        table.update(cx, |_, cx| cx.emit(TableEvent::ClearSelection));
    })
    .unwrap();
    draw(cx, handle);
    cx.update(|cx| {
        let v = view.read(cx);
        assert!(v.selected_row().is_none());
        assert!(v.picked_paths(cx).is_empty());
        assert!(v.preview.is_none());
        assert!(table.read(cx).selected_row().is_none());
    });
    // Picking a row afterwards still works.
    cx.update_window(handle.into(), |_, _, cx| {
        table.update(cx, |t, cx| t.set_selected_row(1, cx));
    })
    .unwrap();
    draw(cx, handle);
    cx.update(|cx| assert_eq!(view.read(cx).picked_paths(cx), ["proj/docs"]));
}

#[gpui_kit::test]
fn the_back_row_goes_up_and_out_of_the_archive_into_its_folder(cx: &mut TestAppContext) {
    use rustzip_gui::view::Page;
    let tmp = tempfile::tempdir().unwrap();
    let archive = make_archive(tmp.path(), "a.zip", None);
    let (handle, view) = open_window(cx, None);
    open_sync(cx, handle, &view, &archive, None);
    let table = cx.update(|cx| view.read(cx).table.clone());
    let double = |cx: &mut TestAppContext, ix: usize| {
        cx.update_window(handle.into(), |_, _, cx| {
            table.update(cx, |_, cx| cx.emit(TableEvent::DoubleClickedRow(ix)));
        })
        .unwrap();
        cx.run_until_parked();
        std::thread::sleep(Duration::from_millis(50));
        cx.run_until_parked();
        draw(cx, handle);
    };

    // The back row is the first row, though it is not an entry of the archive.
    assert_eq!(row_names(cx, &view), ["proj"]);
    cx.update(|cx| {
        let d = view.read(cx).table.read(cx).delegate();
        assert_eq!(d.table_len(), 2);
        assert!(d.row(0).is_none() && d.row(1).is_some());
    });

    // Inside a folder it goes one level up.
    double(cx, 1);
    cx.update(|cx| assert_eq!(view.read(cx).current().unwrap().cwd, "proj"));
    double(cx, 0);
    cx.update(|cx| assert_eq!(view.read(cx).current().unwrap().cwd, ""));

    // At the top it leaves the archive for the folder the archive is in.
    double(cx, 0);
    cx.update(|cx| {
        let v = view.read(cx);
        assert_eq!(v.page, Page::Folder);
        assert_eq!(v.folder.as_deref(), Some(tmp.path()));
        let names: Vec<_> = v.files.items.iter().map(|i| i.name.as_str()).collect();
        assert!(names.contains(&"a.zip"), "{names:?}");
        // Folders first.
        assert!(v.files.items[0].is_dir);
    });

    // An archive in that folder opens in a tab of its own.
    let other = make_archive(tmp.path(), "second.zip", None);
    with_view(cx, handle, &view, |v, _, cx| v.reload_folder(cx));
    std::thread::sleep(Duration::from_millis(50));
    cx.run_until_parked();
    with_view(cx, handle, &view, |v, w, cx| {
        v.open_listed(other.clone(), false, w, cx);
    });
    wait_until(cx, handle, |cx| {
        cx.update(|cx| view.read(cx).archives.len() == 2)
    });
    cx.update(|cx| assert_eq!(view.read(cx).page, Page::Archive));

    // And ".." in the folder page climbs on.
    with_view(cx, handle, &view, |v, _, cx| {
        v.show_folder(tmp.path().join("cfg-none"), cx)
    });
    with_view(cx, handle, &view, |v, _, cx| v.folder_up(cx));
    cx.update(|cx| assert_eq!(view.read(cx).folder.as_deref(), Some(tmp.path())));
}

#[gpui_kit::test]
fn dragging_a_band_over_the_list_picks_the_rows_it_touches(cx: &mut TestAppContext) {
    use gpui_kit::{
        Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PlatformInput, point,
        px,
    };
    let tmp = tempfile::tempdir().unwrap();
    let archive = make_archive(tmp.path(), "a.zip", None);
    let (handle, view) = open_window(cx, None);
    open_sync(cx, handle, &view, &archive, None);
    with_view(cx, handle, &view, |v, _, cx| v.navigate("proj", cx));
    // A frame to learn where the list is, and a second one with it laid out.
    draw(cx, handle);
    draw(cx, handle);
    let bounds = cx.update(|cx| view.read(cx).list_bounds());
    assert!(bounds.size.height > px(200.), "{bounds:?}");

    // Title row, back row, then docs, src, readme.txt; the rest of the list is empty.
    let row_y = |ix: usize| bounds.origin.y + px(28. + 28. * ix as f32 + 14.);
    let x = bounds.origin.x + px(200.);
    let send = |cx: &mut TestAppContext, event: PlatformInput| {
        cx.update_window(handle.into(), |_, window, cx| {
            window.dispatch_event(event, cx);
        })
        .unwrap();
        draw(cx, handle);
    };
    let down = |y| {
        PlatformInput::MouseDown(MouseDownEvent {
            button: MouseButton::Left,
            position: point(x, y),
            modifiers: Modifiers::default(),
            click_count: 1,
            first_mouse: false,
        })
    };
    let moved = |y| {
        PlatformInput::MouseMove(MouseMoveEvent {
            position: point(x, y),
            pressed_button: Some(MouseButton::Left),
            modifiers: Modifiers::default(),
        })
    };
    let up = |y| {
        PlatformInput::MouseUp(MouseUpEvent {
            button: MouseButton::Left,
            position: point(x, y),
            modifiers: Modifiers::default(),
            click_count: 1,
        })
    };
    let picked = |cx: &mut TestAppContext| cx.update(|cx| view.read(cx).picked_paths(cx));

    // From the empty part, up over the last two rows: those two are picked.
    let start = row_y(8);
    send(cx, down(start));
    send(cx, moved(row_y(3)));
    cx.update(|cx| assert!(view.read(cx).marquee.is_some()));
    // Row 3 is readme.txt, the last one.
    assert_eq!(picked(cx), ["proj/readme.txt"]);
    send(cx, moved(row_y(2)));
    assert_eq!(picked(cx), ["proj/readme.txt", "proj/src"]);
    // Further up, over the back row and the first row: everything but the back row.
    send(cx, moved(row_y(0)));
    assert_eq!(picked(cx), ["proj/docs", "proj/readme.txt", "proj/src"]);
    // Back down to the empty part: nothing.
    send(cx, moved(row_y(7)));
    assert!(picked(cx).is_empty());
    send(cx, moved(row_y(2)));
    send(cx, up(row_y(2)));
    cx.update(|cx| assert!(view.read(cx).marquee.is_none()));
    assert_eq!(picked(cx), ["proj/readme.txt", "proj/src"]);

    // A plain click on the empty part lets go of the selection.
    send(cx, down(start));
    send(cx, up(start));
    assert!(picked(cx).is_empty());
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
        table.update(cx, |_, cx| cx.emit(TableEvent::SelectRow(3)));
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
        table.update(cx, |_, cx| cx.emit(TableEvent::DoubleClickedRow(3)));
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
        // A finished task leaves the bottom panel and goes to the history.
        assert!(!v.task_view.iter().any(rustzip_gui::history::is_live));
        assert_eq!(v.history.records.len(), 1);
        assert_eq!(
            v.history.records[0].outcome,
            rz_app::history::Outcome::Success
        );
        assert_eq!(v.history_badges(), (0, 0, 1));
    });
    // A completed task can reveal its in-session result; returning clears the badges.
    let task_id = cx.update(|cx| view.read(cx).history.records[0].task.unwrap());
    with_view(cx, handle, &view, |v, _, cx| {
        v.show_page(rustzip_gui::view::Page::History, cx)
    });
    draw(cx, handle);
    cx.update_window(handle.into(), |_, window, _| {
        assert!(window.find(("history-reveal", task_id as usize)).visible());
    })
    .unwrap();
    with_view(cx, handle, &view, |v, _, cx| {
        v.show_page(rustzip_gui::view::Page::Archive, cx);
    });
    cx.update(|cx| assert_eq!(view.read(cx).history_badges(), (0, 0, 0)));
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
        assert!(matches!(&v.notice, Some(Notice::Error(m)) if m.contains("不是一个压缩包")));
    });
    // A folder gets the same plain explanation, not the raw OS error.
    open_sync(cx, handle, &view, tmp.path(), None);
    cx.update(|cx| {
        let v = view.read(cx);
        assert!(v.archives.is_empty());
        assert!(matches!(&v.notice, Some(Notice::Error(m)) if m.contains("不是一个压缩包")));
    });
}

#[gpui_kit::test]
fn several_archives_keep_their_own_folder_and_tabs_close_cleanly(cx: &mut TestAppContext) {
    let tmp = tempfile::tempdir().unwrap();
    let a = make_archive(tmp.path(), "a.zip", None);
    let b = tmp.path().join("b.tar.gz");
    compress(
        &[tmp.path().join("a.zip.src/proj/docs")],
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
fn out_of_order_archive_open_completions_keep_the_latest_request_active(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let a = make_archive(dir.path(), "slow-a.zip", None);
    let b = make_archive(dir.path(), "fast-b.zip", None);
    let (handle, view) = open_window(cx, None);

    let (request_a, start_a) = with_view(cx, handle, &view, |v, _, _| v.begin_open_request(&a));
    let (request_b, start_b) = with_view(cx, handle, &view, |v, _, _| v.begin_open_request(&b));
    assert!(start_a && start_b);

    let result_b = load_archive(&b, None);
    with_view(cx, handle, &view, |v, window, cx| {
        v.finish_open(request_b, b.clone(), None, result_b, window, cx);
    });
    let result_a = load_archive(&a, None);
    with_view(cx, handle, &view, |v, window, cx| {
        v.finish_open(request_a, a.clone(), None, result_a, window, cx);
    });

    cx.update(|cx| {
        let v = view.read(cx);
        assert_eq!(v.archives.len(), 2);
        assert_eq!(v.current().unwrap().path, b);
        assert_eq!(v.loading, None);
    });

    let (same_request, should_start) =
        with_view(cx, handle, &view, |v, _, _| v.begin_open_request(&a));
    let (same_request_again, should_start_again) =
        with_view(cx, handle, &view, |v, _, _| v.begin_open_request(&a));
    assert_eq!(same_request, same_request_again);
    assert!(should_start);
    assert!(!should_start_again);
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
    let (dictionary_size, solid_block_size) = cx.update(|cx| match &view.read(cx).overlay {
        Some(Overlay::Compress(f)) => (f.dictionary_size.clone(), f.solid_block_size.clone()),
        _ => panic!("no compress form"),
    });
    cx.update_window(handle.into(), |_, window, cx| {
        dictionary_size.update(cx, |s, cx| s.set_value("2", window, cx));
        solid_block_size.update(cx, |s, cx| s.set_value("1", window, cx));
    })
    .unwrap();
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
fn email_share_preset_uses_adjustable_twenty_mib_zip_parts(cx: &mut TestAppContext) {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("data");
    fs::create_dir_all(&src).unwrap();
    let mut seed = 0x1234_5678_9abc_def0u64;
    let mut bytes = Vec::with_capacity(21 << 20);
    for _ in 0..(21 << 20) {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        bytes.push(seed as u8);
    }
    fs::write(src.join("random.bin"), bytes).unwrap();
    let (handle, view) = open_window(cx, None);
    with_view(cx, handle, &view, |v, window, cx| {
        v.open_compress(vec![src.clone()], window, cx);
        v.set_preset(Preset::EmailShare, cx);
    });
    cx.update(|cx| {
        let Some(Overlay::Compress(form)) = &view.read(cx).overlay else {
            panic!("no compress form");
        };
        assert_eq!(form.preset, Preset::EmailShare);
        assert_eq!(form.format, Format::Zip);
    });

    with_view(cx, handle, &view, |v, _, cx| {
        v.set_format(Format::TarZst, cx);
    });
    cx.update(|cx| {
        let Some(Overlay::Compress(form)) = &view.read(cx).overlay else {
            panic!("no compress form");
        };
        assert_eq!(form.preset, Preset::Balanced);
    });
    with_view(cx, handle, &view, |v, _, cx| {
        v.set_preset(Preset::Portable, cx);
    });
    cx.update(|cx| {
        let Some(Overlay::Compress(form)) = &view.read(cx).overlay else {
            panic!("no compress form");
        };
        assert_eq!(form.preset, Preset::Portable);
    });
    with_view(cx, handle, &view, |v, _, cx| {
        v.set_format(Format::Zip, cx);
        v.set_preset(Preset::EmailShare, cx);
        v.submit_compress(cx);
    });
    wait_for_tasks(cx, handle, &view);
    assert!(tmp.path().join("data.z01").is_file());
    assert!(tmp.path().join("data.zip").is_file());
    assert!(load_archive(&tmp.path().join("data.zip"), None).is_ok());

    with_view(cx, handle, &view, |v, window, cx| {
        v.open_compress(vec![src.clone()], window, cx);
        v.set_preset(Preset::EmailShare, cx);
    });
    let (name, volume_size) = cx.update(|cx| match &view.read(cx).overlay {
        Some(Overlay::Compress(form)) => (form.name.clone(), form.volume_size.clone()),
        _ => panic!("no compress form"),
    });
    cx.update_window(handle.into(), |_, window, cx| {
        name.update(cx, |input, cx| input.set_value("custom", window, cx));
        volume_size.update(cx, |input, cx| input.set_value("25", window, cx));
    })
    .unwrap();
    with_view(cx, handle, &view, |v, _, cx| v.submit_compress(cx));
    wait_for_tasks(cx, handle, &view);
    assert!(tmp.path().join("custom.zip").is_file());
    assert!(!tmp.path().join("custom.z01").exists());
}

#[gpui_kit::test]
fn compression_prompts_before_packing_content_duplicates(cx: &mut TestAppContext) {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("data");
    fs::create_dir_all(src.join("one")).unwrap();
    fs::create_dir_all(src.join("two")).unwrap();
    fs::write(src.join("one/a.txt"), b"same content").unwrap();
    fs::write(src.join("two/b.txt"), b"same content").unwrap();
    let output = tmp.path().join("data.zip");
    let (handle, view) = open_window(cx, None);

    with_view(cx, handle, &view, |v, window, cx| {
        v.open_compress(vec![src.clone()], window, cx);
        v.submit_compress(cx);
    });
    wait_for_tasks(cx, handle, &view);
    cx.update(|cx| match &view.read(cx).overlay {
        Some(Overlay::DuplicatePrompt { groups, .. }) => {
            assert_eq!(groups.len(), 1);
            assert_eq!(groups[0].duplicate_count(), 1);
        }
        _ => panic!("expected a duplicate prompt"),
    });
    assert!(!output.exists());

    with_view(cx, handle, &view, |v, _, cx| {
        v.cancel_duplicate_compression(cx);
    });
    assert!(!output.exists());

    with_view(cx, handle, &view, |v, window, cx| {
        v.open_compress(vec![src.clone()], window, cx);
        v.submit_compress(cx);
    });
    wait_for_tasks(cx, handle, &view);
    with_view(cx, handle, &view, |v, _, cx| {
        v.continue_duplicate_compression(cx);
    });
    wait_for_tasks(cx, handle, &view);
    let (tree, _) = load_archive(&output, None).unwrap();
    assert_eq!(tree.total_files(), 2);
}

#[gpui_kit::test]
fn the_compress_form_refuses_bad_names_and_numbers_taken_ones(cx: &mut TestAppContext) {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("data");
    fs::create_dir_all(&src).unwrap();
    fs::write(src.join("a.txt"), "a").unwrap();
    fs::write(tmp.path().join("data.zip"), "already here").unwrap();
    let (handle, view) = open_window(cx, None);
    with_view(cx, handle, &view, |v, window, cx| {
        v.open_compress(vec![src.clone()], window, cx);
    });

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

    // "data.zip" exists next to the source: the new archive is numbered, nothing is lost.
    cx.update_window(handle.into(), |_, window, cx| {
        let name = match &view.read(cx).overlay {
            Some(Overlay::Compress(f)) => f.name.clone(),
            _ => panic!("no form"),
        };
        name.update(cx, |s, cx| s.set_value("data", window, cx));
    })
    .unwrap();
    with_view(cx, handle, &view, |v, _, cx| v.submit_compress(cx));
    wait_for_tasks(cx, handle, &view);
    assert_eq!(
        fs::read_to_string(tmp.path().join("data.zip")).unwrap(),
        "already here"
    );
    assert!(load_archive(&tmp.path().join("data (1).zip"), None).is_ok());
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
    cx.run_until_parked();
    assert_eq!(Settings::load(&settings).recent, vec![archive.clone()]);

    // A second window starts with the saved list.
    let (_handle2, view2) = open_window(cx, Some(settings));
    cx.update(|cx| assert_eq!(view2.read(cx).settings.recent, vec![archive]));
}

#[gpui_kit::test]
fn favorites_work_without_an_open_archive_and_survive_restart(cx: &mut TestAppContext) {
    use rustzip_gui::view::Page;
    let tmp = tempfile::tempdir().unwrap();
    let folder = fs::canonicalize(tmp.path()).unwrap();
    let path = tmp.path().join("cfg/settings.json");
    let (handle, view) = open_window(cx, Some(path.clone()));
    with_view(cx, handle, &view, |v, _, cx| {
        v.show_page(Page::Favorites, cx);
        v.pick_favorite(cx);
    });
    cx.simulate_path_prompt_response(|_| Some(vec![folder.clone(), folder.clone()]));
    cx.run_until_parked();
    assert_eq!(Settings::load(&path).favorites, vec![folder.clone()]);
    with_view(cx, handle, &view, |v, _, cx| {
        v.open_favorite(folder.clone(), cx)
    });
    cx.run_until_parked();
    cx.update(|cx| {
        assert_eq!(view.read(cx).page, Page::Folder);
        assert_eq!(view.read(cx).folder, Some(folder.clone()));
    });
    let (_, restarted) = open_window(cx, Some(path.clone()));
    cx.update(|cx| assert_eq!(restarted.read(cx).settings.favorites, vec![folder.clone()]));
    with_view(cx, handle, &view, |v, _, cx| v.remove_favorite(&folder, cx));
    cx.run_until_parked();
    assert!(Settings::load(&path).favorites.is_empty());
    assert!(folder.is_dir());
}

#[gpui_kit::test]
fn opened_archive_parents_appear_automatically_and_can_be_hidden_until_reopened(
    cx: &mut TestAppContext,
) {
    use rustzip_gui::view::Page;
    let tmp = tempfile::tempdir().unwrap();
    let archive = make_archive(tmp.path(), "common.zip", None);
    let settings_path = tmp.path().join("settings.json");
    let (handle, view) = open_window(cx, Some(settings_path.clone()));
    open_sync(cx, handle, &view, &archive, None);
    with_view(cx, handle, &view, |v, _, cx| {
        v.show_page(Page::Favorites, cx)
    });
    cx.run_until_parked();
    cx.update(|cx| {
        assert_eq!(
            view.read(cx)
                .locations
                .list
                .items
                .iter()
                .filter(|i| i.path == tmp.path())
                .count(),
            1
        )
    });
    draw(cx, handle);
    let index = cx.update(|cx| {
        view.read(cx)
            .locations
            .list
            .items
            .iter()
            .position(|i| i.path == tmp.path())
            .unwrap()
    });
    cx.update_window(handle.into(), |_, window, cx| {
        window.right_click(("directory-row", index), cx);
    })
    .unwrap();
    cx.run_until_parked();
    draw(cx, handle);
    cx.update_window(handle.into(), |_, window, cx| {
        window.within("popup-menu").click(4usize, cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update(|cx| {
        assert!(
            !view
                .read(cx)
                .locations
                .list
                .items
                .iter()
                .any(|i| i.path == tmp.path())
        )
    });
    assert!(archive.exists());
    let restored = Settings::load(&settings_path);
    assert!(
        !rustzip_gui::locations::scan(&restored)
            .iter()
            .any(|i| i.path == tmp.path())
    );
    with_view(cx, handle, &view, |v, window, cx| {
        v.open_path(archive.clone(), None, window, cx)
    });
    with_view(cx, handle, &view, |v, _, cx| {
        v.show_page(Page::Favorites, cx)
    });
    cx.run_until_parked();
    cx.update(|cx| {
        assert!(
            view.read(cx)
                .locations
                .list
                .items
                .iter()
                .any(|i| i.path == tmp.path())
        )
    });
}

#[gpui_kit::test]
fn location_list_and_icons_share_selection_navigation_and_persist_the_view(
    cx: &mut TestAppContext,
) {
    use rustzip_gui::view::Page;
    use rz_app::settings::FolderView;
    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path().join("project");
    fs::create_dir_all(folder.join("subfolder")).unwrap();
    let archive = make_archive(&folder, "nested.zip", None);
    let path = tmp.path().join("settings.json");
    Settings {
        favorites: vec![folder.clone()],
        ..Settings::default()
    }
    .save(&path)
    .unwrap();
    let (handle, view) = open_window(cx, Some(path.clone()));
    with_view(cx, handle, &view, |v, _, cx| {
        v.show_page(Page::Favorites, cx)
    });
    cx.run_until_parked();
    draw(cx, handle);
    let index = cx.update(|cx| {
        view.read(cx)
            .locations
            .list
            .items
            .iter()
            .position(|i| i.path == folder)
            .unwrap()
    });
    cx.update_window(handle.into(), |_, window, cx| {
        window.click(("directory-row", index), cx);
        window.click("view-icons", cx);
        assert!(window.find(("directory-icon", index)).visible());
        assert!(window.find("view-icons").bounds().origin.x > gpui_kit::px(900.));
    })
    .unwrap();
    cx.update(|cx| {
        assert_eq!(view.read(cx).locations.list.selected, Some(folder.clone()));
        assert_eq!(view.read(cx).page, Page::Favorites);
    });
    cx.update_window(handle.into(), |_, window, cx| {
        window.double_click(("directory-icon", index), cx)
    })
    .unwrap();
    cx.run_until_parked();
    draw(cx, handle);
    cx.update(|cx| {
        assert_eq!(view.read(cx).page, Page::Folder);
        assert_eq!(view.read(cx).folder, Some(folder.clone()));
        assert_eq!(view.read(cx).settings.folder_view, FolderView::Icons);
    });
    cx.update_window(handle.into(), |_, window, _| {
        assert!(window.find("view-icons").bounds().origin.x > gpui_kit::px(900.))
    })
    .unwrap();
    let sub = cx.update(|cx| {
        view.read(cx)
            .files
            .items
            .iter()
            .position(|i| i.name == "subfolder")
            .unwrap()
    });
    cx.update_window(handle.into(), |_, window, cx| {
        window.double_click(("directory-icon", sub), cx)
    })
    .unwrap();
    cx.run_until_parked();
    draw(cx, handle);
    cx.update_window(handle.into(), |_, window, cx| {
        window.double_click("folder-up", cx)
    })
    .unwrap();
    cx.run_until_parked();
    draw(cx, handle);
    cx.update(|cx| assert_eq!(view.read(cx).folder, Some(folder.clone())));
    let file = cx.update(|cx| {
        view.read(cx)
            .files
            .items
            .iter()
            .position(|i| i.path == archive)
            .unwrap()
    });
    cx.update_window(handle.into(), |_, window, cx| {
        window.double_click(("directory-icon", file), cx)
    })
    .unwrap();
    cx.run_until_parked();
    cx.update(|cx| assert_eq!(view.read(cx).page, Page::Archive));
    cx.run_until_parked();
    assert_eq!(Settings::load(&path).folder_view, FolderView::Icons);
    let (_, restarted) = open_window(cx, Some(path));
    cx.update(|cx| assert_eq!(restarted.read(cx).settings.folder_view, FolderView::Icons));
}

#[gpui_kit::test]
fn directory_icons_are_virtualized_and_respond_to_narrow_windows(cx: &mut TestAppContext) {
    use rustzip_gui::view::Page;
    use rz_app::settings::FolderView;
    let tmp = tempfile::tempdir().unwrap();
    let (handle, view) = open_window(cx, None);
    with_view(cx, handle, &view, |v, _, cx| {
        v.show_page(Page::Favorites, cx)
    });
    cx.run_until_parked();
    with_view(cx, handle, &view, |v, _, cx| {
        v.locations.list.items = (0..10000)
            .map(|ix| rustzip_gui::recent::RecentItem {
                path: tmp.path().join(format!("folder-{ix}")),
                name: format!("folder-{ix}"),
                folder: String::new(),
                created: None,
                modified: None,
                size: None,
                exists: true,
                is_dir: true,
            })
            .collect();
        v.set_folder_view(FolderView::Icons, cx);
    });
    cx.update_window(handle.into(), |_, window, _| {
        assert!(window.find(("directory-icon", 0usize)).visible());
        assert!(window.try_find(("directory-icon", 9999usize)).is_none());
        assert!(
            window.find(("directory-icon", 1usize)).bounds().origin.x
                > window.find(("directory-icon", 0usize)).bounds().origin.x
        );
    })
    .unwrap();
    cx.simulate_window_resize(
        handle.into(),
        gpui_kit::size(gpui_kit::px(640.), gpui_kit::px(420.)),
    );
    draw(cx, handle);
    cx.update_window(handle.into(), |_, window, _| {
        let first = window.find(("directory-icon", 0usize)).bounds();
        let next_row = window.find(("directory-icon", 4usize)).bounds();
        assert!(next_row.origin.y > first.origin.y);
        assert!(window.find("view-icons").bounds().right() <= gpui_kit::px(640.));
    })
    .unwrap();
}

#[gpui_kit::test]
fn directory_sorting_and_keyboard_opening_work_in_both_views(cx: &mut TestAppContext) {
    use rustzip_gui::view::Page;
    use rz_app::settings::FolderView;
    let tmp = tempfile::tempdir().unwrap();
    for name in ["b", "a"] {
        fs::create_dir(tmp.path().join(name)).unwrap();
    }
    let (handle, view) = open_window(cx, None);
    with_view(cx, handle, &view, |v, _, cx| {
        v.show_folder(tmp.path().to_path_buf(), cx)
    });
    cx.run_until_parked();
    draw(cx, handle);
    cx.update_window(handle.into(), |_, window, cx| {
        window.click(("directory-row", 0usize), cx);
        window.press("down", cx);
    })
    .unwrap();
    cx.update(|cx| assert_eq!(view.read(cx).files.selected, Some(tmp.path().join("b"))));
    cx.update_window(handle.into(), |_, window, cx| window.press("enter", cx))
        .unwrap();
    cx.run_until_parked();
    cx.update(|cx| assert_eq!(view.read(cx).folder, Some(tmp.path().join("b"))));
    with_view(cx, handle, &view, |v, _, cx| {
        v.settings.favorites = vec![tmp.path().join("b"), tmp.path().join("a")];
        v.show_page(Page::Favorites, cx);
    });
    cx.run_until_parked();
    with_view(cx, handle, &view, |v, _, cx| {
        v.sort_directory(rustzip_gui::recent::RecentSort::Name, cx)
    });
    cx.update(|cx| assert!(view.read(cx).locations.list.ascending));
    with_view(cx, handle, &view, |v, _, cx| {
        v.set_folder_view(FolderView::Icons, cx)
    });
    let index = cx.update(|cx| {
        view.read(cx)
            .locations
            .list
            .items
            .iter()
            .position(|i| i.path == tmp.path().join("a"))
            .unwrap()
    });
    cx.update_window(handle.into(), |_, window, cx| {
        window.click(("directory-icon", index), cx);
        window.press("enter", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update(|cx| assert_eq!(view.read(cx).folder, Some(tmp.path().join("a"))));
}

#[gpui_kit::test]
fn startup_navigation_keeps_actions_below_the_window_controls(cx: &mut TestAppContext) {
    use rustzip_gui::view::{Page, TABS_IN_TITLEBAR};
    let (handle, view) = open_window(cx, None);
    draw(cx, handle);
    cx.update_window(handle.into(), |_, window, _| {
        assert_eq!(window.find("nav-current").label(), Some("启动页"));
        if TABS_IN_TITLEBAR {
            assert!(window.find("nav-current").bounds().origin.y >= gpui_kit::px(38.));
        }
    })
    .unwrap();
    for (page, action) in [
        (Page::Recent, "recent-open-other"),
        (Page::Favorites, "tool-add"),
        (Page::Passwords, "tool-add"),
    ] {
        with_view(cx, handle, &view, |v, _, cx| v.show_page(page, cx));
        cx.update_window(handle.into(), |_, window, _| {
            let bounds = window.find(action).bounds();
            if TABS_IN_TITLEBAR {
                assert!(
                    bounds.origin.y >= gpui_kit::px(38.),
                    "{page:?} action overlaps native controls: {bounds:?}"
                );
            }
        })
        .unwrap();
    }
    cx.update_window(handle.into(), |_, window, cx| {
        window.click("nav-current", cx);
        assert!(window.find("empty-open").visible());
    })
    .unwrap();
    cx.update(|cx| assert_eq!(view.read(cx).page, Page::Archive));
    let tmp = tempfile::tempdir().unwrap();
    with_view(cx, handle, &view, |v, _, cx| {
        v.show_folder(tmp.path().to_path_buf(), cx)
    });
    cx.update_window(handle.into(), |_, window, _| {
        if TABS_IN_TITLEBAR {
            assert!(window.find("folder-open-other").bounds().origin.y >= gpui_kit::px(38.));
        }
    })
    .unwrap();
}

#[gpui_kit::test]
fn closing_the_last_archive_keeps_sidebar_navigation_and_titlebar_space(cx: &mut TestAppContext) {
    use rustzip_gui::view::{Page, TABS_IN_TITLEBAR};
    let tmp = tempfile::tempdir().unwrap();
    let archive = make_archive(tmp.path(), "navigation.zip", None);
    let (handle, view) = open_window(cx, None);
    let pages = [
        (Page::Recent, "recent-open-other"),
        (Page::Favorites, "tool-add"),
        (Page::Passwords, "tool-add"),
    ];
    let mut empty_positions = Vec::new();
    for &(page, action) in &pages {
        with_view(cx, handle, &view, |v, _, cx| v.show_page(page, cx));
        empty_positions.push(
            cx.update_window(handle.into(), |_, window, _| {
                window.find(action).bounds().origin.y
            })
            .unwrap(),
        );
    }
    open_sync(cx, handle, &view, &archive, None);
    cx.update_window(handle.into(), |_, window, _| {
        assert_eq!(window.find("nav-current").label(), Some("当前压缩包"))
    })
    .unwrap();
    for (ix, &(page, action)) in pages.iter().enumerate() {
        with_view(cx, handle, &view, |v, _, cx| v.show_page(page, cx));
        cx.update_window(handle.into(), |_, window, _| {
            let actual = window.find(action).bounds().origin.y;
            let expected =
                empty_positions[ix] + gpui_kit::px(if TABS_IN_TITLEBAR { 0. } else { 38. });
            assert_eq!(
                actual, expected,
                "{page:?} toolbar moved when an archive was opened"
            );
        })
        .unwrap();
    }
    with_view(cx, handle, &view, |v, _, cx| v.close_tab(0, cx));
    cx.update_window(handle.into(), |_, window, _| {
        assert_eq!(window.find("nav-current").label(), Some("启动页"))
    })
    .unwrap();
    with_view(cx, handle, &view, |v, _, cx| v.show_page(Page::Recent, cx));
    cx.update_window(handle.into(), |_, window, _| {
        assert_eq!(
            window.find("recent-open-other").bounds().origin.y,
            empty_positions[0]
        )
    })
    .unwrap();
}

#[gpui_kit::test]
fn password_book_creates_edits_and_deletes_with_confirmation(cx: &mut TestAppContext) {
    use rustzip_gui::view::Page;
    let (handle, view) = open_window(cx, None);
    with_view(cx, handle, &view, |v, window, cx| {
        v.show_page(Page::Passwords, cx);
        v.edit_password(None, window, cx);
        if let Some(Overlay::PasswordEdit(f)) = &v.overlay {
            f.label
                .update(cx, |s, cx| s.set_value("work.zip", window, cx));
            f.secret
                .update(cx, |s, cx| s.set_value("first-secret", window, cx));
        }
        v.save_password(cx);
    });
    cx.run_until_parked();
    cx.update(|cx| {
        let v = view.read(cx);
        assert_eq!(v.tools.labels, ["work.zip"]);
        assert!(v.overlay.is_none());
        assert_eq!(v.vault.get("work.zip").as_deref(), Some("first-secret"));
    });
    with_view(cx, handle, &view, |v, window, cx| {
        v.edit_password(Some("work.zip".into()), window, cx)
    });
    cx.run_until_parked();
    with_view(cx, handle, &view, |v, window, cx| {
        let Some(Overlay::PasswordEdit(f)) = &v.overlay else {
            panic!("missing password editor");
        };
        assert_eq!(f.secret.read(cx).value().as_str(), "first-secret");
        f.secret
            .update(cx, |s, cx| s.set_value("changed-secret", window, cx));
        v.save_password(cx);
    });
    cx.run_until_parked();
    cx.update(|cx| {
        assert_eq!(
            view.read(cx).vault.get("work.zip").as_deref(),
            Some("changed-secret")
        )
    });
    with_view(cx, handle, &view, |v, _, cx| {
        v.overlay = Some(Overlay::PasswordDelete("work.zip".into()));
        v.close_overlay(cx);
    });
    cx.update(|cx| assert!(view.read(cx).vault.get("work.zip").is_some()));
    with_view(cx, handle, &view, |v, _, cx| {
        v.overlay = Some(Overlay::PasswordDelete("work.zip".into()));
        v.delete_saved_password(cx);
    });
    cx.run_until_parked();
    cx.update(|cx| {
        assert!(view.read(cx).tools.labels.is_empty());
        assert!(view.read(cx).vault.get("work.zip").is_none());
    });
}

struct LockedStore;
impl rz_app::passwords::SecretStore for LockedStore {
    fn get(&self, _: &str) -> Option<String> {
        None
    }
    fn labels(&self) -> Vec<String> {
        Vec::new()
    }
    fn set(&self, _: &str, _: &str) -> rz_core::Result<()> {
        Err(std::io::Error::other("locked").into())
    }
    fn delete(&self, _: &str) -> rz_core::Result<()> {
        Err(std::io::Error::other("locked").into())
    }
    fn try_labels(&self) -> rz_core::Result<Vec<String>> {
        Err(std::io::Error::other("locked").into())
    }
}

#[gpui_kit::test]
fn locked_keychain_is_an_error_and_failed_save_keeps_the_form(cx: &mut TestAppContext) {
    use rustzip_gui::view::Page;
    let (handle, view) = open_window(cx, None);
    with_view(cx, handle, &view, |v, window, cx| {
        v.vault = std::sync::Arc::new(LockedStore);
        v.show_page(Page::Passwords, cx);
        v.edit_password(None, window, cx);
        if let Some(Overlay::PasswordEdit(f)) = &v.overlay {
            f.label
                .update(cx, |s, cx| s.set_value("work.zip", window, cx));
            f.secret
                .update(cx, |s, cx| s.set_value("secret", window, cx));
        }
        v.save_password(cx);
    });
    cx.run_until_parked();
    cx.update(|cx| {
        let v = view.read(cx);
        assert!(v.tools.error.is_some());
        assert!(!v.tools.busy);
        assert!(matches!(&v.overlay, Some(Overlay::PasswordEdit(f)) if f.error.is_some()));
    });
}

#[gpui_kit::test]
fn conversion_form_runs_a_real_job_and_cancelled_save_keeps_the_form(cx: &mut TestAppContext) {
    let tmp = tempfile::tempdir().unwrap();
    let source = make_archive(tmp.path(), "source.zip", None);
    let output = tmp.path().join("converted.7z");
    let (handle, view) = open_window(cx, None);
    with_view(cx, handle, &view, |v, window, cx| {
        v.open_transform(false, window, cx);
        if let Some(Overlay::Transform(f)) = &mut v.overlay {
            f.source = Some(source.clone());
            f.format = "7z";
        }
        v.submit_transform(cx);
    });
    cx.simulate_new_path_selection(|_| None);
    cx.run_until_parked();
    cx.update(|cx| assert!(matches!(view.read(cx).overlay, Some(Overlay::Transform(_)))));
    with_view(cx, handle, &view, |v, _, cx| v.submit_transform(cx));
    cx.simulate_new_path_selection(|_| Some(output.clone()));
    cx.run_until_parked();
    let id = cx.update(|cx| view.read(cx).tasks.snapshot()[0].id);
    let done = cx.update(|cx| view.read(cx).tasks.wait_for(id, WAIT).unwrap());
    assert_eq!(done.state, TaskState::Done, "{:?}", done.error);
    assert_eq!(
        load_archive(&output, None).unwrap().0.entries().len(),
        load_archive(&source, None).unwrap().0.entries().len()
    );
}

#[gpui_kit::test]
fn repair_form_recovers_a_broken_archive_and_its_report_draws(cx: &mut TestAppContext) {
    let tmp = tempfile::tempdir().unwrap();
    let source = make_archive(tmp.path(), "broken.zip", None);
    let mut bytes = fs::read(&source).unwrap();
    let end = bytes.windows(4).position(|s| s == b"PK\x01\x02").unwrap();
    bytes.truncate(end);
    fs::write(&source, bytes).unwrap();
    let output = tmp.path().join("recovered.zip");
    let (handle, view) = open_window(cx, None);
    with_view(cx, handle, &view, |v, window, cx| {
        v.open_transform(true, window, cx);
        if let Some(Overlay::Transform(f)) = &mut v.overlay {
            f.source = Some(source.clone());
        }
        v.submit_transform(cx);
    });
    cx.simulate_new_path_selection(|_| Some(output.clone()));
    cx.run_until_parked();
    let id = cx.update(|cx| view.read(cx).tasks.snapshot()[0].id);
    let done = cx.update(|cx| view.read(cx).tasks.wait_for(id, WAIT).unwrap());
    assert_eq!(done.state, TaskState::Done, "{:?}", done.error);
    let report = done.summary.unwrap().recovery.unwrap();
    assert!(report.files > 0);
    assert!(!report.issues.is_empty());
    with_view(cx, handle, &view, |v, _, cx| {
        v.refresh_tasks(cx);
        v.overlay = Some(Overlay::RecoveryReport(id));
    });
    assert!(load_archive(&output, None).is_ok());
}

#[gpui_kit::test]
fn apk_case_conflicts_explain_the_volume_problem_before_writing(cx: &mut TestAppContext) {
    let tmp = tempfile::tempdir().unwrap();
    fs::write(tmp.path().join("case-probe"), b"probe").unwrap();
    if !tmp.path().join("CASE-PROBE").exists() {
        return;
    }
    let archive = tmp.path().join("resources.apk");
    let mut writer =
        rz_formats::create(&archive, OutputFormat::Zip, &CreateOptions::default()).unwrap();
    for name in ["res/9N.9.png", "res/9n.9.png"] {
        writer
            .add_file(
                &rz_core::NewEntry {
                    name: name.into(),
                    size: 3,
                    unix_mode: None,
                    modified: None,
                },
                &mut std::io::Cursor::new(b"png"),
            )
            .unwrap();
    }
    writer.finish().unwrap();
    let output = tmp.path().join("unpacked");
    let (handle, view) = open_window(cx, None);
    let id = cx.update(|cx| {
        view.read(cx).tasks.submit(JobSpec::Extract {
            archive,
            dest: output.clone(),
            opts: ExtractOptions::default(),
            smart: false,
            encoding: None,
        })
    });
    let done = cx.update(|cx| view.read(cx).tasks.wait_for(id, WAIT).unwrap());
    let error = done.error.unwrap();
    assert_eq!(error.code, ErrorCode::NameConflict);
    assert!(!output.exists());
    with_view(cx, handle, &view, |v, _, cx| v.refresh_tasks(cx));
    cx.update(|cx| {
        let Some(Notice::Error(message)) = &view.read(cx).notice else {
            panic!("expected error notice");
        };
        assert!(message.contains("不区分大小写"));
        assert!(message.contains("res/9N.9.png") && message.contains("res/9n.9.png"));
        assert!(!message.contains(tmp.path().to_string_lossy().as_ref()));
    });
    assert!(rustzip_gui::text::task_error("en", &error).contains("case-sensitive volume"));
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
            d.toggle(2);
        })
    });
    assert_eq!(marked(cx).len(), 2);
    // Toggling a picked row again takes it out.
    cx.update(|cx| table.update(cx, |t, _| t.delegate_mut().toggle(2)));
    assert_eq!(marked(cx), vec!["proj/docs".to_string()]);
    cx.update(|cx| table.update(cx, |t, _| t.delegate_mut().mark_range(1, 2)));
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
        v.change_settings(|s| s.open_folder_when_done = false, cx);
        v.change_settings(|s| s.task_notifications = true, cx);
    });
    cx.update(|cx| assert!(cx.theme().is_dark()));
    cx.run_until_parked();
    let saved = Settings::load(&path);
    assert_eq!(saved.theme, Pick::Dark);
    assert_eq!(saved.on_conflict, OnConflict::Skip);
    assert!(!saved.smart_extract);
    assert!(!saved.open_folder_when_done);
    assert!(saved.task_notifications);

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
fn interface_language_can_be_changed_and_is_saved(cx: &mut TestAppContext) {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("cfg/settings.json");
    let (handle, view) = open_window(cx, Some(path.clone()));
    with_view(cx, handle, &view, |v, _, cx| v.open_settings(cx));

    cx.update_window(handle.into(), |_, window, cx| {
        window.click("language-picker-settings", cx);
        window.within("popup-menu").click(2usize, cx)
    })
    .unwrap();
    cx.update(|cx| {
        let view = view.read(cx);
        assert_eq!(view.settings.language, "en-US");
        assert_eq!(view.tool_text("settings_title"), "Settings");
        assert_eq!(view.tool_text("sidebar_recent"), "Recent");
        assert_eq!(view.tool_text("toolbar_extract"), "Extract to…");
        assert_eq!(view.tool_text("recent_column_name"), "Name");
    });
    let table = cx.update(|cx| view.read(cx).table.clone());
    let name_column = cx.update(|cx| table.read(cx).delegate().column(0, cx).name);
    assert_eq!(name_column.as_ref(), "Name");

    cx.update_window(handle.into(), |_, window, cx| {
        window.click("language-picker-settings", cx);
        window.within("popup-menu").click(4usize, cx)
    })
    .unwrap();
    cx.run_until_parked();
    cx.update(|cx| {
        let view = view.read(cx);
        assert_eq!(view.settings.language, "fr-FR");
        assert_eq!(view.tool_text("settings_title"), "Paramètres");
        assert_eq!(view.tool_text("toolbar_extract"), "Extraire vers…");
    });

    cx.update_window(handle.into(), |_, window, cx| {
        window.click("language-picker-settings", cx);
        window.within("popup-menu").click(1usize, cx)
    })
    .unwrap();
    cx.run_until_parked();
    cx.update(|cx| {
        let view = view.read(cx);
        assert_eq!(view.settings.language, "zh-CN");
        assert_eq!(view.tool_text("settings_title"), "设置");
        assert_eq!(view.tool_text("toolbar_extract"), "解压到…");
        assert_eq!(view.tool_text("recent_column_name"), "名称");
    });
    let name_column = cx.update(|cx| table.read(cx).delegate().column(0, cx).name);
    assert_eq!(name_column.as_ref(), "名称");
    assert_eq!(Settings::load(&path).language, "zh-CN");
}

#[gpui_kit::test]
fn every_language_menu_choice_updates_the_visible_interface(cx: &mut TestAppContext) {
    let (handle, view) = open_window(cx, None);
    with_view(cx, handle, &view, |v, _, cx| v.open_settings(cx));
    let expected = [
        ("zh-CN", "设置"),
        ("en-US", "Settings"),
        ("es-ES", "Ajustes"),
        ("fr-FR", "Paramètres"),
        ("de-DE", "Einstellungen"),
        ("ja-JP", "設定"),
        ("ko-KR", "설정"),
        ("ar", "الإعدادات"),
        ("pt-BR", "Configurações"),
        ("ru-RU", "Настройки"),
    ];

    for (index, (language, title)) in expected.into_iter().enumerate() {
        cx.update_window(handle.into(), |_, window, cx| {
            window.click("language-picker-settings", cx);
            window.within("popup-menu").click(index + 1, cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update(|cx| {
            let view = view.read(cx);
            assert_eq!(view.settings.language, language);
            assert_eq!(view.tool_text("settings_title"), title);
        });
    }
}

#[gpui_kit::test]
fn localized_sidebar_labels_stay_inside_the_fixed_width_menu(cx: &mut TestAppContext) {
    let tmp = tempfile::tempdir().unwrap();
    let archive = make_archive(tmp.path(), "sidebar-labels.zip", None);
    let (handle, view) = open_window(cx, None);
    let visible_keys = [
        "sidebar_home",
        "sidebar_recent",
        "sidebar_favorites",
        "sidebar_convert",
        "sidebar_repair",
        "sidebar_passwords",
        "sidebar_history",
    ];
    for choice in rustzip_gui::text::LANGUAGE_CHOICES {
        with_view(cx, handle, &view, |v, _, cx| {
            v.change_settings(|settings| settings.language = choice.tag.into(), cx);
        });
        cx.update_window(handle.into(), |_, window, _| {
            let mut main = window.within("main");
            let sidebar = main.find("sidebar").bounds();
            for key in visible_keys {
                let label_text = rustzip_gui::text::tr(choice.tag, key);
                let label = main.within("sidebar").find(label_text).bounds();
                assert!(
                    sidebar.contains(&label.origin),
                    "{choice:?} {key}: {label:?}"
                );
                assert!(
                    sidebar.contains(&label.bottom_right()),
                    "{choice:?} {key}: {label:?}"
                );
            }
        })
        .unwrap();
    }

    open_sync(cx, handle, &view, &archive, None);
    for choice in rustzip_gui::text::LANGUAGE_CHOICES {
        with_view(cx, handle, &view, |v, _, cx| {
            v.change_settings(|settings| settings.language = choice.tag.into(), cx);
        });
        cx.update_window(handle.into(), |_, window, _| {
            let mut main = window.within("main");
            let sidebar = main.find("sidebar").bounds();
            let label_text = rustzip_gui::text::tr(choice.tag, "sidebar_current_archive");
            let label = main.within("sidebar").find(label_text).bounds();
            assert!(sidebar.contains(&label.origin), "{choice:?}: {label:?}");
            assert!(
                sidebar.contains(&label.bottom_right()),
                "{choice:?}: {label:?}"
            );
        })
        .unwrap();
    }
}

#[gpui_kit::test]
fn language_and_settings_controls_stay_in_the_top_row_after_opening_an_archive(
    cx: &mut TestAppContext,
) {
    let tmp = tempfile::tempdir().unwrap();
    let archive = make_archive(tmp.path(), "top-row.zip", None);
    let (handle, view) = open_window(cx, None);

    let empty_header = if rustzip_gui::view::TABS_IN_TITLEBAR {
        "empty-titlebar"
    } else {
        "empty"
    };
    cx.update_window(handle.into(), |_, window, _| {
        assert!(window.within(empty_header).find("open-settings").visible());
        assert!(
            window
                .within(empty_header)
                .find("language-picker-titlebar")
                .visible()
        );
    })
    .unwrap();

    open_sync(cx, handle, &view, &archive, None);
    cx.update_window(handle.into(), |_, window, _| {
        assert!(window.within("tabs").find("open-settings").visible());
        assert!(
            window
                .within("tabs")
                .find("language-picker-titlebar")
                .visible()
        );
        assert!(window.within("toolbar").try_find("open-settings").is_none());
    })
    .unwrap();
}

#[gpui_kit::test]
fn the_accent_colour_is_saved_and_repaints_the_theme(cx: &mut TestAppContext) {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("cfg/settings.json");
    let (handle, view) = open_window(cx, Some(path.clone()));
    let before = cx.update(|cx| cx.theme().primary);

    with_view(cx, handle, &view, |v, _, cx| v.open_settings(cx));
    cx.update_window(handle.into(), |_, window, cx| {
        window.click(("accent", 4usize), cx)
    })
    .unwrap();
    let after = cx.update(|cx| cx.theme().primary);
    assert_ne!(before, after);
    cx.run_until_parked();
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
    cx.run_until_parked();
    let saved = Settings::load(&path);
    assert!(saved.privacy_notice_seen);
    assert!(!saved.send_install_stats);

    // The next start does not ask again.
    let (handle2, view2) = open_window(cx, Some(path));
    with_view(cx, handle2, &view2, |v, _, cx| v.show_first_run_notice(cx));
    cx.update(|cx| assert!(view2.read(cx).overlay.is_none()));
}

#[gpui_kit::test]
fn the_privacy_notice_can_be_read_again_from_the_settings_and_answered_anew(
    cx: &mut TestAppContext,
) {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("settings.json");
    let (handle, view) = open_window(cx, Some(path.clone()));
    with_view(cx, handle, &view, |v, _, cx| v.answer_privacy(false, cx));
    cx.run_until_parked();
    assert!(!Settings::load(&path).send_install_stats);

    // From the settings: the notice opens, and going back changes nothing.
    with_view(cx, handle, &view, |v, _, cx| {
        v.open_settings(cx);
        v.show_privacy_notice(cx);
    });
    cx.update(|cx| assert!(matches!(view.read(cx).overlay, Some(Overlay::Privacy))));
    with_view(cx, handle, &view, |v, _, cx| v.leave_privacy_notice(cx));
    cx.update(|cx| assert!(matches!(view.read(cx).overlay, Some(Overlay::Settings))));
    assert!(!Settings::load(&path).send_install_stats);

    // Answering again changes the choice and returns to the settings.
    with_view(cx, handle, &view, |v, _, cx| v.show_privacy_notice(cx));
    with_view(cx, handle, &view, |v, _, cx| v.answer_privacy(true, cx));
    cx.run_until_parked();
    assert!(Settings::load(&path).send_install_stats);
    cx.update(|cx| assert!(matches!(view.read(cx).overlay, Some(Overlay::Settings))));

    // After the first-run notice, answering closes it, as before.
    with_view(cx, handle, &view, |v, _, cx| v.close_overlay(cx));
    with_view(cx, handle, &view, |v, _, cx| v.answer_privacy(false, cx));
    cx.update(|cx| assert!(view.read(cx).overlay.is_none()));
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
    let path = PathBuf::from("/big.zip");
    let (request_id, _) = with_view(cx, handle, &view, |v, _, _| v.begin_open_request(&path));
    let started = std::time::Instant::now();
    with_view(cx, handle, &view, |v, window, cx| {
        v.finish_open(request_id, path, None, Ok((tree, "zip".into())), window, cx);
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
