//! Quaderno4Linux: 富士通 Quaderno のデスクトップ管理アプリ。

mod app;
mod dialogs;
mod explorer;
mod settings;
mod tasks;

use std::fs::File;
use std::io::Write;
use std::path::PathBuf;

use gpui_kit::component::{Theme, TitleBar};
use gpui_kit::{Anchor, App, AppContext, Bounds, KeyBinding, WindowBounds, WindowOptions, px, size};

use crate::app::QuadernoApp;

fn main() {
    init_logging();

    // `quaderno4linux --connect <address>` で起動するとすぐに接続する (quaderno-mock と組み合わせる
    // と便利)。
    let args: Vec<String> = std::env::args().collect();
    let initial_address = args.iter().position(|a| a == "--connect").and_then(|i| args.get(i + 1).cloned());

    gpui_kit::application().with_assets(gpui_kit::assets::Assets).run(move |cx: &mut App| {
        gpui_kit::init(cx);
        Theme::update(cx, |theme| {
            theme.notification.placement = Anchor::BottomRight;
            // 通知をステータスバーより上に表示する。
            theme.notification.margins.bottom = px(42.);
        });
        bind_keys(cx);
        cx.on_window_closed(|cx, _| {
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
        gpui_kit::open_window(options, cx, |window, cx| {
            window.set_window_title("Quaderno4Linux");
            cx.new(|cx| QuadernoApp::new(initial_address.clone(), window, cx))
        })
        .expect("failed to open the main window");
        cx.activate(true);
    });
}

/// `~/.local/state/quaderno4linux/quaderno4linux.log`。起動のたびに上書きする。
pub fn log_file_path() -> Option<PathBuf> {
    Some(dirs::state_dir()?.join("quaderno4linux").join("quaderno4linux.log"))
}

/// 標準エラー出力と、作成できればログファイルにログを出力する。
fn init_logging() {
    let file = log_file_path().and_then(|path| {
        std::fs::create_dir_all(path.parent()?).ok()?;
        File::create(path).ok()
    });
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .target(env_logger::Target::Pipe(Box::new(Tee(file))))
        .init();
}

struct Tee(Option<File>);

impl Write for Tee {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if let Some(f) = &mut self.0 {
            let _ = f.write_all(buf);
        }
        std::io::stderr().write_all(buf)?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        if let Some(f) = &mut self.0 {
            let _ = f.flush();
        }
        std::io::stderr().flush()
    }
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
