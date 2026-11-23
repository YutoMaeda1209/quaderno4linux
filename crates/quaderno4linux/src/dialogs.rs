//! Small modal dialogs (text prompt, confirmation).

use std::rc::Rc;

use gpui::{div, prelude::*, Context, Entity, SharedString, Window};
use gpui_component::button::ButtonVariant;
use gpui_component::dialog::DialogButtonProps;
use gpui_component::input::{Input, InputState};
use gpui_component::notification::Notification;
use gpui_component::{v_flex, ActiveTheme, WindowExt};

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
            move |window: &mut Window, cx: &mut gpui::App| -> bool {
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
        let input_for_dialog = input.clone();
        window.open_dialog(cx, move |dialog, _, cx| {
            let submit = submit.clone();
            dialog
                .title(title.clone())
                .w(gpui::px(440.))
                .child(
                    v_flex()
                        .gap_2()
                        .child(div().text_sm().text_color(cx.theme().muted_foreground).child(description.clone()))
                        .child(Input::new(&input_for_dialog)),
                )
                .confirm()
                .button_props(DialogButtonProps::default().ok_text(ok_text.clone()).cancel_text("キャンセル"))
                .on_ok(move |_, window, cx| submit(window, cx))
        });
        input.update(cx, |state, cx| state.focus(window, cx));
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
        window.open_dialog(cx, move |dialog, _, _| {
            let view = view.clone();
            let on_ok = on_ok.clone();
            let props = DialogButtonProps::default().ok_text(ok_text.clone()).cancel_text("キャンセル");
            let props = if danger { props.ok_variant(ButtonVariant::Danger) } else { props };
            dialog
                .title(title.clone())
                .w(gpui::px(440.))
                .child(div().text_sm().child(message.clone()))
                .confirm()
                .button_props(props)
                .on_ok(move |_, window, cx| {
                    view.update(cx, |this, cx| on_ok(this, window, cx));
                    true
                })
        });
    }

    pub fn notify_error(&mut self, message: impl Into<SharedString>, window: &mut Window, cx: &mut Context<Self>) {
        let message = message.into();
        log::error!("{message}");
        self.push_log(format!("エラー: {message}"));
        window.push_notification(Notification::error(message).autohide(false), cx);
    }

    pub fn notify_warning(&mut self, message: impl Into<SharedString>, window: &mut Window, cx: &mut Context<Self>) {
        let message = message.into();
        self.push_log(format!("警告: {message}"));
        window.push_notification(Notification::warning(message), cx);
    }

    pub fn notify_success(&mut self, message: impl Into<SharedString>, window: &mut Window, cx: &mut Context<Self>) {
        let message = message.into();
        self.push_log(message.to_string());
        window.push_notification(Notification::success(message), cx);
    }
}
