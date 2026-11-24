//! Small modal dialogs (text prompt, confirmation) and notifications.

use std::rc::Rc;
use std::time::Duration;

use gpui::{div, prelude::*, px, Context, Entity, SharedString, Window};
use gpui_component::button::{Button, ButtonVariant, ButtonVariants};
use gpui_component::dialog::DialogButtonProps;
use gpui_component::input::{Input, InputState};
use gpui_component::{h_flex, v_flex, ActiveTheme, Icon, IconName, Sizable, WindowExt};

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
        self.push_toast(ToastKind::Error, summary, window, cx);
    }

    pub fn notify_warning(&mut self, message: impl Into<SharedString>, window: &mut Window, cx: &mut Context<Self>) {
        let message = message.into();
        log::warn!("{message}");
        self.record_log(format!("警告: {message}"));
        self.push_toast(ToastKind::Warning, message, window, cx);
    }

    pub fn notify_success(&mut self, message: impl Into<SharedString>, window: &mut Window, cx: &mut Context<Self>) {
        let message = message.into();
        self.push_log(message.to_string());
        self.push_toast(ToastKind::Success, message, window, cx);
    }

    fn push_toast(&mut self, kind: ToastKind, message: SharedString, window: &mut Window, cx: &mut Context<Self>) {
        let id = self.next_toast_id;
        self.next_toast_id += 1;
        self.toasts.push(Toast { id, kind, message });
        if self.toasts.len() > MAX_TOASTS {
            self.toasts.remove(0);
        }
        cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(TOAST_DURATION).await;
            let _ = this.update(cx, |this, cx| this.dismiss_toast(id, cx));
        })
        .detach();
        cx.notify();
    }

    fn dismiss_toast(&mut self, id: u64, cx: &mut Context<Self>) {
        self.toasts.retain(|t| t.id != id);
        cx.notify();
    }

    /// Notifications stacked at the bottom right; newer ones push older ones up.
    pub fn render_toasts(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let toasts = self.toasts.iter().map(|toast| {
            let id = toast.id;
            let icon = match toast.kind {
                ToastKind::Success => Icon::new(IconName::CircleCheck).text_color(theme.success),
                ToastKind::Warning => Icon::new(IconName::TriangleAlert).text_color(theme.warning),
                ToastKind::Error => Icon::new(IconName::CircleX).text_color(theme.danger),
            };
            h_flex()
                .id(("toast", id))
                .occlude()
                .w(px(380.))
                .px_4()
                .py_3()
                .gap_3()
                .border_1()
                .border_color(theme.border)
                .bg(theme.popover)
                .rounded(theme.radius_lg)
                .shadow_md()
                .child(icon)
                .child(div().flex_1().min_w_0().text_sm().child(toast.message.clone()))
                .when(toast.kind == ToastKind::Error, |el| {
                    el.child(Button::new(("toast-log", id)).label("ログを開く").small().outline().on_click(
                        cx.listener(|_, _, _, cx| match crate::log_file_path() {
                            Some(path) => cx.open_with_system(&path),
                            None => log::error!("ログファイルの場所を決められません"),
                        }),
                    ))
                })
                .child(
                    Button::new(("toast-close", id))
                        .icon(IconName::Close)
                        .ghost()
                        .xsmall()
                        .on_click(cx.listener(move |this, _, _, cx| this.dismiss_toast(id, cx))),
                )
        });
        div().absolute().bottom(px(42.)).right_4().child(v_flex().gap_2().children(toasts))
    }
}

const TOAST_DURATION: Duration = Duration::from_secs(10);
const MAX_TOASTS: usize = 5;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ToastKind {
    Success,
    Warning,
    Error,
}

pub struct Toast {
    id: u64,
    kind: ToastKind,
    message: SharedString,
}
