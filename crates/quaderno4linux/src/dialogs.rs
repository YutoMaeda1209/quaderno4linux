//! Small modal dialogs (text prompt, confirmation) and notifications.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::component::button::{Button, ButtonVariant};
use gpui_kit::component::dialog::DialogButtonProps;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::notification::{Notification, NotificationType};
use gpui_kit::component::WindowExt;
use gpui_kit::{prelude::*, px, Context, Entity, Focusable, SharedString, Window};

use crate::app::QuadernoApp;

type Submit = Rc<dyn Fn(&mut QuadernoApp, String, &mut Window, &mut Context<QuadernoApp>)>;

impl QuadernoApp {
    /// Shows a dialog with a single text field. `on_submit` runs with the trimmed
    /// value when the user confirms (OK button or Enter – the dialog maps Enter to
    /// its OK action) and the value is not empty.
    #[allow(clippy::too_many_arguments)]
    pub fn prompt_text(
        &mut self,
        title: impl Into<SharedString>,
        description: impl Into<SharedString>,
        initial: impl Into<SharedString>,
        ok_text: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
        on_submit: impl Fn(&mut QuadernoApp, String, &mut Window, &mut Context<QuadernoApp>) + 'static,
    ) {
        let initial: SharedString = initial.into();
        let input: Entity<InputState> = cx.new(|cx| InputState::new(window, cx).default_value(initial.clone()));
        let on_submit: Submit = Rc::new(on_submit);
        let view = cx.entity();

        let submit = {
            let input = input.clone();
            let view = view.clone();
            let on_submit = on_submit.clone();
            move |window: &mut Window, cx: &mut gpui_kit::App| -> bool {
                let value = input.read(cx).value().trim().to_string();
                if value.is_empty() {
                    return false;
                }
                view.update(cx, |this, cx| on_submit(this, value, window, cx));
                true
            }
        };

        let title: SharedString = title.into();
        let description: SharedString = description.into();
        let ok_text: SharedString = ok_text.into();
        // The builder can run again when the dialog layer is rebuilt; focus the input only once.
        let initial_focus = Cell::new(true);
        window.open_alert_dialog(cx, move |dialog, window, cx| {
            if initial_focus.replace(false) {
                input.focus_handle(cx).focus(window, cx);
            }
            let submit = submit.clone();
            dialog
                .title(title.clone())
                .description(description.clone())
                .width(px(440.))
                .child(Input::new(&input))
                .confirm()
                .button_props(DialogButtonProps::default().ok_text(ok_text.clone()).cancel_text("キャンセル"))
                .on_ok(move |_, window, cx| submit(window, cx))
        });
    }

    /// Shows a confirmation dialog.
    #[allow(clippy::too_many_arguments)]
    pub fn confirm(
        &mut self,
        title: impl Into<SharedString>,
        message: impl Into<SharedString>,
        ok_text: impl Into<SharedString>,
        danger: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
        on_ok: impl Fn(&mut QuadernoApp, &mut Window, &mut Context<QuadernoApp>) + 'static,
    ) {
        let view = cx.entity();
        let on_ok = Rc::new(on_ok);
        let title: SharedString = title.into();
        let message: SharedString = message.into();
        let ok_text: SharedString = ok_text.into();
        window.open_alert_dialog(cx, move |dialog, _, _| {
            let view = view.clone();
            let on_ok = on_ok.clone();
            let props = DialogButtonProps::default().ok_text(ok_text.clone()).cancel_text("キャンセル");
            let props = if danger { props.ok_variant(ButtonVariant::Danger) } else { props };
            dialog
                .title(title.clone())
                .description(message.clone())
                .width(px(440.))
                .confirm()
                .button_props(props)
                .on_ok(move |_, window, cx| {
                    view.update(cx, |this, cx| on_ok(this, window, cx));
                    true
                })
        });
    }

    /// Shows an error notification. `detail` goes only to the log file, which the
    /// notification offers to open.
    pub fn notify_error(
        &mut self,
        summary: impl Into<SharedString>,
        detail: impl std::fmt::Display,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let summary = summary.into();
        log::error!("{summary}: {detail:#}");
        self.record_log(format!("エラー: {summary}"));
        self.push_toast(NotificationType::Error, summary, window, cx);
    }

    pub fn notify_warning(&mut self, message: impl Into<SharedString>, window: &mut Window, cx: &mut Context<Self>) {
        let message = message.into();
        log::warn!("{message}");
        self.record_log(format!("警告: {message}"));
        self.push_toast(NotificationType::Warning, message, window, cx);
    }

    pub fn notify_success(&mut self, message: impl Into<SharedString>, window: &mut Window, cx: &mut Context<Self>) {
        let message = message.into();
        self.push_log(message.to_string());
        self.push_toast(NotificationType::Success, message, window, cx);
    }

    fn push_toast(
        &mut self,
        kind: NotificationType,
        message: SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let id = self.next_toast_id;
        self.next_toast_id += 1;
        // Autohide is fixed at 5 seconds, so hide it ourselves after `TOAST_DURATION`.
        let note = Notification::new().id1::<ToastKey>(id).with_type(kind).message(message).autohide(false);
        let note = if matches!(kind, NotificationType::Error) {
            note.action(|_, _, _| {
                Button::new("open-log").label("ログを開く").outline().on_click(|_, _, cx| {
                    match crate::log_file_path() {
                        Some(path) => cx.open_with_system(&path),
                        None => log::error!("ログファイルの場所を決められません"),
                    }
                })
            })
        } else {
            note
        };
        window.push_notification(note, cx);
        cx.spawn_in(window, async move |_, cx| {
            cx.background_executor().timer(TOAST_DURATION).await;
            let _ = cx.update(|window, cx| window.remove_notification1::<ToastKey>(id, cx));
        })
        .detach();
    }
}

const TOAST_DURATION: Duration = Duration::from_secs(10);

/// Id type that keys this app's notifications so each can be removed on its own.
struct ToastKey;
