//! 設定（検索するフォルダと拡張子、一覧の表示のしかた）。`Plugin/FileFinder_H/config.json` に保存する。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::index::Kind;

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
    pub kind_filter: KindFilter,
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

/// 読み込む。無ければ既定値。壊れていたら退避してから既定値で始める（黙って上書きしない）。
pub fn load(path: &Path) -> (Config, Option<String>) {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (Config::default(), None),
        Err(e) => return (Config::default(), Some(format!("設定を読めませんでした: {e}"))),
    };
    match serde_json::from_str::<Config>(&text) {
        Ok(c) => (c, None),
        Err(e) => {
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let broken = path.with_extension(format!("json.broken-{stamp}"));
            let moved = std::fs::rename(path, &broken).is_ok();
            let note = if moved {
                format!("設定が壊れていたので {} へ退避しました: {e}", broken.display())
            } else {
                format!("設定が壊れています（退避にも失敗）: {e}")
            };
            (Config::default(), Some(note))
        }
    }
}

pub fn save(path: &Path, config: &Config) -> anyhow::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let json = serde_json::to_string_pretty(config)?;
    // 書きかけで落ちても元の設定が残るように、一時ファイルに書いてから置き換える
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
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
}
