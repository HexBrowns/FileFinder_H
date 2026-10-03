//! ファイル一覧の作成（別スレッドで走査する）

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use aviutl2::tracing;
use aviutl2_eframe::egui;
use nucleo_matcher::Utf32String;
use parking_lot::Mutex;

use crate::config::Config;
use crate::kana::normalize_kana_for_search;

/// これを超えたら走査を打ち切る（ドライブ直下を指定されたときの保険）
pub const MAX_ENTRIES: usize = 500_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Image,
    Video,
    Audio,
    /// オブジェクトエイリアス（.object）。エイリアスとして挿入する
    Alias,
    Other,
}

impl Kind {
    pub fn from_ext(ext: &str) -> Self {
        match ext {
            "png" | "jpg" | "jpeg" | "bmp" | "gif" | "webp" | "tif" | "tiff" | "tga" | "dds"
            | "psd" | "avif" | "heic" | "jxl" | "svg" => Kind::Image,
            "mp4" | "mov" | "mkv" | "avi" | "webm" | "wmv" | "m4v" | "mpg" | "mpeg" | "ts"
            | "m2ts" | "flv" | "3gp" => Kind::Video,
            "wav" | "mp3" | "ogg" | "flac" | "m4a" | "aac" | "opus" | "wma" | "aiff" | "aif" => {
                Kind::Audio
            }
            "object" => Kind::Alias,
            _ => Kind::Other,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Kind::Image => "画像",
            Kind::Video => "動画",
            Kind::Audio => "音声",
            Kind::Alias => "エイリアス",
            Kind::Other => "その他",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub path: PathBuf,
    /// ファイル名
    pub name: String,
    /// 表示用のフォルダ（`ルートのフォルダ名/サブ/フォルダ`）。区切りは `/`
    pub dir: String,
    /// 小文字の拡張子
    pub ext: String,
    pub kind: Kind,
    /// バイト数
    pub size: u64,
    /// 更新日時（UNIX 秒）。取れなければ 0
    pub modified: i64,
    pub search_name: Utf32String,
    /// `dir/name` を正規化したもの
    pub search_path: Utf32String,
    /// 並べ替え用（小文字）
    pub sort_name: String,
    pub sort_path: String,
}

#[derive(Debug, Default)]
pub struct Index {
    pub entries: Vec<Entry>,
    pub truncated: bool,
    pub missing_roots: Vec<PathBuf>,
    pub unreadable_dirs: usize,
    pub elapsed: Duration,
}

#[derive(Default)]
struct Shared {
    index: Option<Arc<Index>>,
    scanning: bool,
}

/// 走査の状態。UI スレッドから `start` し、`index()` で結果を読む。
#[derive(Clone, Default)]
pub struct Scanner {
    shared: Arc<Mutex<Shared>>,
    /// 走査の世代。新しい `start` で古い走査を打ち切る
    generation: Arc<AtomicU64>,
    progress: Arc<AtomicUsize>,
}

impl Scanner {
    pub fn index(&self) -> Option<Arc<Index>> {
        self.shared.lock().index.clone()
    }

    pub fn is_scanning(&self) -> bool {
        self.shared.lock().scanning
    }

    pub fn progress(&self) -> usize {
        self.progress.load(Ordering::Relaxed)
    }

    /// 走査を始める（前の結果は新しい結果が出るまで残す）
    pub fn start(&self, config: &Config, ctx: egui::Context) {
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        self.progress.store(0, Ordering::Relaxed);
        self.shared.lock().scanning = true;
        let config = config.clone();
        let this = self.clone();
        let spawned = crate::shutdown::spawn("FileFinder_H scan", move || {
            let cancelled = || {
                this.generation.load(Ordering::SeqCst) != generation
                    || crate::shutdown::is_shutting_down()
            };
            let index = scan(&config, &this.progress, &cancelled, &ctx);
            if cancelled() {
                return;
            }
            tracing::info!(
                "FileFinder_H: {} 件を {:.2} 秒で読み込みました",
                index.entries.len(),
                index.elapsed.as_secs_f32()
            );
            let mut s = this.shared.lock();
            s.index = Some(Arc::new(index));
            s.scanning = false;
            drop(s);
            ctx.request_repaint();
        });
        if let Err(e) = spawned {
            tracing::error!("FileFinder_H: 走査スレッドを起動できませんでした: {e}");
            self.shared.lock().scanning = false;
        }
    }
}

fn scan(
    config: &Config,
    progress: &AtomicUsize,
    cancelled: &dyn Fn() -> bool,
    ctx: &egui::Context,
) -> Index {
    let started = Instant::now();
    let extensions: HashSet<String> = config.extension_list().into_iter().collect();
    let mut index = Index::default();
    let mut visited: HashSet<PathBuf> = HashSet::new();
    let mut last_repaint = Instant::now();

    'roots: for root in &config.roots {
        if !root.is_dir() {
            index.missing_roots.push(root.clone());
            continue;
        }
        let root_label = root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| {
                root.to_string_lossy()
                    .trim_end_matches(['\\', '/'])
                    .to_string()
            });
        let mut stack: Vec<(PathBuf, String)> = vec![(root.clone(), root_label)];

        while let Some((dir, rel)) = stack.pop() {
            if cancelled() {
                break 'roots;
            }
            // シンボリックリンク・ジャンクションの循環を避ける（実体のパスで一度だけ読む）
            if let Ok(real) = std::fs::canonicalize(&dir)
                && !visited.insert(real)
            {
                continue;
            }
            let Ok(read) = std::fs::read_dir(&dir) else {
                index.unreadable_dirs += 1;
                continue;
            };
            let mut subdirs = Vec::new();
            for item in read.flatten() {
                let name = item.file_name().to_string_lossy().into_owned();
                // Windows では DirEntry::metadata() は列挙時の情報から作られる（追加の読み込みは無い）
                let meta = item.metadata().ok();
                if !config.include_hidden && is_hidden(meta.as_ref(), &name) {
                    continue;
                }
                let path = item.path();
                // file_type() は列挙時の情報で済む。リンクのときだけ辿って実体を見る
                let is_dir = match item.file_type() {
                    Ok(t) if t.is_symlink() => path.is_dir(),
                    Ok(t) => t.is_dir(),
                    Err(_) => false,
                };
                if is_dir {
                    subdirs.push((path, format!("{rel}/{name}")));
                    continue;
                }
                let ext = Path::new(&name)
                    .extension()
                    .map(|e| e.to_string_lossy().to_lowercase())
                    .unwrap_or_default();
                if !config.all_files && !extensions.contains(&ext) {
                    continue;
                }
                let size = meta.as_ref().map_or(0, |m| m.len());
                let modified = meta.as_ref().map_or(0, |m| unix_seconds(m.modified().ok()));
                index
                    .entries
                    .push(make_entry(path, name, rel.clone(), ext, size, modified));
                let n = index.entries.len();
                if n % 1000 == 0 {
                    progress.store(n, Ordering::Relaxed);
                    if last_repaint.elapsed() > Duration::from_millis(200) {
                        ctx.request_repaint();
                        last_repaint = Instant::now();
                    }
                }
                if n >= MAX_ENTRIES {
                    index.truncated = true;
                    break 'roots;
                }
            }
            // 名前順に見えるよう、逆順に積む（スタックなので後に積んだものから読まれる）
            subdirs.sort_by(|a, b| b.1.cmp(&a.1));
            stack.extend(subdirs);
        }
    }
    progress.store(index.entries.len(), Ordering::Relaxed);
    index.elapsed = started.elapsed();
    index
}

pub fn make_entry(
    path: PathBuf,
    name: String,
    dir: String,
    ext: String,
    size: u64,
    modified: i64,
) -> Entry {
    let full = format!("{dir}/{name}");
    let search_name = Utf32String::from(normalize_kana_for_search(&name).as_str());
    let search_path = Utf32String::from(normalize_kana_for_search(&full).as_str());
    Entry {
        kind: Kind::from_ext(&ext),
        sort_name: name.to_lowercase(),
        sort_path: full.to_lowercase(),
        path,
        name,
        dir,
        ext,
        size,
        modified,
        search_name,
        search_path,
    }
}

fn unix_seconds(t: Option<std::time::SystemTime>) -> i64 {
    t.and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs() as i64)
}

fn is_hidden(meta: Option<&std::fs::Metadata>, name: &str) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
    const FILE_ATTRIBUTE_SYSTEM: u32 = 0x4;
    if name.starts_with('.') {
        return true;
    }
    meta.is_some_and(|m| m.file_attributes() & (FILE_ATTRIBUTE_HIDDEN | FILE_ATTRIBUTE_SYSTEM) != 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_tree() -> PathBuf {
        let base = std::env::temp_dir().join(format!(
            "file_finder_h_test_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(base.join("素材").join("BGM")).unwrap();
        std::fs::create_dir_all(base.join("素材").join(".cache")).unwrap();
        std::fs::write(base.join("素材").join("背景.PNG"), b"12345").unwrap();
        std::fs::write(base.join("素材").join("BGM").join("雨.wav"), b"").unwrap();
        std::fs::write(base.join("素材").join("メモ.txt"), b"").unwrap();
        std::fs::write(base.join("素材").join(".cache").join("x.png"), b"").unwrap();
        base
    }

    #[test]
    fn scans_recursively_with_extension_filter_and_hidden_skip() {
        let base = temp_tree();
        let config = Config {
            roots: vec![base.join("素材"), base.join("存在しない")],
            ..Config::default()
        };
        let index = scan(
            &config,
            &AtomicUsize::new(0),
            &|| false,
            &egui::Context::default(),
        );
        let mut got: Vec<_> = index
            .entries
            .iter()
            .map(|e| format!("{}/{}", e.dir, e.name))
            .collect();
        got.sort();
        assert_eq!(got, vec!["素材/BGM/雨.wav", "素材/背景.PNG"]);
        assert_eq!(index.missing_roots.len(), 1);
        let png = index.entries.iter().find(|e| e.name == "背景.PNG").unwrap();
        assert_eq!(png.ext, "png");
        assert_eq!(png.kind, Kind::Image);
        assert_eq!(png.size, 5);
        let now = unix_seconds(Some(std::time::SystemTime::now()));
        assert!(
            (now - png.modified).abs() < 60,
            "更新日時が取れていない: {}",
            png.modified
        );
        assert_eq!(png.sort_path, "素材/背景.png");

        let all = Config {
            all_files: true,
            include_hidden: true,
            ..config
        };
        let index = scan(
            &all,
            &AtomicUsize::new(0),
            &|| false,
            &egui::Context::default(),
        );
        assert_eq!(index.entries.len(), 4);
        std::fs::remove_dir_all(&base).unwrap();
    }
}
