//! 画面
//!
//! - 本体への書き込み（`call_edit_section`）は、クリックと Enter で挿入するときだけ行う。
//!   イベントやポーリングからは呼ばない（`.claude/rules/au2-rs-plugin.md`「操作中の本体 Undo を捨てる」）
//! - パネルは上 → 下 → 中央の順に足す（`CentralPanel` を先に足すと下のパネルが隠れる）

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::Context as _;
use aviutl2::tracing;
use aviutl2_eframe::{AviUtl2EframeHandle, eframe, egui};
use chrono::{Local, TimeZone};
use itertools::Itertools;
use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};

use crate::config::{self, Config, KindFilter, SortKey};
use crate::history::{self, History};
use crate::index::{Entry, Index, Kind, Scanner};
use crate::kana::{normalize_kana_for_search, normalize_kana_for_search_with_map};
use crate::query::Query;
use crate::watcher::FolderWatcher;

#[derive(Debug, Clone)]
struct Hit {
    entry: usize,
    /// true ならファイル名で一致（`indices` は名前の中の位置）、false ならパス全体で一致
    by_name: bool,
    score: u32,
    /// 一致した文字の位置（あいまい検索をしていなければ空）
    indices: Vec<u32>,
}

#[derive(Debug, Default)]
struct Results {
    hits: Vec<Hit>,
    /// 解釈できなかった `ext:` `size:` `dm:`
    errors: Vec<String>,
}

/// これが変わったら結果を計算し直す
#[derive(Debug, Clone, PartialEq)]
struct ResultsKey {
    needle: String,
    index: usize,
    kind: KindFilter,
    sort: SortKey,
    descending: bool,
    history_gen: u64,
    /// `dm:today` などが時刻で変わるので、分が変わったら計算し直す
    minute: i64,
}

pub(crate) struct FileFinderApp {
    handle: AviUtl2EframeHandle,
    config: Config,
    config_path: PathBuf,
    /// false なら設定ファイルを読めず退避もできなかったので、この起動の間は保存しない
    config_writable: bool,
    scanner: Scanner,
    watcher: Option<FolderWatcher>,
    matcher: nucleo_matcher::Matcher,
    history: History,
    history_path: PathBuf,
    /// false なら挿入履歴を読めず退避もできなかったので、この起動の間は保存しない
    history_writable: bool,
    history_gen: u64,

    needle: String,
    results: Option<(ResultsKey, Arc<Results>)>,
    selected: usize,
    /// 一覧が作り直されても同じファイルを選んだままにするため
    selected_path: Option<PathBuf>,
    scroll_to_selected: bool,
    scroll_offset: f32,
    viewport_height: f32,
    focus_search: bool,

    show_settings: bool,
    draft: Config,
    new_root_text: String,
    dialog: Option<mpsc::Receiver<Option<PathBuf>>>,
    clear_history_armed: Option<Instant>,
    /// Esc で設定の小窓を閉じてよいかの判定
    esc_dismiss: EscDismiss,

    status: String,
    status_is_error: bool,
}

fn play_beep() {
    let _ = unsafe {
        windows::Win32::System::Diagnostics::Debug::MessageBeep(
            windows::Win32::UI::WindowsAndMessaging::MB_ICONEXCLAMATION,
        )
    };
}

fn now_secs() -> i64 {
    Local::now().timestamp()
}

impl FileFinderApp {
    pub(crate) fn new(cc: &eframe::CreationContext<'_>, handle: AviUtl2EframeHandle) -> Self {
        cc.egui_ctx.all_styles_mut(|style| {
            style.visuals = aviutl2_eframe::aviutl2_visuals();
        });
        cc.egui_ctx.set_fonts(aviutl2_eframe::aviutl2_fonts());

        let config_path = config::default_path();
        let config = config::load(&config_path);
        let history_path = history::default_path();
        let history = history::load(&history_path);
        let warning = [config.warning, history.warning].into_iter().flatten().join(" / ");
        let (config, config_writable) = (config.value, config.writable);
        let (history, history_writable) = (history.value, history.writable);
        if !warning.is_empty() {
            tracing::warn!("FileFinder_H: {warning}");
        }

        let mut app = Self {
            handle,
            draft: config.clone(),
            config,
            config_path,
            config_writable,
            scanner: Scanner::default(),
            watcher: None,
            matcher: nucleo_matcher::Matcher::new(nucleo_matcher::Config::DEFAULT.match_paths()),
            history,
            history_path,
            history_writable,
            history_gen: 0,
            needle: String::new(),
            results: None,
            selected: 0,
            selected_path: None,
            scroll_to_selected: false,
            scroll_offset: 0.0,
            viewport_height: 0.0,
            focus_search: true,
            show_settings: false,
            new_root_text: String::new(),
            dialog: None,
            clear_history_armed: None,
            esc_dismiss: EscDismiss::default(),
            status_is_error: !warning.is_empty(),
            status: warning,
        };
        if !app.config.roots.is_empty() {
            app.scanner.start(&app.config, cc.egui_ctx.clone());
        }
        app.restart_watcher(&cc.egui_ctx);
        app
    }

    fn set_status(&mut self, text: impl Into<String>, is_error: bool) {
        self.status = text.into();
        self.status_is_error = is_error;
    }

    fn save_config(&mut self) {
        if !self.config_writable {
            self.set_status(
                format!(
                    "設定ファイルを読めなかったので、この起動の間は設定を保存しません: {}",
                    self.config_path.display()
                ),
                true,
            );
            return;
        }
        if let Err(e) = config::save(&self.config_path, &self.config) {
            tracing::warn!("FileFinder_H: 設定を保存できませんでした: {e:#}");
            self.set_status(format!("設定を保存できませんでした: {e:#}"), true);
        }
    }

    /// 設定画面の内容を反映して読み込み直す（表示のしかたの設定はそのまま）
    fn apply_settings(&mut self, draft: &Config, ctx: &egui::Context) {
        self.config.roots = draft.roots.clone();
        self.config.extensions = draft.extensions.clone();
        self.config.all_files = draft.all_files;
        self.config.include_hidden = draft.include_hidden;
        self.config.auto_update = draft.auto_update;
        self.save_config();
        self.results = None;
        self.scanner.start(&self.config, ctx.clone());
        self.restart_watcher(ctx);
    }

    fn restart_watcher(&mut self, ctx: &egui::Context) {
        self.watcher = None;
        if !self.config.auto_update || self.config.roots.is_empty() {
            return;
        }
        let scanner = self.scanner.clone();
        let config = self.config.clone();
        let ctx = ctx.clone();
        let watcher = FolderWatcher::start(&self.config, move || scanner.start(&config, ctx.clone()));
        if !watcher.failed.is_empty() {
            self.set_status(
                format!("自動更新できないフォルダがあります（⟳ で読み込み直してください）: {}", watcher.failed.join(", ")),
                true,
            );
        }
        self.watcher = Some(watcher);
    }

    fn record_use(&mut self, path: &Path) {
        self.history.bump(path, now_secs());
        self.history_gen += 1;
        self.save_history();
    }

    /// 読めなかった履歴ファイルは上書きしない（起動時に知らせてある）
    fn save_history(&mut self) -> bool {
        if !self.history_writable {
            return false;
        }
        if let Err(e) = history::save(&self.history_path, &self.history) {
            tracing::warn!("FileFinder_H: 挿入履歴を保存できませんでした: {e:#}");
        }
        true
    }

    /// 自分のウィンドウ（フォルダ選択ダイアログのオーナーを求める元）
    fn own_hwnd(&self) -> Option<isize> {
        use aviutl2::raw_window_handle::{HasWindowHandle, RawWindowHandle};
        match self.handle.window_handle().ok()?.as_raw() {
            RawWindowHandle::Win32(h) => Some(h.hwnd.get()),
            _ => None,
        }
    }
}

impl eframe::App for FileFinderApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let esc_closes = self.esc_dismiss.pressed(ui.ctx());
        self.poll_dialog();
        let results = self.current_results();

        egui::Panel::top("toolbar").show(ui, |ui| self.render_toolbar(ui, results.as_deref()));
        egui::Panel::bottom("status").show(ui, |ui| self.render_status(ui, results.as_deref()));
        egui::CentralPanel::default().show(ui, |ui| self.render_list(ui, results));
        self.render_settings(ui);
        // フォルダ選択ダイアログの結果を待っている間は閉じない（選んだフォルダの行き先が無くなる）
        if esc_closes && self.show_settings && self.dialog.is_none() {
            self.show_settings = false;
        }
        self.esc_dismiss.end_frame(ui.ctx());
    }
}

// ---- 検索 ----

impl FileFinderApp {
    fn current_results(&mut self) -> Option<Arc<Results>> {
        let index = self.scanner.index()?;
        let key = ResultsKey {
            needle: self.needle.trim().to_string(),
            index: Arc::as_ptr(&index) as usize,
            kind: self.config.kind_filter,
            sort: self.config.sort,
            descending: self.config.descending,
            history_gen: self.history_gen,
            minute: now_secs() / 60,
        };
        if let Some((k, r)) = &self.results
            && *k == key
        {
            return Some(Arc::clone(r));
        }
        let needle_changed = self.results.as_ref().is_none_or(|(k, _)| k.needle != key.needle);
        let results = Arc::new(compute_results(
            &mut self.matcher,
            &index,
            &key.needle,
            key.kind,
            key.sort,
            key.descending,
            &self.history,
        ));
        // 検索語が変わったら先頭へ。それ以外（読み込み直し・並べ替え）なら同じファイルを選んだまま
        self.selected = if needle_changed {
            0
        } else {
            self.selected_path
                .as_ref()
                .and_then(|p| results.hits.iter().position(|h| &index.entries[h.entry].path == p))
                .unwrap_or(0)
        };
        self.scroll_to_selected = true;
        self.results = Some((key, Arc::clone(&results)));
        Some(results)
    }
}

fn compute_results(
    matcher: &mut nucleo_matcher::Matcher,
    index: &Index,
    needle: &str,
    kind: KindFilter,
    sort: SortKey,
    descending: bool,
    history: &History,
) -> Results {
    let query = Query::parse(needle, Local::now());
    let candidates = index
        .entries
        .iter()
        .enumerate()
        .filter(|(_, e)| kind.matches(e.kind) && query.matches_attrs(e));
    let has_text = !query.text.is_empty();
    let mut hits: Vec<Hit> = if has_text {
        let pattern = Pattern::parse(
            normalize_kana_for_search(&query.text).as_str(),
            CaseMatching::Smart,
            Normalization::Smart,
        );
        candidates
            .filter_map(|(i, e)| {
                let mut indices = Vec::new();
                if let Some(score) = pattern.indices(e.search_name.slice(..), matcher, &mut indices) {
                    return Some(Hit { entry: i, by_name: true, score, indices });
                }
                indices.clear();
                pattern
                    .indices(e.search_path.slice(..), matcher, &mut indices)
                    .map(|score| Hit { entry: i, by_name: false, score, indices })
            })
            .collect()
    } else {
        candidates
            .map(|(i, _)| Hit { entry: i, by_name: false, score: 0, indices: Vec::new() })
            .collect()
    };
    sort_hits(&mut hits, index, sort, descending, history, has_text);
    Results { hits, errors: query.errors }
}

fn sort_hits(
    hits: &mut Vec<Hit>,
    index: &Index,
    sort: SortKey,
    descending: bool,
    history: &History,
    has_text: bool,
) {
    use std::cmp::Ordering;
    let entry = |h: &Hit| &index.entries[h.entry];
    if sort == SortKey::Relevance {
        // 検索語が無ければ読み込んだ順（フォルダ順）のまま
        if has_text {
            // 名前で一致したものを先に、同じ組の中はスコアの高い順、同点なら短いパスを先に
            hits.sort_by(|a, b| {
                b.by_name
                    .cmp(&a.by_name)
                    .then(b.score.cmp(&a.score))
                    .then_with(|| entry(a).sort_path.len().cmp(&entry(b).sort_path.len()))
                    .then_with(|| entry(a).sort_path.cmp(&entry(b).sort_path))
            });
        }
        return;
    }
    // 履歴の値は比べるたびに引かず、先に引いておく
    let mut keyed: Vec<(Option<i64>, Hit)> = std::mem::take(hits)
        .into_iter()
        .map(|h| {
            let record = match sort {
                SortKey::RecentlyInserted | SortKey::InsertCount => history.get(&entry(&h).path),
                _ => None,
            };
            let value = record.map(|r| if sort == SortKey::InsertCount { r.count as i64 } else { r.last });
            (value, h)
        })
        .collect();
    keyed.sort_by(|(ka, a), (kb, b)| {
        let (ea, eb) = (entry(a), entry(b));
        let primary = match sort {
            SortKey::Name => ea.sort_name.cmp(&eb.sort_name),
            SortKey::Path => ea.sort_path.cmp(&eb.sort_path),
            SortKey::Size => ea.size.cmp(&eb.size),
            SortKey::Modified => ea.modified.cmp(&eb.modified),
            SortKey::RecentlyInserted | SortKey::InsertCount => match (ka, kb) {
                (Some(x), Some(y)) => x.cmp(y),
                // 一度も挿入していないものは、向きに関係なく後ろ
                (Some(_), None) => return Ordering::Less,
                (None, Some(_)) => return Ordering::Greater,
                (None, None) => Ordering::Equal,
            },
            SortKey::Relevance => Ordering::Equal,
        };
        let primary = if descending { primary.reverse() } else { primary };
        primary.then_with(|| ea.sort_path.cmp(&eb.sort_path))
    });
    *hits = keyed.into_iter().map(|(_, h)| h).collect();
}

// ---- 挿入・ドラッグ ----

fn insert_entry(entry: &Entry) -> anyhow::Result<()> {
    anyhow::ensure!(crate::EDIT_HANDLE.is_ready(), "編集 API の準備ができていません");
    let path = entry.path.clone();
    // オブジェクトエイリアスはファイルの中身をエイリアスとして渡す
    let alias = if entry.kind == Kind::Alias {
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("{} を読めませんでした", path.display()))?;
        Some(text.trim_start_matches('\u{feff}').to_string())
    } else {
        None
    };
    crate::EDIT_HANDLE.call_edit_section(move |e| -> anyhow::Result<()> {
        let (layer, frame) = (e.info.layer, e.info.frame);
        let created = match &alias {
            Some(text) => e.create_object_from_alias(text, layer, frame, 0),
            None => e.create_object_from_media_file(&path, layer, frame, None),
        }
        .map_err(|err| {
            anyhow::anyhow!("挿入できませんでした（対応していない形式か、その位置に置けません）: {err}")
        })?;
        e.set_focus_object(Some(created))?;
        Ok(())
    })?
}

/// explorer に渡す（`/select,` は引数を丸ごとクォートされると効かないので raw_arg で渡す）
fn run_explorer(arg: String) {
    use std::os::windows::process::CommandExt;
    if let Err(e) = std::process::Command::new("explorer").raw_arg(arg).spawn() {
        tracing::warn!("FileFinder_H: エクスプローラーを起動できませんでした: {e}");
    }
}

impl FileFinderApp {
    fn do_insert(&mut self, entry: &Entry) {
        match insert_entry(entry) {
            Ok(()) => {
                self.record_use(&entry.path);
                self.set_status(format!("挿入しました: {}", entry.name), false);
            }
            Err(e) => {
                play_beep();
                tracing::warn!("FileFinder_H: {} を挿入できませんでした: {e:#}", entry.path.display());
                self.set_status(format!("{}: {e:#}", entry.name), true);
            }
        }
    }

    fn do_drag(&mut self, entry: &Entry) {
        match crate::dnd::drag_files(&[entry.path.as_path()]) {
            Ok(true) => {
                self.record_use(&entry.path);
                self.set_status(format!("ドロップしました: {}", entry.name), false);
            }
            Ok(false) => {}
            Err(e) => {
                tracing::warn!("FileFinder_H: ドラッグできませんでした: {e}");
                self.set_status(format!("ドラッグできませんでした: {e}"), true);
            }
        }
    }

    fn insert_selected(&mut self, results: Option<&Results>) {
        let (Some(index), Some(results)) = (self.scanner.index(), results) else {
            play_beep();
            return;
        };
        match results.hits.get(self.selected) {
            Some(hit) => self.do_insert(&index.entries[hit.entry]),
            None => play_beep(),
        }
    }
}

// ---- 描画 ----

impl FileFinderApp {
    fn render_toolbar(&mut self, ui: &mut egui::Ui, results: Option<&Results>) {
        let total = results.map_or(0, |r| r.hits.len());
        // 上下キーは検索欄より先に取る（欄の中のカーソル移動に使わせない）
        let (up, down, page_up, page_down) = ui.input_mut(|i| {
            (
                i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp),
                i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown),
                i.consume_key(egui::Modifiers::NONE, egui::Key::PageUp),
                i.consume_key(egui::Modifiers::NONE, egui::Key::PageDown),
            )
        });
        if total > 0 {
            let page = ((self.viewport_height / row_stride(ui)).floor() as usize).max(1);
            let last = total - 1;
            let before = self.selected;
            if up {
                self.selected = self.selected.saturating_sub(1);
            }
            if down {
                self.selected = (self.selected + 1).min(last);
            }
            if page_up {
                self.selected = self.selected.saturating_sub(page);
            }
            if page_down {
                self.selected = (self.selected + page).min(last);
            }
            if self.selected != before {
                self.scroll_to_selected = true;
            }
        }

        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let button_side = ui.spacing().interact_size.y;
            let buttons_width = (button_side + ui.spacing().item_spacing.x) * 2.0 + 24.0;
            let te = egui::TextEdit::singleline(&mut self.needle)
                .desired_width((ui.available_width() - buttons_width).max(40.0))
                .hint_text("検索…（ext:png  size:>1mb  dm:today なども使えます）")
                .show(ui);
            let response = te.response;
            if response.secondary_clicked() {
                let _ = self.handle.show_context_menu();
            }
            if self.focus_search {
                response.request_focus();
                self.focus_search = false;
            }
            // 打つたびに絞り込む欄なので、確定を待たずに使う。Esc は search_escape のとおり
            let before_key = response.id.with("before_edit");
            if response.gained_focus() {
                ui.data_mut(|d| d.insert_temp(before_key, self.needle.clone()));
            }
            if response.lost_focus() {
                let before = ui.data_mut(|d| d.remove_temp::<String>(before_key));
                let (enter, escape) =
                    ui.input(|i| (i.key_pressed(egui::Key::Enter), i.key_pressed(egui::Key::Escape)));
                if enter {
                    self.insert_selected(results);
                    self.focus_search = true;
                } else if escape {
                    match search_escape(&self.needle, before.as_deref()) {
                        SearchEscape::Revert(text) => {
                            self.needle = text;
                            self.focus_search = true;
                        }
                        SearchEscape::Clear => {
                            self.needle.clear();
                            self.focus_search = true;
                        }
                        SearchEscape::Release => {}
                    }
                }
            }
            if ui
                .add_enabled(!self.scanner.is_scanning() && !self.config.roots.is_empty(), egui::Button::new("⟳"))
                .on_hover_text("フォルダを読み込み直す")
                .clicked()
            {
                self.results = None;
                self.scanner.start(&self.config, ui.ctx().clone());
            }
            if ui.button("⚙").on_hover_text("検索するフォルダと拡張子の設定").clicked() {
                self.open_settings();
            }
        });

        // 種類フィルタと並べ替え
        ui.horizontal(|ui| {
            let mut changed = false;
            for k in KindFilter::ALL {
                changed |= ui.selectable_value(&mut self.config.kind_filter, k, k.label()).changed();
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let relevance = self.config.sort == SortKey::Relevance;
                let arrow = if self.config.descending { "▼" } else { "▲" };
                let dir = ui
                    .add_enabled(!relevance, egui::Button::new(arrow))
                    .on_hover_text(if self.config.descending { "降順（クリックで昇順）" } else { "昇順（クリックで降順）" });
                if dir.clicked() {
                    self.config.descending = !self.config.descending;
                    changed = true;
                }
                let before = self.config.sort;
                egui::ComboBox::from_id_salt("sort")
                    .selected_text(self.config.sort.label())
                    .show_ui(ui, |ui| {
                        for s in SortKey::ALL {
                            ui.selectable_value(&mut self.config.sort, s, s.label());
                        }
                    })
                    .response
                    .on_hover_text("一致順: 検索語があれば一致の良い順、無ければフォルダ順");
                if self.config.sort != before {
                    self.config.descending = self.config.sort.default_descending();
                    changed = true;
                }
            });
            if changed {
                self.save_config();
            }
        });
        ui.add_space(2.0);
    }

    fn render_status(&mut self, ui: &mut egui::Ui, results: Option<&Results>) {
        ui.horizontal(|ui| {
            let index = self.scanner.index();
            let mut parts = Vec::new();
            if self.scanner.is_scanning() {
                parts.push(format!("読み込み中… {} 件", self.scanner.progress()));
            }
            if let (Some(index), Some(results)) = (&index, results) {
                if results.hits.len() == index.entries.len() {
                    parts.push(format!("全 {} 件（{:.1} 秒）", index.entries.len(), index.elapsed.as_secs_f32()));
                } else {
                    parts.push(format!("一致 {} 件 / 全 {} 件", results.hits.len(), index.entries.len()));
                }
                if index.truncated {
                    parts.push(format!("{} 件で打ち切り", crate::index::MAX_ENTRIES));
                }
                if !index.missing_roots.is_empty() {
                    parts.push(format!("見つからないフォルダ {} 件", index.missing_roots.len()));
                }
            }
            if self.watcher.is_some() {
                parts.push("自動更新".to_string());
            }
            ui.label(parts.join(" ・ "));
            let errors = results.map(|r| &r.errors).filter(|e| !e.is_empty());
            let (text, is_error) = match errors {
                Some(e) => (format!("解釈できない条件（無視しています）: {}", e.join(" ")), true),
                None => (self.status.clone(), self.status_is_error),
            };
            if !text.is_empty() {
                ui.separator();
                let color = if is_error { ui.visuals().error_fg_color } else { ui.visuals().text_color() };
                ui.add(egui::Label::new(egui::RichText::new(text).color(color)).truncate());
            }
        });
    }

    fn render_list(&mut self, ui: &mut egui::Ui, results: Option<Arc<Results>>) {
        if self.config.roots.is_empty() {
            ui.add_space(12.0);
            ui.label("検索するフォルダがまだありません。");
            ui.add_space(4.0);
            if ui.button("フォルダを追加…").clicked() {
                self.open_settings();
                self.dialog = Some(crate::folder_dialog::pick_folder_async(ui.ctx().clone(), self.own_hwnd()));
            }
            return;
        }
        let (Some(index), Some(results)) = (self.scanner.index(), results) else {
            ui.label("フォルダを読み込み中…");
            return;
        };
        let hits = &results.hits;
        if hits.is_empty() {
            ui.label(if index.entries.is_empty() {
                "対象のファイルがありません。拡張子の設定を確かめてください。"
            } else {
                "一致するファイルがありません。"
            });
            return;
        }
        self.selected = self.selected.min(hits.len() - 1);
        self.selected_path = Some(index.entries[hits[self.selected].entry].path.clone());

        let row_height = row_height(ui);
        let stride = row_stride(ui);
        let mut area = egui::ScrollArea::vertical().auto_shrink([false, false]);
        if self.scroll_to_selected {
            let top = self.selected as f32 * stride;
            let bottom = top + row_height;
            let mut offset = self.scroll_offset;
            if top < offset {
                offset = top;
            } else if bottom > offset + self.viewport_height {
                offset = bottom - self.viewport_height;
            }
            area = area.vertical_scroll_offset(offset.max(0.0));
            self.scroll_to_selected = false;
        }

        let mut clicked: Option<usize> = None;
        let mut dragged: Option<usize> = None;
        let history = &self.history;
        let selected = self.selected;
        let output = area.show_rows(ui, row_height, hits.len(), |ui, range| {
            for row in range {
                let hit = &hits[row];
                let entry = &index.entries[hit.entry];
                let response = render_row(ui, entry, hit, row == selected, row_height, history);
                if response.drag_started() {
                    dragged = Some(row);
                } else if response.clicked() {
                    clicked = Some(row);
                }
                response.context_menu(|ui| {
                    if ui.button("タイムラインに挿入").clicked() {
                        clicked = Some(row);
                        ui.close();
                    }
                    if ui.button("既定のアプリで開く").clicked() {
                        run_explorer(format!("\"{}\"", entry.path.display()));
                        ui.close();
                    }
                    if ui.button("エクスプローラーで表示").clicked() {
                        run_explorer(format!("/select,\"{}\"", entry.path.display()));
                        ui.close();
                    }
                    if ui.button("パスをコピー").clicked() {
                        ui.ctx().copy_text(entry.path.display().to_string());
                        ui.close();
                    }
                });
            }
        });
        self.scroll_offset = output.state.offset.y;
        self.viewport_height = output.inner_rect.height();

        if let Some(row) = clicked {
            self.selected = row;
            self.do_insert(&index.entries[hits[row].entry]);
        } else if let Some(row) = dragged {
            self.selected = row;
            self.do_drag(&index.entries[hits[row].entry]);
        }
    }
}

/// Esc で小さなウィンドウを閉じてよいか（ルール au2-rs-plugin「入力の確定と取り消し」。参照実装 `MidpointTable_H` の `busy_last_frame`）。
/// 閉じるのは、押す前にどこにもフォーカスが無く、ポップアップも開いていなかったときだけ
/// （入力中・並べ替えの一覧・右クリックのメニューの Esc は、そちらを取り消すだけにする）。
/// egui はフレームの始めに Esc でフォーカスを外しているので、前のフレームの終わりの状態で見る
#[derive(Debug, Default)]
struct EscDismiss {
    busy_last_frame: bool,
}

impl EscDismiss {
    /// フレームの始めに呼ぶ
    fn pressed(&self, ctx: &egui::Context) -> bool {
        ctx.input(|i| i.key_pressed(egui::Key::Escape)) && !self.busy_last_frame
    }

    /// フレームの終わり（すべて描いた後）に呼ぶ
    fn end_frame(&mut self, ctx: &egui::Context) {
        self.busy_last_frame = ctx.memory(|m| m.focused().is_some()) || ctx.any_popup_open();
    }
}

/// 検索欄で Esc を押したときにすること（ルール au2-rs-plugin「入力の確定と取り消し」）
#[derive(Debug, PartialEq)]
enum SearchEscape {
    /// 入力を始める前の文字に戻し、欄にフォーカスを戻す
    Revert(String),
    /// 空にして、欄にフォーカスを戻す（決まりより前からの動き。打ち直さずに全件へ戻れる）
    Clear,
    /// 何もしない。フォーカスは外れたままにして、キーを本体へ返す
    Release,
}

/// `before` は欄にフォーカスが入ったときの文字。
/// 入力していれば 1 回目の Esc で入力前に戻し、何も打っていなければ空にし、空なら欄から抜ける
fn search_escape(current: &str, before: Option<&str>) -> SearchEscape {
    match before {
        Some(b) if b != current => SearchEscape::Revert(b.to_string()),
        _ if !current.is_empty() => SearchEscape::Clear,
        _ => SearchEscape::Release,
    }
}

/// 1 行の入力欄の後に呼ぶ。入力中に Esc を押したら、入力を始める前の文字に戻す
/// （参照実装 `MidpointTable_H/src/gui.rs` の `number_text`）
fn revert_on_escape(ui: &egui::Ui, response: &egui::Response, text: &mut String) {
    let key = response.id.with("before_edit");
    if response.gained_focus() {
        ui.data_mut(|d| d.insert_temp(key, text.clone()));
    }
    if response.lost_focus() {
        let before = ui.data_mut(|d| d.remove_temp::<String>(key));
        if let (true, Some(b)) = (ui.input(|i| i.key_pressed(egui::Key::Escape)), before) {
            *text = b;
        }
    }
}

fn row_height(ui: &egui::Ui) -> f32 {
    ui.text_style_height(&egui::TextStyle::Body) + ui.text_style_height(&egui::TextStyle::Small) + 8.0
}

fn row_stride(ui: &egui::Ui) -> f32 {
    row_height(ui) + ui.spacing().item_spacing.y
}

fn kind_color(kind: Kind) -> egui::Color32 {
    match kind {
        Kind::Image => egui::Color32::from_rgb(52, 110, 190),
        Kind::Video => egui::Color32::from_rgb(150, 75, 185),
        Kind::Audio => egui::Color32::from_rgb(40, 140, 85),
        Kind::Alias => egui::Color32::from_rgb(190, 120, 35),
        Kind::Other => egui::Color32::from_gray(100),
    }
}

pub fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64 / 1024.0;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if value < 10.0 {
        format!("{value:.1} {}", UNITS[unit])
    } else {
        format!("{value:.0} {}", UNITS[unit])
    }
}

fn format_time(secs: i64) -> String {
    match Local.timestamp_opt(secs, 0).single() {
        Some(t) if secs > 0 => t.format("%Y-%m-%d %H:%M").to_string(),
        _ => "—".to_string(),
    }
}

fn render_row(
    ui: &mut egui::Ui,
    entry: &Entry,
    hit: &Hit,
    selected: bool,
    height: f32,
    history: &History,
) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), height), egui::Sense::click_and_drag());
    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_ui(|ui| {
        ui.label(entry.path.display().to_string());
        ui.label(format!(
            "{} ・ {} ・ 更新 {}",
            entry.kind.label(),
            format_size(entry.size),
            format_time(entry.modified)
        ));
        match history.get(&entry.path) {
            Some(r) => ui.label(format!("挿入 {} 回 ・ 最後 {}", r.count, format_time(r.last))),
            None => ui.label("まだ挿入していません"),
        };
        ui.weak("クリックで挿入 ・ ドラッグでタイムラインの好きな位置へ");
    });
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let visuals = ui.visuals().clone();
    let painter = ui.painter_at(rect);
    if selected {
        painter.rect_filled(rect, 3.0, visuals.selection.bg_fill.gamma_multiply(0.6));
    } else if response.hovered() {
        painter.rect_filled(rect, 3.0, visuals.widgets.hovered.weak_bg_fill);
    }

    // 種類のバッジ（拡張子）
    let badge = egui::Rect::from_min_size(
        rect.min + egui::vec2(4.0, (height - 18.0) / 2.0),
        egui::vec2(46.0, 18.0),
    );
    painter.rect_filled(badge, 3.0, kind_color(entry.kind));
    let ext: String = entry.ext.to_uppercase().chars().take(5).collect();
    painter.text(
        badge.center(),
        egui::Align2::CENTER_CENTER,
        if ext.is_empty() { "—".to_string() } else { ext },
        egui::FontId::proportional(11.0),
        egui::Color32::WHITE,
    );

    // 一致した文字の位置（元の文字列の文字単位）
    let (name_flags, dir_flags) = if hit.indices.is_empty() {
        (Vec::new(), Vec::new())
    } else if hit.by_name {
        (flags_for(&entry.name, &hit.indices), Vec::new())
    } else {
        let combined = format!("{}/{}", entry.dir, entry.name);
        let flags = flags_for(&combined, &hit.indices);
        let dir_len = entry.dir.chars().count();
        let dir_flags = flags[..dir_len.min(flags.len())].to_vec();
        let name_flags = flags.get(dir_len + 1..).map(<[bool]>::to_vec).unwrap_or_default();
        (name_flags, dir_flags)
    };

    let body = egui::TextStyle::Body.resolve(ui.style());
    let small = egui::TextStyle::Small.resolve(ui.style());
    let top = rect.top() + 4.0;

    // 右端にサイズ（1 行目）と更新日時（2 行目）。狭いときは出さない
    let meta_width = if rect.width() >= 360.0 { 118.0 } else { 0.0 };
    let body_height = ui.text_style_height(&egui::TextStyle::Body);
    if meta_width > 0.0 {
        let right = rect.right() - 6.0;
        painter.text(
            egui::pos2(right, top),
            egui::Align2::RIGHT_TOP,
            format_size(entry.size),
            small.clone(),
            visuals.weak_text_color(),
        );
        painter.text(
            egui::pos2(right, top + body_height),
            egui::Align2::RIGHT_TOP,
            format_time(entry.modified),
            small.clone(),
            visuals.weak_text_color(),
        );
    }

    let text_left = badge.right() + 8.0;
    let text_width = (rect.right() - text_left - 6.0 - meta_width).max(10.0);
    let highlight = visuals.hyperlink_color;
    let name_job = highlighted_job(&entry.name, &name_flags, body, visuals.text_color(), highlight, text_width);
    let dir_job = highlighted_job(&entry.dir, &dir_flags, small, visuals.weak_text_color(), highlight, text_width);
    let name_galley = painter.layout_job(name_job);
    let dir_galley = painter.layout_job(dir_job);
    painter.galley(egui::pos2(text_left, top), name_galley, visuals.text_color());
    painter.galley(egui::pos2(text_left, top + body_height), dir_galley, visuals.weak_text_color());
    response
}

/// 正規化後の位置 `indices` を、元の文字列の文字ごとの一致フラグに戻す
fn flags_for(text: &str, indices: &[u32]) -> Vec<bool> {
    let (_, map) = normalize_kana_for_search_with_map(text);
    let mut flags = vec![false; text.chars().count()];
    for &i in indices {
        if let Some(&(start, end)) = map.get(i as usize) {
            for f in flags.iter_mut().take(end + 1).skip(start) {
                *f = true;
            }
        }
    }
    flags
}

fn highlighted_job(
    text: &str,
    flags: &[bool],
    font: egui::FontId,
    color: egui::Color32,
    highlight: egui::Color32,
    width: f32,
) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    job.wrap = egui::text::TextWrapping::truncate_at_width(width);
    let chunks = text
        .chars()
        .enumerate()
        .chunk_by(|(i, _)| flags.get(*i).copied().unwrap_or(false));
    for (matched, chunk) in &chunks {
        let s: String = chunk.map(|(_, c)| c).collect();
        job.append(
            &s,
            0.0,
            egui::TextFormat {
                font_id: font.clone(),
                color: if matched { highlight } else { color },
                underline: if matched {
                    egui::Stroke::new(1.0, highlight)
                } else {
                    egui::Stroke::NONE
                },
                ..Default::default()
            },
        );
    }
    job
}

// ---- 設定 ----

impl FileFinderApp {
    fn open_settings(&mut self) {
        self.draft = self.config.clone();
        self.new_root_text.clear();
        self.clear_history_armed = None;
        self.show_settings = true;
    }

    fn poll_dialog(&mut self) {
        let Some(rx) = &self.dialog else { return };
        match rx.try_recv() {
            Ok(picked) => {
                self.dialog = None;
                if let Some(path) = picked {
                    push_root(&mut self.draft, path);
                }
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => self.dialog = None,
        }
    }

    fn render_settings(&mut self, ui: &mut egui::Ui) {
        if !self.show_settings {
            return;
        }
        let mut open = true;
        let mut apply = false;
        let mut cancel = false;
        // ドッキングした狭いパネルでも収まるように、高さを画面内に抑えて中身をスクロールさせる。
        // 中央固定（anchor）にすると動かせず、画面より高いと下のボタンに届かない
        let screen = ui.ctx().content_rect();
        let margin = 8.0;
        let footer_height = ui.spacing().interact_size.y + ui.spacing().item_spacing.y * 2.0 + 8.0;
        let title_height = ui.text_style_height(&egui::TextStyle::Heading) + 12.0;
        let body_max_height = (screen.height() - margin * 2.0 - title_height - footer_height).max(60.0);
        egui::Window::new("ファイル検索の設定")
            .collapsible(false)
            .resizable(true)
            .movable(true)
            .constrain(true)
            .open(&mut open)
            .pivot(egui::Align2::CENTER_CENTER)
            .default_pos(screen.center())
            .default_width((screen.width() - margin * 2.0).clamp(200.0, 420.0))
            .max_height(screen.height() - margin * 2.0)
            .show(ui.ctx(), |ui| {
                egui::ScrollArea::vertical()
                    .max_height(body_max_height)
                    .auto_shrink([false, true])
                    .show(ui, |ui| self.render_settings_body(ui));
                ui.separator();
                ui.horizontal(|ui| {
                    if ui.button("適用して読み込む").clicked() {
                        apply = true;
                    }
                    if ui.button("キャンセル").clicked() {
                        cancel = true;
                    }
                });
            });
        if apply {
            let draft = self.draft.clone();
            self.apply_settings(&draft, ui.ctx());
            self.show_settings = false;
            self.focus_search = true;
        } else if cancel || !open {
            self.show_settings = false;
        }
    }

    /// 設定画面のスクロールする部分
    fn render_settings_body(&mut self, ui: &mut egui::Ui) {
        ui.label("検索するフォルダ（サブフォルダも含めて読み込みます）");
        let mut remove = None;
        if self.draft.roots.is_empty() {
            ui.weak("（なし）");
        }
        for (i, root) in self.draft.roots.iter().enumerate() {
            ui.horizontal(|ui| {
                if ui.small_button("✕").on_hover_text("一覧から外す").clicked() {
                    remove = Some(i);
                }
                let text = root.display().to_string();
                let label = if root.is_dir() {
                    egui::RichText::new(text)
                } else {
                    egui::RichText::new(format!("{text}（見つかりません）")).color(ui.visuals().error_fg_color)
                };
                ui.add(egui::Label::new(label).truncate());
            });
        }
        if let Some(i) = remove {
            self.draft.roots.remove(i);
        }
        let picking = self.dialog.is_some();
        if ui
            .add_enabled(!picking, egui::Button::new(if picking { "選択中…" } else { "フォルダを追加…" }))
            .clicked()
        {
            self.dialog = Some(crate::folder_dialog::pick_folder_async(ui.ctx().clone(), self.own_hwnd()));
        }
        ui.horizontal(|ui| {
            let te = egui::TextEdit::singleline(&mut self.new_root_text)
                .hint_text("パスを貼り付けて追加")
                .desired_width((ui.available_width() - 60.0).max(60.0))
                .show(ui);
            // 使うのは Enter か「追加」を押したときだけ（ほかをクリックしただけでは足さない）
            revert_on_escape(ui, &te.response, &mut self.new_root_text);
            let entered = te.response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if (ui.button("追加").clicked() || entered) && !self.new_root_text.trim().is_empty() {
                let path = PathBuf::from(self.new_root_text.trim().trim_matches('"'));
                push_root(&mut self.draft, path);
                self.new_root_text.clear();
            }
        });

        ui.add_space(8.0);
        ui.checkbox(&mut self.draft.all_files, "すべてのファイルを対象にする（拡張子で絞らない）");
        ui.add_enabled_ui(!self.draft.all_files, |ui| {
            ui.label("対象の拡張子（空白かカンマ区切り。.object はエイリアスとして挿入）");
            // 複数行の欄なので Esc で戻さない（フォーカスを外すだけ）。使うのは「適用して読み込む」を押したとき
            ui.add(
                egui::TextEdit::multiline(&mut self.draft.extensions)
                    .desired_rows(2)
                    .desired_width(ui.available_width()),
            );
            if ui.small_button("既定に戻す").clicked() {
                self.draft.extensions = config::DEFAULT_EXTENSIONS.to_string();
            }
        });
        ui.checkbox(&mut self.draft.include_hidden, "隠しファイル・隠しフォルダも含める");
        ui.checkbox(&mut self.draft.auto_update, "フォルダの変更を自動で反映する");

        ui.add_space(8.0);
        let armed = self.clear_history_armed.is_some_and(|t| t.elapsed() < Duration::from_secs(3));
        let label = if armed {
            "もう一度押すと消去".to_string()
        } else {
            format!("挿入履歴を消去（{} 件）", self.history.len())
        };
        if ui.add_enabled(self.history.len() > 0, egui::Button::new(label)).clicked() {
            if armed {
                self.history.clear();
                self.history_gen += 1;
                if !self.save_history() {
                    self.set_status(
                        format!(
                            "挿入履歴のファイルを読めなかったので、この起動の間は保存しません（消去はこの起動の間だけ）: {}",
                            self.history_path.display()
                        ),
                        true,
                    );
                }
                self.clear_history_armed = None;
            } else {
                self.clear_history_armed = Some(Instant::now());
            }
        }
        if armed {
            ui.ctx().request_repaint_after(Duration::from_millis(500));
        }
    }
}

fn push_root(config: &mut Config, path: PathBuf) {
    if !config.roots.iter().any(|r| r == &path) {
        config.roots.push(path);
    }
}

#[cfg(test)]
mod tests {
    /// テストの 1 フレームの出力を捨てる。テクスチャの差分を片付けずに捨てると、デバッグビルドで epaint の debug_assert
    /// （Dropped TexturesDelta with N unapplied deltas）が落ちる（au2 release の prebuild の cargo test はデバッグビルド）
    fn discard_frame(mut out: egui::FullOutput) {
        out.textures_delta.clear();
    }

    use super::*;
    use crate::index::make_entry;

    fn entry(dir: &str, name: &str, size: u64, modified: i64) -> Entry {
        let ext = Path::new(name)
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        make_entry(PathBuf::from(format!("C:/{dir}/{name}")), name.into(), dir.into(), ext, size, modified)
    }

    fn index(entries: Vec<Entry>) -> Index {
        Index { entries, ..Index::default() }
    }

    fn matcher() -> nucleo_matcher::Matcher {
        nucleo_matcher::Matcher::new(nucleo_matcher::Config::DEFAULT.match_paths())
    }

    fn names(idx: &Index, r: &Results) -> Vec<String> {
        r.hits.iter().map(|h| idx.entries[h.entry].name.clone()).collect()
    }

    fn run(idx: &Index, needle: &str, kind: KindFilter, sort: SortKey, desc: bool, h: &History) -> Results {
        compute_results(&mut matcher(), idx, needle, kind, sort, desc, h)
    }

    #[test]
    fn name_matches_rank_before_path_matches() {
        let idx = index(vec![entry("素材/rain", "loop.wav", 0, 0), entry("素材/BGM", "rain_heavy.wav", 0, 0)]);
        let r = run(&idx, "rain", KindFilter::All, SortKey::Relevance, false, &History::default());
        assert_eq!(names(&idx, &r), vec!["rain_heavy.wav", "loop.wav"]);
        assert!(r.hits[0].by_name && !r.hits[1].by_name);
    }

    #[test]
    fn katakana_query_matches_hiragana_and_halfwidth_names() {
        let idx = index(vec![
            entry("x", "あめのおと.wav", 0, 0),
            entry("x", "ｱﾒ.png", 0, 0),
            entry("x", "晴れ.png", 0, 0),
        ]);
        let r = run(&idx, "アメ", KindFilter::All, SortKey::Relevance, false, &History::default());
        assert_eq!(r.hits.len(), 2);
        assert!(!names(&idx, &r).contains(&"晴れ.png".to_string()));
    }

    #[test]
    fn space_separated_terms_are_and_across_dir_and_name() {
        let idx = index(vec![entry("素材/BGM", "rain.wav", 0, 0), entry("素材/SE", "rain.wav", 0, 0)]);
        let r = run(&idx, "bgm rain", KindFilter::All, SortKey::Relevance, false, &History::default());
        assert_eq!(r.hits.len(), 1);
        assert_eq!(idx.entries[r.hits[0].entry].dir, "素材/BGM");
    }

    #[test]
    fn kind_filter_and_query_filters_combine() {
        let idx = index(vec![
            entry("a", "big.wav", 5 << 20, 0),
            entry("a", "small.wav", 1 << 10, 0),
            entry("a", "big.png", 5 << 20, 0),
        ]);
        let h = History::default();
        let r = run(&idx, "size:>1mb", KindFilter::Audio, SortKey::Name, false, &h);
        assert_eq!(names(&idx, &r), vec!["big.wav"]);
        // 検索語が条件だけでも、残りのあいまい検索は空として全件が対象
        let r = run(&idx, "ext:png", KindFilter::All, SortKey::Relevance, false, &h);
        assert_eq!(names(&idx, &r), vec!["big.png"]);
        let r = run(&idx, "dm:nope", KindFilter::All, SortKey::Relevance, false, &h);
        assert_eq!(r.hits.len(), 3);
        assert_eq!(r.errors, vec!["dm:nope"]);
    }

    #[test]
    fn sorts_by_size_and_date_in_both_directions() {
        let idx = index(vec![
            entry("a", "b.png", 300, 10),
            entry("a", "a.png", 100, 30),
            entry("a", "c.png", 200, 20),
        ]);
        let h = History::default();
        let size_desc = run(&idx, "", KindFilter::All, SortKey::Size, true, &h);
        assert_eq!(names(&idx, &size_desc), vec!["b.png", "c.png", "a.png"]);
        let date_desc = run(&idx, "", KindFilter::All, SortKey::Modified, true, &h);
        assert_eq!(names(&idx, &date_desc), vec!["a.png", "c.png", "b.png"]);
        let name_asc = run(&idx, "", KindFilter::All, SortKey::Name, false, &h);
        assert_eq!(names(&idx, &name_asc), vec!["a.png", "b.png", "c.png"]);
        // 検索語があっても、並べ替えを選んでいればそちらが優先
        let filtered = run(&idx, "png", KindFilter::All, SortKey::Size, false, &h);
        assert_eq!(names(&idx, &filtered), vec!["a.png", "c.png", "b.png"]);
    }

    #[test]
    fn history_sorts_keep_never_inserted_last() {
        let idx = index(vec![
            entry("a", "never.png", 0, 0),
            entry("a", "once.png", 0, 0),
            entry("a", "twice.png", 0, 0),
        ]);
        let mut h = History::default();
        h.bump(Path::new("C:/a/twice.png"), 100);
        h.bump(Path::new("C:/a/twice.png"), 101);
        h.bump(Path::new("C:/a/once.png"), 200);
        let count = run(&idx, "", KindFilter::All, SortKey::InsertCount, true, &h);
        assert_eq!(names(&idx, &count), vec!["twice.png", "once.png", "never.png"]);
        let recent = run(&idx, "", KindFilter::All, SortKey::RecentlyInserted, true, &h);
        assert_eq!(names(&idx, &recent), vec!["once.png", "twice.png", "never.png"]);
        // 昇順にしても挿入していないものは後ろのまま
        let recent_asc = run(&idx, "", KindFilter::All, SortKey::RecentlyInserted, false, &h);
        assert_eq!(names(&idx, &recent_asc), vec!["twice.png", "once.png", "never.png"]);
    }

    #[test]
    fn size_format() {
        assert_eq!(format_size(512), "512 B");
        assert_eq!(format_size(1536), "1.5 KB");
        assert_eq!(format_size(20 << 20), "20 MB");
        assert_eq!(format_size(3 << 30), "3.0 GB");
    }

    #[test]
    fn search_escape_reverts_then_clears_then_releases() {
        // 入力前 "rain" に " bgm" を打った → 1 回目の Esc で "rain" に戻す
        assert_eq!(search_escape("rain bgm", Some("rain")), SearchEscape::Revert("rain".into()));
        // 戻した後（フォーカスを入れ直したので入力前も "rain"）→ 空にする
        assert_eq!(search_escape("rain", Some("rain")), SearchEscape::Clear);
        // 空 → 欄から抜ける
        assert_eq!(search_escape("", Some("")), SearchEscape::Release);
        // 空の欄に打ってから Esc → 空に戻す（従来の「空にする」と同じ結果）
        assert_eq!(search_escape("abc", Some("")), SearchEscape::Revert(String::new()));
        // 入力前の文字を覚えていない（フォーカスが入ったフレームを通っていない）ときは従来どおり
        assert_eq!(search_escape("abc", None), SearchEscape::Clear);
        assert_eq!(search_escape("", None), SearchEscape::Release);
    }

    // ---- egui だけで動かす ----

    fn key(key: egui::Key) -> egui::Event {
        egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE }
    }

    fn input(events: Vec<egui::Event>) -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 300.0))),
            events,
            ..Default::default()
        }
    }

    /// 1 行の欄を 1 フレーム描く。`focus` なら描いた後にフォーカスを入れる（`focus_search` と同じ順）
    fn text_frame(ctx: &egui::Context, text: &mut String, focus: bool, events: Vec<egui::Event>) {
        discard_frame(ctx.run_ui(input(events), |ui| {
            let response = ui.add(egui::TextEdit::singleline(text));
            if focus {
                response.request_focus();
            }
            revert_on_escape(ui, &response, text);
        }));
    }

    #[test]
    fn escape_restores_text_before_editing() {
        let ctx = egui::Context::default();
        let mut text = String::from("C:/素材");
        text_frame(&ctx, &mut text, true, vec![]);
        text_frame(&ctx, &mut text, false, vec![egui::Event::Text("/BGM".into())]);
        assert_eq!(text, "C:/素材/BGM");
        text_frame(&ctx, &mut text, false, vec![key(egui::Key::Escape)]);
        assert_eq!(text, "C:/素材");
        // フォーカスを失った扱いが続くフレームでも、そのまま
        text_frame(&ctx, &mut text, false, vec![]);
        text_frame(&ctx, &mut text, false, vec![]);
        assert_eq!(text, "C:/素材");
    }

    #[test]
    fn enter_keeps_typed_text() {
        let ctx = egui::Context::default();
        let mut text = String::new();
        text_frame(&ctx, &mut text, true, vec![]);
        text_frame(&ctx, &mut text, false, vec![egui::Event::Text("D:/x".into())]);
        text_frame(&ctx, &mut text, false, vec![key(egui::Key::Enter)]);
        text_frame(&ctx, &mut text, false, vec![key(egui::Key::Escape)]);
        assert_eq!(text, "D:/x");
    }

    #[test]
    fn escape_closes_only_when_nothing_was_focused() {
        let ctx = egui::Context::default();
        let mut dismiss = EscDismiss::default();
        let mut text = String::new();
        let mut frame = |focus: bool, events: Vec<egui::Event>, dismiss: &mut EscDismiss| {
            let mut closes = false;
            discard_frame(ctx.run_ui(input(events), |ui| {
                closes = dismiss.pressed(ui.ctx());
                let response = ui.add(egui::TextEdit::singleline(&mut text));
                if focus {
                    response.request_focus();
                }
                dismiss.end_frame(ui.ctx());
            }));
            closes
        };
        assert!(!frame(true, vec![], &mut dismiss));
        // 入力中の Esc はフォーカスを外すだけ（egui はこのフレームの始めにフォーカスを外している）
        assert!(!frame(false, vec![key(egui::Key::Escape)], &mut dismiss));
        // どこにもフォーカスが無いときの Esc で閉じる
        assert!(frame(false, vec![key(egui::Key::Escape)], &mut dismiss));
    }

    #[test]
    fn flags_cover_both_chars_of_halfwidth_dakuten() {
        // "ｶﾞa" は正規化で "がa"。正規化後の 0 文字目は元の 0〜1 文字目
        assert_eq!(flags_for("ｶﾞa", &[0]), vec![true, true, false]);
    }
}
