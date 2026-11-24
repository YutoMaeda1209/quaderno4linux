//! File explorer: browsing, transfer and file operations.

use std::path::PathBuf;
use std::sync::Arc;

use gpui::{
    div, prelude::*, px, uniform_list, ClickEvent, Context, ExternalPaths, PathPromptOptions, ScrollStrategy,
    SharedString, Window,
};
use gpui_component::breadcrumb::{Breadcrumb, BreadcrumbItem};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::{h_flex, v_flex, ActiveTheme, Disableable, Icon, IconName, Sizable};
use quaderno_core::{format_date, format_size, DeviceClient, Entry};

use crate::app::actions::*;
use crate::app::{is_double_click, Clipboard, ClipboardMode, QuadernoApp};

const ROW_HEIGHT: f32 = 32.;

impl QuadernoApp {
    pub fn set_entries(&mut self, mut entries: Vec<Entry>) {
        entries.sort_by(|a, b| {
            b.is_folder()
                .cmp(&a.is_folder())
                .then_with(|| a.entry_name.to_lowercase().cmp(&b.entry_name.to_lowercase()))
        });
        self.selection.retain(|id| entries.iter().any(|e| &e.entry_id == id));
        self.entries = entries;
        self.cursor = self.cursor.filter(|&c| c < self.entries.len());
        self.anchor = self.anchor.filter(|&c| c < self.entries.len());
    }

    pub fn selected_entries(&self) -> Vec<Entry> {
        self.entries.iter().filter(|e| self.selection.contains(&e.entry_id)).cloned().collect()
    }

    fn require_client(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<Arc<DeviceClient>> {
        let client = self.client();
        if client.is_none() {
            self.notify_warning("端末に接続していません", window, cx);
        }
        client
    }

    /// Reloads the current folder.
    pub fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(client) = self.require_client(window, cx) else { return };
        let folder = self.current_folder();
        self.run_task(
            "読み込んでいます",
            window,
            cx,
            move |_| {
                let entries = client.list_folder(&folder)?;
                let status = crate::app::device_status(&client);
                Ok((entries, status))
            },
            |this, result, window, cx| match result {
                Ok((entries, new_status)) => {
                    this.set_entries(entries);
                    if let crate::app::Connection::Connected { status, .. } = &mut this.connection {
                        *status = new_status;
                    }
                }
                Err(e) => this.notify_error(format!("フォルダを読み込めません: {e:#}"), window, cx),
            },
        );
    }

    /// Navigates to the folder at breadcrumb depth `depth`, or into `child`.
    fn navigate(&mut self, path: Vec<(String, String)>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(client) = self.require_client(window, cx) else { return };
        let folder = path.last().map(|p| p.0.clone()).unwrap_or_default();
        self.run_task(
            "読み込んでいます",
            window,
            cx,
            move |_| Ok(client.list_folder(&folder)?),
            move |this, result, window, cx| match result {
                Ok(entries) => {
                    this.path = path;
                    this.selection.clear();
                    this.cursor = None;
                    this.anchor = None;
                    this.set_entries(entries);
                    this.list_scroll.scroll_to_item(0, ScrollStrategy::Top);
                }
                Err(e) => this.notify_error(format!("フォルダを開けません: {e:#}"), window, cx),
            },
        );
    }

    fn open_entry(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(entry) = self.entries.get(ix).cloned() else { return };
        if entry.is_folder() {
            let mut path = self.path.clone();
            path.push((entry.entry_id, entry.entry_name));
            self.navigate(path, window, cx);
        } else {
            self.selection = [entry.entry_id].into();
            self.download(window, cx);
        }
    }

    fn go_up(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.path.len() > 1 {
            let mut path = self.path.clone();
            path.pop();
            self.navigate(path, window, cx);
        }
    }

    // ----- selection ----------------------------------------------------

    fn click_row(&mut self, ix: usize, ev: &ClickEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus_handle);
        if is_double_click(ev) {
            self.open_entry(ix, window, cx);
            return;
        }
        let Some(entry) = self.entries.get(ix) else { return };
        let id = entry.entry_id.clone();
        let mods = ev.modifiers();
        if mods.shift {
            let anchor = self.anchor.unwrap_or(ix);
            let (a, b) = if anchor <= ix { (anchor, ix) } else { (ix, anchor) };
            if !mods.control {
                self.selection.clear();
            }
            for e in &self.entries[a..=b] {
                self.selection.insert(e.entry_id.clone());
            }
        } else if mods.control {
            if !self.selection.remove(&id) {
                self.selection.insert(id);
            }
            self.anchor = Some(ix);
        } else {
            self.selection = [id].into();
            self.anchor = Some(ix);
        }
        self.cursor = Some(ix);
        cx.notify();
    }

    fn move_cursor(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.entries.is_empty() {
            return;
        }
        let max = self.entries.len() as isize - 1;
        let ix = match self.cursor {
            Some(c) => (c as isize + delta).clamp(0, max) as usize,
            None => 0,
        };
        self.cursor = Some(ix);
        self.anchor = Some(ix);
        self.selection = [self.entries[ix].entry_id.clone()].into();
        self.list_scroll.scroll_to_item(ix, ScrollStrategy::Center);
        cx.notify();
    }

    // ----- operations ---------------------------------------------------

    pub fn new_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.require_client(window, cx).is_none() {
            return;
        }
        self.prompt_text(
            "新しいフォルダ",
            "フォルダ名を入力してください",
            "新しいフォルダ",
            "作成",
            window,
            cx,
            |this, name, window, cx| {
                let Some(client) = this.client() else { return };
                let parent = this.current_folder();
                this.run_task(
                    "フォルダを作成しています",
                    window,
                    cx,
                    move |_| {
                        client.create_folder(&parent, &name)?;
                        Ok((name, client.list_folder(&parent)?))
                    },
                    |this, result, window, cx| match result {
                        Ok((name, entries)) => {
                            this.set_entries(entries);
                            if let Some(e) = this.entries.iter().find(|e| e.entry_name == name) {
                                this.selection = [e.entry_id.clone()].into();
                            }
                            this.notify_success(format!("フォルダ「{name}」を作成しました"), window, cx);
                        }
                        Err(e) => this.notify_error(format!("フォルダを作成できません: {e:#}"), window, cx),
                    },
                );
            },
        );
    }

    pub fn rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let selected = self.selected_entries();
        let [entry] = selected.as_slice() else {
            self.notify_warning("名前を変更する項目を 1 つ選択してください", window, cx);
            return;
        };
        let entry = entry.clone();
        let current = entry.entry_name.clone();
        self.prompt_text(
            "名前の変更",
            format!("「{current}」の新しい名前"),
            current.clone(),
            "変更",
            window,
            cx,
            move |this, name, window, cx| {
                if name == current {
                    return;
                }
                let Some(client) = this.client() else { return };
                let entry = entry.clone();
                let parent = this.current_folder();
                this.run_task(
                    "名前を変更しています",
                    window,
                    cx,
                    move |_| {
                        client.rename(&entry, &name)?;
                        Ok((name, client.list_folder(&parent)?))
                    },
                    |this, result, window, cx| match result {
                        Ok((name, entries)) => {
                            this.set_entries(entries);
                            this.notify_success(format!("「{name}」に名前を変更しました"), window, cx);
                        }
                        Err(e) => this.notify_error(format!("名前を変更できません: {e:#}"), window, cx),
                    },
                );
            },
        );
    }

    pub fn delete(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let selected = self.selected_entries();
        if selected.is_empty() {
            self.notify_warning("削除する項目を選択してください", window, cx);
            return;
        }
        let message = if selected.len() == 1 {
            let e = &selected[0];
            if e.is_folder() {
                format!("フォルダ「{}」とその中身をすべて削除します。元に戻せません。", e.entry_name)
            } else {
                format!("「{}」を削除します。元に戻せません。", e.entry_name)
            }
        } else {
            format!("{} 件の項目を削除します。フォルダは中身ごと削除されます。元に戻せません。", selected.len())
        };
        self.confirm("削除の確認", message, "削除", true, window, cx, move |this, window, cx| {
            let Some(client) = this.client() else { return };
            let entries = selected.clone();
            let parent = this.current_folder();
            this.run_task(
                "削除しています",
                window,
                cx,
                move |progress| {
                    for e in &entries {
                        progress.set_detail(e.entry_name.clone());
                        client.delete(e)?;
                    }
                    Ok((entries.len(), client.list_folder(&parent)?))
                },
                |this, result, window, cx| match result {
                    Ok((n, entries)) => {
                        this.set_entries(entries);
                        this.notify_success(format!("{n} 件を削除しました"), window, cx);
                    }
                    Err(e) => {
                        this.notify_error(format!("削除できません: {e:#}"), window, cx);
                        this.refresh(window, cx);
                    }
                },
            );
        });
    }

    fn set_clipboard(&mut self, mode: ClipboardMode, window: &mut Window, cx: &mut Context<Self>) {
        let entries = self.selected_entries();
        if entries.is_empty() {
            self.notify_warning("項目を選択してください", window, cx);
            return;
        }
        let verb = if mode == ClipboardMode::Copy { "コピー" } else { "切り取り" };
        self.push_log(format!("{} 件を{verb}しました。移動先のフォルダで「貼り付け」を押してください", entries.len()));
        self.clipboard = Some(Clipboard { mode, entries, source_folder: self.current_folder() });
        cx.notify();
    }

    pub fn paste(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(client) = self.require_client(window, cx) else { return };
        let Some(clip) = self.clipboard.as_ref() else {
            self.notify_warning("クリップボードが空です", window, cx);
            return;
        };
        let dest = self.current_folder();
        // A folder must not be copied or moved into itself or its descendants.
        let ancestors: Vec<&str> = self.path.iter().map(|p| p.0.as_str()).collect();
        if let Some(bad) = clip.entries.iter().find(|e| e.is_folder() && ancestors.contains(&e.entry_id.as_str())) {
            let msg = format!("フォルダ「{}」をそれ自身の中に貼り付けることはできません", bad.entry_name);
            self.notify_error(msg, window, cx);
            return;
        }
        let mode = clip.mode;
        let same_folder = clip.source_folder == dest;
        if mode == ClipboardMode::Cut && same_folder {
            self.clipboard = None;
            cx.notify();
            return;
        }
        let entries = clip.entries.clone();
        let existing: Vec<String> = self.entries.iter().map(|e| e.entry_name.clone()).collect();
        let label = if mode == ClipboardMode::Copy { "コピーしています" } else { "移動しています" };
        self.run_task(
            label,
            window,
            cx,
            move |progress| {
                let mut names = existing;
                for e in &entries {
                    progress.set_detail(e.entry_name.clone());
                    match mode {
                        ClipboardMode::Copy => {
                            let name = unique_name(&e.entry_name, &names, e.is_folder());
                            client.copy_to(e, &dest, Some(&name))?;
                            names.push(name);
                        }
                        ClipboardMode::Cut => client.move_to(e, &dest)?,
                    }
                }
                Ok((entries.len(), client.list_folder(&dest)?))
            },
            move |this, result, window, cx| match result {
                Ok((n, entries)) => {
                    if mode == ClipboardMode::Cut {
                        this.clipboard = None;
                    }
                    this.set_entries(entries);
                    let verb = if mode == ClipboardMode::Copy { "コピー" } else { "移動" };
                    this.notify_success(format!("{n} 件を{verb}しました"), window, cx);
                }
                Err(e) => {
                    this.notify_error(format!("貼り付けに失敗しました: {e:#}"), window, cx);
                    this.refresh(window, cx);
                }
            },
        );
    }

    /// Asks for local files (or folders) and uploads them to the current folder.
    pub fn upload(&mut self, directories: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.require_client(window, cx).is_none() {
            return;
        }
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: !directories,
            directories,
            multiple: true,
            prompt: Some(if directories { "フォルダをアップロード" } else { "アップロード" }.into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = rx.await;
            let _ = this.update_in(cx, |this, window, cx| match result {
                Ok(Ok(Some(paths))) => this.upload_paths(paths, window, cx),
                Ok(Err(e)) => this.notify_file_dialog_error(e, window, cx),
                _ => {}
            });
        })
        .detach();
    }

    pub fn upload_paths(&mut self, paths: Vec<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(client) = self.require_client(window, cx) else { return };
        if paths.is_empty() {
            return;
        }
        let non_pdf = paths
            .iter()
            .filter(|p| p.is_file() && !p.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf")))
            .count();
        if non_pdf > 0 {
            self.notify_warning(
                format!("PDF 以外のファイルが {non_pdf} 件含まれています。端末で開けない可能性があります"),
                window,
                cx,
            );
        }
        let dest = self.current_folder();
        self.run_task(
            "アップロードしています",
            window,
            cx,
            move |progress| {
                let cb = progress.callback();
                for p in &paths {
                    progress.set_detail(p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
                    if p.is_dir() {
                        client.upload_dir(p, &dest, &cb)?;
                    } else {
                        client.upload_file(p, &dest, &cb)?;
                    }
                }
                Ok((paths.len(), client.list_folder(&dest)?))
            },
            |this, result, window, cx| match result {
                Ok((n, entries)) => {
                    this.set_entries(entries);
                    this.notify_success(format!("{n} 件をアップロードしました"), window, cx);
                }
                Err(e) => {
                    this.notify_error(format!("アップロードに失敗しました: {e:#}"), window, cx);
                    this.refresh(window, cx);
                }
            },
        );
    }

    /// Asks for a local directory and downloads the selection into it.
    pub fn download(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.require_client(window, cx).is_none() {
            return;
        }
        let entries = self.selected_entries();
        if entries.is_empty() {
            self.notify_warning("ダウンロードする項目を選択してください", window, cx);
            return;
        }
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("ここに保存".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = rx.await;
            let _ = this.update_in(cx, |this, window, cx| match result {
                Ok(Ok(Some(mut dirs))) => {
                    if let Some(dir) = dirs.pop() {
                        this.download_to(entries, dir, window, cx);
                    }
                }
                Ok(Err(e)) => this.notify_file_dialog_error(e, window, cx),
                _ => {}
            });
        })
        .detach();
    }

    fn notify_file_dialog_error(&mut self, e: anyhow::Error, window: &mut Window, cx: &mut Context<Self>) {
        self.notify_error(
            format!(
                "ファイル選択ダイアログを開けません。xdg-desktop-portal (GNOME/KDE/GTK 実装) が必要です。\
                 ファイルをウィンドウにドラッグ＆ドロップしてアップロードすることもできます: {e:#}"
            ),
            window,
            cx,
        );
    }

    fn download_to(&mut self, entries: Vec<Entry>, dir: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let Some(client) = self.require_client(window, cx) else { return };
        self.run_task(
            "ダウンロードしています",
            window,
            cx,
            move |progress| {
                let cb = progress.callback();
                for e in &entries {
                    progress.set_detail(e.entry_name.clone());
                    client.download_entry(e, &dir, &cb)?;
                }
                Ok((entries.len(), dir))
            },
            |this, result, window, cx| match result {
                Ok((n, dir)) => {
                    this.notify_success(format!("{n} 件を {} にダウンロードしました", dir.display()), window, cx)
                }
                Err(e) => this.notify_error(format!("ダウンロードに失敗しました: {e:#}"), window, cx),
            },
        );
    }

    // ----- action handlers ---------------------------------------------

    pub fn on_refresh(&mut self, _: &Refresh, window: &mut Window, cx: &mut Context<Self>) {
        self.refresh(window, cx);
    }
    pub fn on_go_up(&mut self, _: &GoUp, window: &mut Window, cx: &mut Context<Self>) {
        self.go_up(window, cx);
    }
    pub fn on_open_selected(&mut self, _: &OpenSelected, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ix) = self.cursor {
            self.open_entry(ix, window, cx);
        }
    }
    pub fn on_delete_selected(&mut self, _: &DeleteSelected, window: &mut Window, cx: &mut Context<Self>) {
        self.delete(window, cx);
    }
    pub fn on_rename_selected(&mut self, _: &RenameSelected, window: &mut Window, cx: &mut Context<Self>) {
        self.rename(window, cx);
    }
    pub fn on_copy_selected(&mut self, _: &CopySelected, window: &mut Window, cx: &mut Context<Self>) {
        self.set_clipboard(ClipboardMode::Copy, window, cx);
    }
    pub fn on_cut_selected(&mut self, _: &CutSelected, window: &mut Window, cx: &mut Context<Self>) {
        self.set_clipboard(ClipboardMode::Cut, window, cx);
    }
    pub fn on_paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        self.paste(window, cx);
    }
    pub fn on_select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.selection = self.entries.iter().map(|e| e.entry_id.clone()).collect();
        cx.notify();
    }
    pub fn on_select_prev(&mut self, _: &SelectPrev, _: &mut Window, cx: &mut Context<Self>) {
        self.move_cursor(-1, cx);
    }
    pub fn on_select_next(&mut self, _: &SelectNext, _: &mut Window, cx: &mut Context<Self>) {
        self.move_cursor(1, cx);
    }
    pub fn on_clear_selection(&mut self, _: &ClearSelection, _: &mut Window, cx: &mut Context<Self>) {
        self.selection.clear();
        cx.notify();
    }
    pub fn on_new_folder(&mut self, _: &NewFolder, window: &mut Window, cx: &mut Context<Self>) {
        self.new_folder(window, cx);
    }
    pub fn on_upload(&mut self, _: &Upload, window: &mut Window, cx: &mut Context<Self>) {
        self.upload(false, window, cx);
    }
    pub fn on_download(&mut self, _: &Download, window: &mut Window, cx: &mut Context<Self>) {
        self.download(window, cx);
    }

    // ----- rendering ----------------------------------------------------

    fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let enabled = self.client().is_some() && self.busy.is_none();
        let has_sel = !self.selection.is_empty();
        let one_sel = self.selection.len() == 1;
        let can_paste = self.clipboard.is_some();

        let btn = |id: &'static str, label: &'static str, icon: IconName, tip: &'static str| {
            Button::new(id).label(label).icon(icon).small().ghost().tooltip(tip)
        };

        h_flex()
            .gap_1()
            .px_2()
            .py_1()
            .flex_wrap()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                btn("up", "上へ", IconName::ArrowUp, "親フォルダへ (Backspace)")
                    .disabled(!enabled || self.path.len() <= 1)
                    .on_click(cx.listener(|this, _, window, cx| this.go_up(window, cx))),
            )
            .child(
                btn("refresh", "更新", IconName::Redo, "再読み込み (F5)")
                    .disabled(!enabled)
                    .on_click(cx.listener(|this, _, window, cx| this.refresh(window, cx))),
            )
            .child(separator(cx))
            .child(
                btn("upload", "アップロード", IconName::ArrowUp, "ファイルをアップロード (Ctrl+U)")
                    .disabled(!enabled)
                    .on_click(cx.listener(|this, _, window, cx| this.upload(false, window, cx))),
            )
            .child(
                btn("upload-dir", "フォルダをアップロード", IconName::FolderOpen, "フォルダごとアップロード")
                    .disabled(!enabled)
                    .on_click(cx.listener(|this, _, window, cx| this.upload(true, window, cx))),
            )
            .child(
                btn("download", "ダウンロード", IconName::ArrowDown, "選択項目をダウンロード (Ctrl+D)")
                    .disabled(!enabled || !has_sel)
                    .on_click(cx.listener(|this, _, window, cx| this.download(window, cx))),
            )
            .child(separator(cx))
            .child(
                btn("new-folder", "新しいフォルダ", IconName::Plus, "フォルダを作成 (Ctrl+Shift+N)")
                    .disabled(!enabled)
                    .on_click(cx.listener(|this, _, window, cx| this.new_folder(window, cx))),
            )
            .child(
                btn("copy", "コピー", IconName::Copy, "コピー (Ctrl+C)")
                    .disabled(!enabled || !has_sel)
                    .on_click(cx.listener(|this, _, window, cx| this.set_clipboard(ClipboardMode::Copy, window, cx))),
            )
            .child(
                btn("cut", "切り取り", IconName::Replace, "移動のため切り取り (Ctrl+X)")
                    .disabled(!enabled || !has_sel)
                    .on_click(cx.listener(|this, _, window, cx| this.set_clipboard(ClipboardMode::Cut, window, cx))),
            )
            .child(
                btn("paste", "貼り付け", IconName::Inbox, "このフォルダに貼り付け (Ctrl+V)")
                    .disabled(!enabled || !can_paste)
                    .on_click(cx.listener(|this, _, window, cx| this.paste(window, cx))),
            )
            .child(
                btn("rename", "名前の変更", IconName::ALargeSmall, "名前を変更 (F2)")
                    .disabled(!enabled || !one_sel)
                    .on_click(cx.listener(|this, _, window, cx| this.rename(window, cx))),
            )
            .child(
                Button::new("delete")
                    .label("削除")
                    .icon(IconName::Delete)
                    .small()
                    .ghost()
                    .tooltip("削除 (Delete)")
                    .disabled(!enabled || !has_sel)
                    .on_click(cx.listener(|this, _, window, cx| this.delete(window, cx))),
            )
    }

    fn render_breadcrumb(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let enabled = self.client().is_some() && self.busy.is_none();
        let items = self.path.iter().enumerate().map(|(depth, (_, name))| {
            let last = depth + 1 == self.path.len();
            BreadcrumbItem::new(SharedString::from(name.clone())).disabled(!enabled || last).on_click(cx.listener(
                move |this, _, window, cx| {
                    let path = this.path[..=depth].to_vec();
                    this.navigate(path, window, cx);
                },
            ))
        });
        h_flex().px_3().py_2().child(Breadcrumb::new().children(items))
    }

    pub fn render_explorer(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let connected = self.client().is_some();
        let count = self.entries.len();

        let header = h_flex()
            .h(px(28.))
            .px_3()
            .gap_2()
            .flex_shrink_0()
            .bg(theme.table_head)
            .text_xs()
            .text_color(theme.muted_foreground)
            .child(div().flex_1().child("名前"))
            .child(div().w(px(90.)).text_right().child("サイズ"))
            .child(div().w(px(60.)).text_right().child("ページ"))
            .child(div().w(px(140.)).child("更新日時"));

        let body = if !connected {
            v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .gap_2()
                .text_color(theme.muted_foreground)
                .child(Icon::new(IconName::BookOpen).size_8())
                .child("端末に接続してください")
                .child(
                    div()
                        .text_xs()
                        .child("USB は「USB で接続」、Bluetooth はペアリング済み端末の「接続」、Wi-Fi は「端末を検索」から接続します"),
                )
                .into_any_element()
        } else if count == 0 {
            v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .text_color(theme.muted_foreground)
                .child("このフォルダは空です (ファイルをドロップしてアップロード)")
                .into_any_element()
        } else {
            uniform_list(
                "entries",
                count,
                cx.processor(move |this, range: std::ops::Range<usize>, _window, cx| {
                    let theme = cx.theme();
                    range
                        .map(|ix| {
                            let e = &this.entries[ix];
                            let selected = this.selection.contains(&e.entry_id);
                            let cut = this.clipboard.as_ref().is_some_and(|c| {
                                c.mode == ClipboardMode::Cut && c.entries.iter().any(|x| x.entry_id == e.entry_id)
                            });
                            let icon = if e.is_folder() { IconName::Folder } else { IconName::File };
                            h_flex()
                                .id(("entry", ix))
                                .w_full()
                                .h(px(ROW_HEIGHT))
                                .px_3()
                                .gap_2()
                                .text_sm()
                                .border_b_1()
                                .border_color(theme.table_row_border)
                                .when(ix % 2 == 1, |el| el.bg(theme.table_even))
                                .when(selected, |el| el.bg(theme.table_active))
                                .when(!selected, |el| el.hover(|s| s.bg(theme.table_hover)))
                                .when(cut, |el| el.opacity(0.5))
                                .child(
                                    h_flex()
                                        .flex_1()
                                        .min_w_0()
                                        .gap_2()
                                        .child(Icon::new(icon).small().text_color(if e.is_folder() {
                                            theme.primary
                                        } else {
                                            theme.muted_foreground
                                        }))
                                        .child(div().truncate().child(e.entry_name.clone())),
                                )
                                .child(
                                    div()
                                        .w(px(90.))
                                        .text_right()
                                        .text_color(theme.muted_foreground)
                                        .child(e.file_size.map(format_size).unwrap_or_default()),
                                )
                                .child(
                                    div()
                                        .w(px(60.))
                                        .text_right()
                                        .text_color(theme.muted_foreground)
                                        .child(e.total_page.map(|p| p.to_string()).unwrap_or_default()),
                                )
                                .child(
                                    div().w(px(140.)).text_color(theme.muted_foreground).child(
                                        e.modified_date
                                            .as_deref()
                                            .or(e.created_date.as_deref())
                                            .map(format_date)
                                            .unwrap_or_default(),
                                    ),
                                )
                                .on_click(cx.listener(move |this, ev: &ClickEvent, window, cx| {
                                    this.click_row(ix, ev, window, cx)
                                }))
                        })
                        .collect::<Vec<_>>()
                }),
            )
            .track_scroll(self.list_scroll.clone())
            .w_full()
            .flex_1()
            .into_any_element()
        };

        let footer = h_flex().px_3().h(px(24.)).text_xs().text_color(theme.muted_foreground).child(if connected {
            format!("{count} 項目 · {} 件選択", self.selection.len())
        } else {
            String::new()
        });

        v_flex()
            .id("explorer")
            .flex_1()
            .min_w_0()
            .h_full()
            .child(self.render_toolbar(cx))
            .child(self.render_breadcrumb(cx))
            .child(v_flex().flex_1().min_h_0().child(header).child(body))
            .child(footer)
            .drag_over::<ExternalPaths>(move |style, _, _, _| style.bg(theme.drop_target))
            .on_drop(cx.listener(|this, paths: &ExternalPaths, window, cx| {
                this.upload_paths(paths.paths().to_vec(), window, cx);
            }))
    }
}

fn separator(cx: &Context<QuadernoApp>) -> impl IntoElement {
    div().w(px(1.)).h(px(18.)).mx_1().bg(cx.theme().border)
}

/// Returns `name` or, if taken, `name (コピー)`, `name (コピー 2)`, ... (keeping the extension).
pub fn unique_name(name: &str, existing: &[String], is_folder: bool) -> String {
    if !existing.iter().any(|n| n == name) {
        return name.to_string();
    }
    let (stem, ext) = match name.rfind('.') {
        Some(i) if !is_folder && i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    (1..)
        .map(|n| if n == 1 { format!("{stem} (コピー){ext}") } else { format!("{stem} (コピー {n}){ext}") })
        .find(|candidate| !existing.iter().any(|e| e == candidate))
        .expect("an unused name exists")
}

#[cfg(test)]
mod tests {
    use super::unique_name;

    #[test]
    fn unique_names() {
        let existing = vec!["a.pdf".to_string(), "a (コピー).pdf".to_string(), "dir.x".to_string()];
        assert_eq!(unique_name("b.pdf", &existing, false), "b.pdf");
        assert_eq!(unique_name("a.pdf", &existing, false), "a (コピー 2).pdf");
        assert_eq!(unique_name("dir.x", &existing, true), "dir.x (コピー)");
    }
}
