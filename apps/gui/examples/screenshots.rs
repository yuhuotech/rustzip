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
    AppContext, Bounds, Entity, HeadlessAppContext, WindowHandle, base::Root, px, size,
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
    // One real picture, so the details panel has something to show.
    image::RgbaImage::from_fn(320, 200, |x, y| {
        image::Rgba([(x * 255 / 320) as u8, (y * 255 / 200) as u8, 170, 255])
    })
    .save(src.join("设计稿/主窗口.png"))
    .unwrap();
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
                // RZ_SHOT_ACCENT=graphite (or #rrggbb) previews another accent.
                if let Ok(accent) = std::env::var("RZ_SHOT_ACCENT") {
                    rustzip_gui::brand::set_accent(&accent, cx);
                }
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
    // Images decode in the background, so draw once, let that finish, and draw again.
    for _ in 0..2 {
        cx.update_window(handle.into(), |_, window, cx| {
            window.refresh();
            window.render_frame(cx);
        })
        .unwrap();
        cx.run_until_parked();
    }
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
            Arc::new(rustzip_gui::assets::AppAssets),
            gpui_kit::platform::current_headless_renderer,
        );
        cx.update(gpui_kit::init);

        // 1. Nothing open yet.
        // One archive that exists and two that were moved away or never existed.
        let recent = vec![
            archive.clone(),
            work.path().join("旧项目备份.7z"),
            work.path().join("photos.zip"),
        ];
        let (handle, view) = window(&mut cx, mode, recent);
        shoot(&mut cx, handle, &out_dir, &format!("{tag}-1-empty"));
        for (page, suffix) in [
            (rustzip_gui::view::Page::Recent, "1b-startup-recent"),
            (rustzip_gui::view::Page::Favorites, "1c-startup-favorites"),
            (rustzip_gui::view::Page::Passwords, "1d-startup-passwords"),
        ] {
            act(&mut cx, handle, &view, |v, _, cx| v.show_page(page, cx));
            shoot(&mut cx, handle, &out_dir, &format!("{tag}-{suffix}"));
            if page == rustzip_gui::view::Page::Favorites {
                act(&mut cx, handle, &view, |v, _, cx| {
                    v.set_folder_view(rz_app::settings::FolderView::Icons, cx)
                });
                shoot(
                    &mut cx,
                    handle,
                    &out_dir,
                    &format!("{tag}-1g-location-icons"),
                );
                act(&mut cx, handle, &view, |v, _, cx| {
                    v.show_folder(work.path().to_path_buf(), cx)
                });
                shoot(&mut cx, handle, &out_dir, &format!("{tag}-1h-folder-icons"));
                act(&mut cx, handle, &view, |v, _, cx| {
                    v.set_folder_view(rz_app::settings::FolderView::List, cx)
                });
            }
        }
        act(&mut cx, handle, &view, |v, w, cx| {
            v.show_page(rustzip_gui::view::Page::Archive, cx);
            v.open_transform(false, w, cx);
        });
        shoot(
            &mut cx,
            handle,
            &out_dir,
            &format!("{tag}-1e-startup-convert"),
        );
        act(&mut cx, handle, &view, |v, w, cx| {
            v.close_overlay(cx);
            v.open_transform(true, w, cx);
        });
        shoot(
            &mut cx,
            handle,
            &out_dir,
            &format!("{tag}-1f-startup-repair"),
        );
        act(&mut cx, handle, &view, |v, _, cx| v.close_overlay(cx));

        // 2. An archive open at its top level.
        let loaded = load_archive(&archive, None);
        let path = archive.clone();
        act(&mut cx, handle, &view, move |v, w, cx| {
            let (request_id, _) = v.begin_open_request(&path);
            v.finish_open(request_id, path, None, loaded, w, cx);
        });
        shoot(&mut cx, handle, &out_dir, &format!("{tag}-2-browse"));

        // 3. Inside a folder with a row selected.
        act(&mut cx, handle, &view, |v, _, cx| {
            v.navigate("项目文档", cx);
        });
        let table = view.read_with(&cx, |v, _| v.table.clone());
        cx.update_window(handle.into(), |_, _, cx| {
            table.update(cx, |t, cx| t.set_selected_row(3, cx));
        })
        .unwrap();
        shoot(&mut cx, handle, &out_dir, &format!("{tag}-3-selected"));

        // 3b. A picture selected: the panel shows it, and the archive has been tested.
        act(&mut cx, handle, &view, |v, _, cx| {
            v.navigate("项目文档/设计稿", cx);
            if let Some(a) = v.archives.first_mut() {
                a.verified = Some(true);
            }
        });
        cx.update_window(handle.into(), |_, _, cx| {
            table.update(cx, |t, cx| t.set_selected_row(1, cx));
        })
        .unwrap();
        shoot(&mut cx, handle, &out_dir, &format!("{tag}-3b-picture"));
        act(&mut cx, handle, &view, |v, _, cx| {
            v.navigate("项目文档", cx)
        });

        // 3c. The recent archives page.
        act(&mut cx, handle, &view, |v, _, cx| {
            v.show_page(rustzip_gui::view::Page::Recent, cx);
        });
        // The scan of the files runs in the background; give it a moment.
        std::thread::sleep(std::time::Duration::from_millis(200));
        shoot(&mut cx, handle, &out_dir, &format!("{tag}-3c-recent"));
        // With one archive picked, the buttons are the ones for that archive.
        let pick = archive.clone();
        act(&mut cx, handle, &view, move |v, _, cx| {
            v.select_recent(pick, cx);
        });
        shoot(
            &mut cx,
            handle,
            &out_dir,
            &format!("{tag}-3e-recent-picked"),
        );
        // The two dialogs that belong to a picked archive file.
        act(&mut cx, handle, &view, |v, w, cx| v.ask_rename_file(w, cx));
        shoot(&mut cx, handle, &out_dir, &format!("{tag}-3g-rename"));
        act(&mut cx, handle, &view, |v, _, cx| {
            v.close_overlay(cx);
            v.ask_trash(cx);
        });
        shoot(&mut cx, handle, &out_dir, &format!("{tag}-3h-trash"));
        act(&mut cx, handle, &view, |v, _, cx| v.close_overlay(cx));
        act(&mut cx, handle, &view, |v, _, cx| {
            v.show_page(rustzip_gui::view::Page::Archive, cx);
        });

        // 3d. Asking before deleting from the archive.
        cx.update_window(handle.into(), |_, _, cx| {
            table.update(cx, |t, cx| t.set_selected_row(4, cx));
        })
        .unwrap();
        act(&mut cx, handle, &view, |v, _, cx| v.ask_delete(cx));
        shoot(&mut cx, handle, &out_dir, &format!("{tag}-3d-delete"));
        act(&mut cx, handle, &view, |v, _, cx| v.close_overlay(cx));

        // 3f. Out of the archive, into the folder it lies in.
        act(&mut cx, handle, &view, |v, _, cx| {
            // Once up to the top of the archive, and once more to leave it.
            v.go_up_or_out(cx);
            v.go_up_or_out(cx);
        });
        std::thread::sleep(std::time::Duration::from_millis(200));
        shoot(&mut cx, handle, &out_dir, &format!("{tag}-3f-folder"));
        act(&mut cx, handle, &view, |v, _, cx| {
            v.show_page(rustzip_gui::view::Page::Archive, cx);
        });

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
        // The same notice opened again from the settings.
        act(&mut cx, handle, &view, |v, _, cx| {
            v.close_overlay(cx);
            v.open_settings(cx);
            v.show_privacy_notice(cx);
        });
        shoot(
            &mut cx,
            handle,
            &out_dir,
            &format!("{tag}-8b-privacy-again"),
        );
        act(&mut cx, handle, &view, |v, _, cx| {
            v.close_overlay(cx);
            v.settings.favorites = vec![
                work.path().to_path_buf(),
                PathBuf::from("/Users/example/Documents"),
            ];
            v.show_page(rustzip_gui::view::Page::Favorites, cx);
        });
        shoot(&mut cx, handle, &out_dir, &format!("{tag}-9-favorites"));
        act(&mut cx, handle, &view, |v, _, cx| {
            v.show_page(rustzip_gui::view::Page::Passwords, cx);
            v.vault.set("项目备份.zip", "example").unwrap();
            v.vault.set("共享资料.7z", "example").unwrap();
            v.tools.labels = vec!["项目备份.zip".into(), "共享资料.7z".into()];
        });
        shoot(
            &mut cx,
            handle,
            &out_dir,
            &format!("{tag}-10-password-book"),
        );
        act(&mut cx, handle, &view, |v, w, cx| {
            v.edit_password(None, w, cx)
        });
        shoot(
            &mut cx,
            handle,
            &out_dir,
            &format!("{tag}-11-password-edit"),
        );
        act(&mut cx, handle, &view, |v, w, cx| {
            v.close_overlay(cx);
            v.open_transform(false, w, cx);
        });
        shoot(&mut cx, handle, &out_dir, &format!("{tag}-12-convert"));
        act(&mut cx, handle, &view, |v, w, cx| {
            v.close_overlay(cx);
            v.open_transform(true, w, cx);
        });
        shoot(&mut cx, handle, &out_dir, &format!("{tag}-13-repair"));
        let damaged = work.path().join(format!("{tag}-damaged.zip"));
        let mut bytes = fs::read(&archive).unwrap();
        let end = bytes.windows(4).position(|b| b == b"PK\x01\x02").unwrap();
        bytes.truncate(end);
        fs::write(&damaged, bytes).unwrap();
        act(&mut cx, handle, &view, |v, _, cx| {
            v.close_overlay(cx);
            let id = v.tasks.submit(JobSpec::Transform {
                archive: damaged,
                output: work.path().join(format!("{tag}-recovered.zip")),
                opts: rz_app::TransformOptions {
                    format: OutputFormat::Zip,
                    create: CreateOptions::default(),
                    limits: ExtractOptions::default(),
                    encoding: None,
                },
                recover: true,
            });
            let info = v
                .tasks
                .wait_for(id, std::time::Duration::from_secs(30))
                .unwrap();
            assert_eq!(info.state, rz_app::TaskState::Done);
            v.refresh_tasks(cx);
            v.overlay = Some(rustzip_gui::view::Overlay::RecoveryReport(id));
        });
        shoot(
            &mut cx,
            handle,
            &out_dir,
            &format!("{tag}-14-recovery-report"),
        );
    }
}
