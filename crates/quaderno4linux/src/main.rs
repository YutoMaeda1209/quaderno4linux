//! Quaderno4Linux – a desktop manager for the Fujitsu Quaderno.

mod app;
mod dialogs;
mod explorer;
mod tasks;

use gpui::{px, size, App, AppContext, Application, Bounds, KeyBinding, WindowBounds, WindowOptions};
use gpui_component::{Root, TitleBar};

use crate::app::QuadernoApp;

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    // `quaderno4linux --connect <address>` connects right away (handy with quaderno-mock).
    let args: Vec<String> = std::env::args().collect();
    let initial_address = args.iter().position(|a| a == "--connect").and_then(|i| args.get(i + 1).cloned());

    Application::new().with_assets(gpui_component_assets::Assets).run(move |cx: &mut App| {
        gpui_component::init(cx);
        bind_keys(cx);
        cx.on_window_closed(|cx| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();

        let bounds = Bounds::centered(None, size(px(1100.), px(720.)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitleBar::title_bar_options()),
            window_min_size: Some(size(px(720.), px(480.))),
            app_id: Some("io.github.yutomaeda1209.quaderno4linux".into()),
            ..Default::default()
        };
        cx.open_window(options, |window, cx| {
            window.set_window_title("Quaderno4Linux");
            let view = cx.new(|cx| QuadernoApp::new(initial_address.clone(), window, cx));
            cx.new(|cx| Root::new(view, window, cx))
        })
        .expect("failed to open the main window");
        cx.activate(true);
    });
}

fn bind_keys(cx: &mut App) {
    use crate::app::actions::*;
    let ctx = Some(app::CONTEXT);
    cx.bind_keys([
        KeyBinding::new("f5", Refresh, ctx),
        KeyBinding::new("ctrl-r", Refresh, ctx),
        KeyBinding::new("alt-up", GoUp, ctx),
        KeyBinding::new("backspace", GoUp, ctx),
        KeyBinding::new("enter", OpenSelected, ctx),
        KeyBinding::new("delete", DeleteSelected, ctx),
        KeyBinding::new("f2", RenameSelected, ctx),
        KeyBinding::new("ctrl-c", CopySelected, ctx),
        KeyBinding::new("ctrl-x", CutSelected, ctx),
        KeyBinding::new("ctrl-v", Paste, ctx),
        KeyBinding::new("ctrl-a", SelectAll, ctx),
        KeyBinding::new("ctrl-shift-n", NewFolder, ctx),
        KeyBinding::new("ctrl-u", Upload, ctx),
        KeyBinding::new("ctrl-d", Download, ctx),
        KeyBinding::new("up", SelectPrev, ctx),
        KeyBinding::new("down", SelectNext, ctx),
        KeyBinding::new("escape", ClearSelection, ctx),
        KeyBinding::new("ctrl-q", Quit, None),
    ]);
    cx.on_action(|_: &Quit, cx| cx.quit());
}
