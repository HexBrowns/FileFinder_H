//! 設定（検索するフォルダと拡張子、一覧の表示のしかた）。`Plugin/FileFinder_H/config.json` に保存する。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::index::Kind;
use crate::store::{self, Loaded};

pub const DEFAULT_EXTENSIONS: &str = "png jpg jpeg bmp gif webp tif tiff \
     mp4 mov mkv avi webm wmv m4v mpg mpeg ts flv \
     wav mp3 ogg flac m4a aac opus wma aiff \
     object";

/// 種類で絞る（Everything の「フィルタ」に当たる）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum KindFilter {
    #[default]
    All,
    Image,
    Video,
    Audio,
    Alias,
}

impl KindFilter {
    pub const ALL: [KindFilter; 5] = [
        KindFilter::All,
        KindFilter::Image,
        KindFilter::Video,
        KindFilter::Audio,
        KindFilter::Alias,
    ];

    pub fn label(self) -> &'static str {
        match self {
            KindFilter::All => "すべて",
            KindFilter::Image => "画像",
            KindFilter::Video => "動画",
            KindFilter::Audio => "音声",
            KindFilter::Alias => "エイリアス",
        }
    }

    pub fn matches(self, kind: Kind) -> bool {
        match self {
            KindFilter::All => true,
            KindFilter::Image => kind == Kind::Image,
            KindFilter::Video => kind == Kind::Video,
            KindFilter::Audio => kind == Kind::Audio,
            KindFilter::Alias => kind == Kind::Alias,
        }
    }
}

/// 並べ替えの基準
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SortKey {
    /// 検索語があれば一致の良い順、無ければ読み込んだ順（フォルダ順）
    #[default]
    Relevance,
    Name,
    Path,
    Size,
    Modified,
    /// 最後に挿入した時刻（Everything の「最終実行日時」）
    RecentlyInserted,
    /// 挿入した回数（Everything の「実行回数」）
    InsertCount,
}

impl SortKey {
    pub const ALL: [SortKey; 7] = [
        SortKey::Relevance,
        SortKey::Name,
        SortKey::Path,
        SortKey::Size,
        SortKey::Modified,
        SortKey::RecentlyInserted,
        SortKey::InsertCount,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SortKey::Relevance => "一致順",
            SortKey::Name => "名前",
            SortKey::Path => "パス",
            SortKey::Size => "サイズ",
            SortKey::Modified => "更新日時",
            SortKey::RecentlyInserted => "最近挿入した順",
            SortKey::InsertCount => "挿入回数",
        }
    }

    /// 選んだときの向きの既定（大きい・新しいものを先に見たい基準は降順）
    pub fn default_descending(self) -> bool {
        matches!(
            self,
            SortKey::Size | SortKey::Modified | SortKey::RecentlyInserted | SortKey::InsertCount
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// 検索するフォルダ（それぞれ再帰的に走査する）
    pub roots: Vec<PathBuf>,
    /// 対象の拡張子。空白・カンマ区切り、ドット無し、大文字小文字は区別しない
    pub extensions: String,
    /// true なら拡張子で絞らない
    pub all_files: bool,
    /// true なら隠しファイル・隠しフォルダ（属性 or 先頭が `.`）も含める
    pub include_hidden: bool,
    /// true ならフォルダの変更を監視して読み込み直す
    pub auto_update: bool,
    /// 値が増えうる列挙は、知らない値（新しい版が書いたもの）を初期値に読み替える。ファイル全体を捨てないため
    #[serde(deserialize_with = "crate::store::lenient")]
    pub kind_filter: KindFilter,
    #[serde(deserialize_with = "crate::store::lenient")]
    pub sort: SortKey,
    pub descending: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            roots: Vec::new(),
            extensions: DEFAULT_EXTENSIONS.to_string(),
            all_files: false,
            include_hidden: false,
            auto_update: true,
            kind_filter: KindFilter::All,
            sort: SortKey::Relevance,
            descending: false,
        }
    }
}

impl Config {
    /// 小文字・ドット無しの拡張子一覧
    pub fn extension_list(&self) -> Vec<String> {
        self.extensions
            .split(|c: char| c.is_whitespace() || c == ',' || c == ';')
            .map(|s| s.trim().trim_start_matches('.').to_lowercase())
            .filter(|s| !s.is_empty())
            .collect()
    }
}

pub fn default_path() -> PathBuf {
    aviutl2::config::app_data_path()
        .join("Plugin")
        .join("FileFinder_H")
        .join("config.json")
}

/// 読み込む。無ければ既定値。読めなければ退避してから既定値で始め、退避もできなければ保存しない（`store.rs`）
pub fn load(path: &Path) -> Loaded<Config> {
    store::load_json(path, "設定")
}

/// 一時ファイルに書いてから置き換える
pub fn save(path: &Path, config: &Config) -> anyhow::Result<()> {
    store::save_text(path, &serde_json::to_string_pretty(config)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extension_list_accepts_mixed_separators() {
        let c = Config {
            extensions: " .PNG, jpg;wav  object ".into(),
            ..Config::default()
        };
        assert_eq!(c.extension_list(), vec!["png", "jpg", "wav", "object"]);
    }

    #[test]
    fn missing_fields_fall_back_to_default() {
        let c: Config = serde_json::from_str(r#"{"roots":["C:/x"]}"#).unwrap();
        assert_eq!(c.extensions, DEFAULT_EXTENSIONS);
        assert!(!c.all_files);
        // v0.1.0 の設定ファイルには無い項目
        assert!(c.auto_update);
        assert_eq!(c.sort, SortKey::Relevance);
        assert_eq!(c.kind_filter, KindFilter::All);
    }

    /// v0.1.0 の設定ファイル（並べ替え・種類・自動更新が無い）を、項目を落とさずに読む
    #[test]
    fn reads_old_file_without_newer_fields() {
        let old = r#"{
  "roots": ["D:/素材", "E:/SE"],
  "extensions": "png wav",
  "all_files": true,
  "include_hidden": true
}"#;
        let c: Config = serde_json::from_str(old).unwrap();
        assert_eq!(c.roots, vec![PathBuf::from("D:/素材"), PathBuf::from("E:/SE")]);
        assert_eq!(c.extensions, "png wav");
        assert!(c.all_files && c.include_hidden && c.auto_update);
        assert_eq!((c.kind_filter, c.sort, c.descending), (KindFilter::All, SortKey::Relevance, false));
    }

    /// 新しい版が足した列挙の値・知らない項目があっても、ほかの項目（検索するフォルダ）は残す
    #[test]
    fn unknown_enum_values_fall_back_without_dropping_file() {
        let newer = r#"{
  "roots": ["D:/素材"],
  "extensions": "png",
  "kind_filter": "Font",
  "sort": "Duration",
  "descending": true,
  "future_option": 1
}"#;
        let c: Config = serde_json::from_str(newer).unwrap();
        assert_eq!(c.roots, vec![PathBuf::from("D:/素材")]);
        assert_eq!(c.extensions, "png");
        assert_eq!(c.kind_filter, KindFilter::All);
        assert_eq!(c.sort, SortKey::Relevance);
        assert!(c.descending);

        // 知っている値はそのまま読む
        let c: Config = serde_json::from_str(r#"{"kind_filter":"Audio","sort":"InsertCount"}"#).unwrap();
        assert_eq!((c.kind_filter, c.sort), (KindFilter::Audio, SortKey::InsertCount));
    }

    #[test]
    fn roundtrips_all_enum_values() {
        for kind_filter in KindFilter::ALL {
            for sort in SortKey::ALL {
                let c = Config { kind_filter, sort, ..Config::default() };
                let back: Config = serde_json::from_str(&serde_json::to_string_pretty(&c).unwrap()).unwrap();
                assert_eq!(back, c);
            }
        }
    }

    /// 読めないファイルは退避して既定値。退避した中身は元のまま
    #[test]
    fn load_moves_broken_file_aside_and_save_writes_back() {
        let dir = crate::store::tests::temp_dir("config");
        let p = dir.join("config.json");
        std::fs::write(&p, "{\"roots\": [").unwrap();
        let r = load(&p);
        assert_eq!(r.value, Config::default());
        assert!(r.writable);
        assert!(!p.exists());
        assert_eq!(crate::store::tests::names_starting_with(&dir, "config.json.broken-").len(), 1);

        let c = Config { roots: vec![PathBuf::from("D:/素材")], sort: SortKey::Size, ..Config::default() };
        save(&p, &c).unwrap();
        let r = load(&p);
        assert_eq!(r.value, c);
        assert!(r.warning.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
