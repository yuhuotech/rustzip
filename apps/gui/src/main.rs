// A release build on Windows is a window program: no console flashing up behind it.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

fn main() {
    // Archives named on the command line, or the file manager's Extract here / Compress.
    let launch = rustzip_gui::launch::parse_args(std::env::args_os().skip(1));
    // Without the icon assets every icon in the kit's controls is silently blank.
    let app = gpui_kit::application().with_assets(gpui_kit::assets::Assets);
    let opened_by_system = rustzip_gui::listen_for_open_urls(&app);
    app.run(move |cx| {
        gpui_kit::init(cx);
        rustzip_gui::install_app_actions(cx);
        rustzip_gui::forward_open_urls(opened_by_system, cx);
        rustzip_gui::open_main_window(launch, cx);
        cx.activate(true);
    });
}
