//! 端末の設定ダイアログ。「一般」と「Wi-Fi」のタブに分け、それぞれ [`general`] と [`wifi`] で組み立てる。

mod general;
mod wifi;

use gpui_kit::component::dialog::DialogButtonProps;
use gpui_kit::component::searchable_list::{SearchableListItem, SearchableVec};
use gpui_kit::component::select::SelectState;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::tab::TabBar;
use gpui_kit::component::{ActiveTheme, IndexPath, WindowExt, h_flex, v_flex};
use gpui_kit::{AnyElement, App, Context, Entity, SharedString, Window, div, prelude::*, px};

use self::general::GeneralForm;
use self::wifi::WifiPanel;
use crate::app::QuadernoApp;

/// 選択肢。表示名と、端末に送る値を分けて持つ。
#[derive(Clone)]
struct Choice {
    value: String,
    label: SharedString,
}

impl SearchableListItem for Choice {
    type Value = String;

    fn title(&self) -> SharedString {
        self.label.clone()
    }

    fn value(&self) -> &String {
        &self.value
    }

    // タイムゾーンは表示名 (`UTC+9`) でも ID (`Asia/Tokyo`) でも探せるようにする。
    fn matches(&self, query: &str) -> bool {
        let query = query.to_lowercase();
        self.label.to_lowercase().contains(&query) || self.value.to_lowercase().contains(&query)
    }
}

type ChoiceSelect = Entity<SelectState<SearchableVec<Choice>>>;

/// バックグラウンドで読み込む値。
#[derive(Clone)]
enum Loading<T> {
    Pending,
    Failed,
    Ready(T),
}

fn set_loaded<T: 'static>(entity: &Entity<Loading<T>>, value: Loading<T>, cx: &mut App) {
    entity.update(cx, |v, cx| {
        *v = value;
        cx.notify();
    });
}

/// 読み込み中の表示。
fn loading_indicator(message: &'static str, cx: &App) -> AnyElement {
    h_flex()
        .gap_2()
        .py_6()
        .justify_center()
        .child(Spinner::new())
        .child(div().text_sm().text_color(cx.theme().muted_foreground).child(message))
        .into_any_element()
}

fn load_failed(message: &'static str, cx: &App) -> AnyElement {
    div().py_6().text_sm().text_color(cx.theme().muted_foreground).child(message).into_any_element()
}

impl QuadernoApp {
    /// 設定ダイアログを開き、接続中の端末の設定を読み込む。読み込むまではスピナーを表示する。
    pub fn open_device_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(client) = self.client() else { return };
        let general: Entity<Loading<GeneralForm>> = cx.new(|_| Loading::Pending);
        // Wi-Fi のタブの表示は、操作のたびに読み込み直した状態で更新する。
        let wifi: WifiPanel = cx.new(|_| Loading::Pending);
        let started = self.run_task(
            "端末の設定を読み込んでいます",
            window,
            cx,
            move |_| Ok((client.settings()?, client.wifi_status())),
            {
                let (general, wifi) = (general.clone(), wifi.clone());
                move |this, result, window, cx| {
                    let (form, status) = match result {
                        Ok((settings, status)) => {
                            let status = match status {
                                Ok(status) => Loading::Ready(status),
                                Err(e) => {
                                    this.notify_error("Wi-Fi の設定を読み込めませんでした", &e, window, cx);
                                    Loading::Failed
                                }
                            };
                            (Loading::Ready(GeneralForm::new(settings, window, cx)), status)
                        }
                        Err(e) => {
                            this.notify_error("端末の設定を読み込めませんでした", &e, window, cx);
                            (Loading::Failed, Loading::Failed)
                        }
                    };
                    set_loaded(&general, form, cx);
                    set_loaded(&wifi, status, cx);
                }
            },
        );
        if started {
            self.show_device_settings(general, wifi, window, cx);
        }
    }

    fn show_device_settings(
        &mut self,
        general: Entity<Loading<GeneralForm>>,
        wifi: WifiPanel,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let tab = cx.new(|_| 0usize);
        let view = cx.entity();
        window.open_alert_dialog(cx, move |dialog, _, cx| {
            let view = view.clone();
            let selected_tab = *tab.read(cx);
            let tabs = {
                let tab = tab.clone();
                TabBar::new("settings-tabs")
                    .underline()
                    .selected_index(selected_tab)
                    .on_click(move |ix, _, cx| {
                        tab.update(cx, |t, cx| {
                            *t = *ix;
                            cx.notify();
                        })
                    })
                    .child("一般")
                    .child("Wi-Fi")
            };
            let content = match (selected_tab, general.read(cx)) {
                (0, Loading::Pending) => loading_indicator("端末の設定を読み込んでいます", cx),
                (0, Loading::Failed) => load_failed("端末の設定を読み込めませんでした", cx),
                (0, Loading::Ready(form)) => form.render(&view),
                _ => wifi::wifi_tab(&view, &wifi, cx),
            };
            let general = general.clone();
            dialog
                .title("端末の設定")
                .width(px(520.))
                .child(v_flex().gap_4().child(tabs).child(content))
                .confirm()
                .button_props(DialogButtonProps::default().ok_text("保存").cancel_text("キャンセル"))
                .on_ok(move |_, window, cx| {
                    // 読み込みが終わっていなければ、保存するものはない。
                    let Loading::Ready(form) = general.read(cx) else { return true };
                    let (current, new) = (form.current.clone(), form.values(cx));
                    // 別の処理を実行中で保存できなければ、変更を残すためにダイアログを閉じない。
                    view.update(cx, |this, cx| this.save_device_settings(current, new, window, cx))
                })
        });
    }
}

fn choice(value: &str, label: impl Into<SharedString>) -> Choice {
    Choice { value: value.to_string(), label: label.into() }
}

/// `current` を選んだ状態の選択欄。端末の値が選択肢になければ、その値も選択肢に加える。
fn choice_select(
    mut choices: Vec<Choice>,
    current: &str,
    searchable: bool,
    window: &mut Window,
    cx: &mut App,
) -> ChoiceSelect {
    if !current.is_empty() && !choices.iter().any(|c| c.value == current) {
        choices.push(choice(current, current.to_string()));
    }
    let selected = choices.iter().position(|c| c.value == current).map(IndexPath::new);
    cx.new(|cx| SelectState::new(SearchableVec::new(choices), selected, window, cx).searchable(searchable))
}
