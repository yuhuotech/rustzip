use std::path::PathBuf;

fn main() {
    // Archives named on the command line (or "Open With" on some systems) open at once.
    let initial: Vec<PathBuf> = std::env::args_os()
        .skip(1)
        .map(PathBuf::from)
        .filter(|p| p.is_file())
        .collect();
    // Without the icon assets every icon in the kit's controls is silently blank.
    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(move |cx| {
            gpui_kit::init(cx);
            rustzip_gui::install_app_actions(cx);
            rustzip_gui::open_main_window(initial, cx);
            cx.activate(true);
        });
}
