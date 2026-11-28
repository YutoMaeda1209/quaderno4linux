//! 端末の設定ダイアログ。

use std::time::SystemTime;

use gpui_kit::component::button::Button;
use gpui_kit::component::dialog::DialogButtonProps;
use gpui_kit::component::form::{field, v_form};
use gpui_kit::component::searchable_list::{SearchableListItem, SearchableVec};
use gpui_kit::component::select::{Select, SelectState};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::tab::TabBar;
use gpui_kit::component::{ActiveTheme, IndexPath, Sizable, WindowExt, h_flex, v_flex};
use gpui_kit::{AnyElement, App, Context, Entity, SharedString, Window, div, prelude::*, px};
use quaderno_core::DeviceSettings;

use crate::app::QuadernoApp;
use crate::wifi::WifiPanel;

/// 選択肢。表示名と、端末に送る値を分けて持つ。
#[derive(Clone)]
pub(crate) struct Choice {
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

pub(crate) type ChoiceSelect = Entity<SelectState<SearchableVec<Choice>>>;

/// 公式アプリ (Digital Paper App) が選択肢にしている値。
const STANDBY_MINUTES: [&str; 4] = ["5", "10", "30", "60"];
const DATE_FORMATS: [&str; 2] = ["yyyy/M/d", "M/d/yyyy"];
const TIMEZONES: [&str; 24] = [
    "UTC",
    "Asia/Tokyo",
    "Pacific/Honolulu",
    "America/Anchorage",
    "America/Los_Angeles",
    "America/Denver",
    "America/Chicago",
    "America/New_York",
    "America/Halifax",
    "Europe/Amsterdam",
    "Europe/Helsinki",
    "Etc/GMT+12",
    "Etc/GMT+11",
    "Etc/GMT+3",
    "Etc/GMT+2",
    "Etc/GMT+1",
    "Etc/GMT-3",
    "Etc/GMT-4",
    "Etc/GMT-5",
    "Etc/GMT-6",
    "Etc/GMT-7",
    "Etc/GMT-8",
    "Etc/GMT-10",
    "Etc/GMT-11",
];

/// バックグラウンドで読み込む値。
#[derive(Clone)]
pub(crate) enum Loading<T> {
    Pending,
    Failed,
    Ready(T),
}

pub(crate) fn set_loaded<T: 'static>(entity: &Entity<Loading<T>>, value: Loading<T>, cx: &mut App) {
    entity.update(cx, |v, cx| {
        *v = value;
        cx.notify();
    });
}

/// 読み込み中の表示。
pub(crate) fn loading_indicator(message: &'static str, cx: &App) -> AnyElement {
    h_flex()
        .gap_2()
        .py_6()
        .justify_center()
        .child(Spinner::new())
        .child(div().text_sm().text_color(cx.theme().muted_foreground).child(message))
        .into_any_element()
}

pub(crate) fn load_failed(message: &'static str, cx: &App) -> AnyElement {
    div().py_6().text_sm().text_color(cx.theme().muted_foreground).child(message).into_any_element()
}

/// 「一般」のタブの入力欄。
#[derive(Clone)]
struct GeneralForm {
    current: DeviceSettings,
    standby: ChoiceSelect,
    date_format: ChoiceSelect,
    time_format: ChoiceSelect,
    timezone: ChoiceSelect,
}

impl GeneralForm {
    fn new(current: DeviceSettings, window: &mut Window, cx: &mut App) -> Self {
        let standby = choice_select(
            STANDBY_MINUTES
                .iter()
                .map(|m| choice(m, format!("{m} 分")))
                .chain([choice("never", "スタンバイにしない")])
                .collect(),
            &current.timeout_to_standby,
            false,
            window,
            cx,
        );
        let date_format = choice_select(
            DATE_FORMATS.iter().map(|f| choice(f, *f)).collect(),
            &current.date_format,
            false,
            window,
            cx,
        );
        let time_format = choice_select(
            vec![choice("24hour", "24 時間表示"), choice("12hour", "12 時間表示 (午前 / 午後)")],
            &current.time_format,
            false,
            window,
            cx,
        );
        let timezone = choice_select(
            TIMEZONES.iter().map(|tz| choice(tz, timezone_label(tz))).collect(),
            &current.timezone,
            true,
            window,
            cx,
        );
        Self { current, standby, date_format, time_format, timezone }
    }

    fn render(&self, view: &Entity<QuadernoApp>) -> AnyElement {
        let sync_clock = {
            let view = view.clone();
            Button::new("sync-clock")
                .label("端末の日時をこの PC に合わせる")
                .small()
                .outline()
                .on_click(move |_, window, cx| view.update(cx, |this, cx| this.sync_device_clock(window, cx)))
        };
        v_form()
            .child(field().label("スタンバイまでの時間").child(Select::new(&self.standby)))
            .child(field().label("日付の形式").child(Select::new(&self.date_format)))
            .child(field().label("時刻の形式").child(Select::new(&self.time_format)))
            .child(field().label("タイムゾーン").child(Select::new(&self.timezone).menu_max_h(px(240.))))
            .child(field().label("日時").child(h_flex().child(sync_clock)))
            .into_any_element()
    }

    fn values(&self, cx: &App) -> DeviceSettings {
        let selected = |select: &ChoiceSelect, fallback: &str| {
            select.read(cx).selected_value().cloned().unwrap_or_else(|| fallback.to_string())
        };
        DeviceSettings {
            timeout_to_standby: selected(&self.standby, &self.current.timeout_to_standby),
            date_format: selected(&self.date_format, &self.current.date_format),
            time_format: selected(&self.time_format, &self.current.time_format),
            timezone: selected(&self.timezone, &self.current.timezone),
        }
    }
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
                _ => crate::wifi::wifi_tab(&view, &wifi, cx),
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

    fn save_device_settings(
        &mut self,
        current: DeviceSettings,
        new: DeviceSettings,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if current == new {
            return true;
        }
        let Some(client) = self.client() else { return true };
        self.run_task(
            "端末の設定を保存しています",
            window,
            cx,
            move |_| Ok(client.update_settings(&current, &new)?),
            |this, result, window, cx| match result {
                Ok(()) => this.notify_success("端末の設定を保存しました", window, cx),
                Err(e) => this.notify_error("端末の設定を保存できませんでした", &e, window, cx),
            },
        )
    }

    fn sync_device_clock(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(client) = self.client() else { return };
        self.run_task(
            "端末の日時を合わせています",
            window,
            cx,
            move |_| Ok(client.set_datetime(SystemTime::now())?),
            |this, result, window, cx| match result {
                Ok(()) => this.notify_success("端末の日時をこの PC に合わせました", window, cx),
                Err(e) => this.notify_error("端末の日時を合わせられませんでした", &e, window, cx),
            },
        );
    }
}

pub(crate) fn choice(value: &str, label: impl Into<SharedString>) -> Choice {
    Choice { value: value.to_string(), label: label.into() }
}

/// `current` を選んだ状態の選択欄。端末の値が選択肢になければ、その値も選択肢に加える。
pub(crate) fn choice_select(
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

/// `Etc/GMT-9` は UTC+9 を表す (符号が逆) ので、UTC からのずれで表示する。
fn timezone_label(tz: &str) -> String {
    match tz.strip_prefix("Etc/GMT") {
        Some(offset) => {
            let flipped: String = offset
                .chars()
                .map(|c| {
                    if c == '-' {
                        '+'
                    } else if c == '+' {
                        '-'
                    } else {
                        c
                    }
                })
                .collect();
            format!("UTC{flipped} ({tz})")
        }
        None => tz.to_string(),
    }
}
