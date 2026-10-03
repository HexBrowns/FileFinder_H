//! フォルダの監視（Everything のリアルタイム更新に当たる）
//!
//! 変更を受けたらすぐには読み込み直さず、変更が QUIET だけ止んだとき（書き出し中の動画のように
//! 書き込みが続く間は待つ）か、最初の変更から MAX_WAIT 経ったときに 1 回だけ読み込み直す。
//! 読み込み直しは全体の走査（差分の更新はしない）。

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use aviutl2::tracing;
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use parking_lot::Mutex;

use crate::config::Config;

pub const QUIET: Duration = Duration::from_millis(1500);
pub const MAX_WAIT: Duration = Duration::from_secs(20);

#[derive(Debug, Clone, Copy, Default)]
struct Pending {
    first: Option<Instant>,
    last: Option<Instant>,
}

/// 保留中の変更を、いま読み込み直すべきか
fn should_fire(first: Instant, last: Instant, now: Instant) -> bool {
    now.duration_since(last) >= QUIET || now.duration_since(first) >= MAX_WAIT
}

pub struct FolderWatcher {
    _watchers: Vec<RecommendedWatcher>,
    stop: Arc<AtomicBool>,
    /// 監視を始められなかったフォルダ（ネットワークドライブなど）
    pub failed: Vec<String>,
}

impl Drop for FolderWatcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

impl FolderWatcher {
    /// `config.roots` を監視し、読み込み直すべきときに `on_change` を呼ぶ（別スレッドから）
    pub fn start(config: &Config, on_change: impl Fn() + Send + 'static) -> Self {
        let pending = Arc::new(Mutex::new(Pending::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let extensions: Arc<HashSet<String>> =
            Arc::new(config.extension_list().into_iter().collect());
        let all_files = config.all_files;

        let mut watchers = Vec::new();
        let mut failed = Vec::new();
        for root in &config.roots {
            let pending = Arc::clone(&pending);
            let extensions = Arc::clone(&extensions);
            let handler = move |res: notify::Result<notify::Event>| {
                let Ok(event) = res else {
                    // 取りこぼし（バッファあふれ等）は、分からないので読み込み直す
                    mark(&pending);
                    return;
                };
                if matches!(event.kind, EventKind::Access(_)) {
                    return;
                }
                if all_files
                    || event.need_rescan()
                    || event.paths.iter().any(|p| relevant(p, &extensions))
                {
                    mark(&pending);
                }
            };
            let result = notify::recommended_watcher(handler)
                .and_then(|mut w| w.watch(root, RecursiveMode::Recursive).map(|()| w));
            match result {
                Ok(w) => watchers.push(w),
                Err(e) => {
                    tracing::warn!("FileFinder_H: {} を監視できません: {e}", root.display());
                    failed.push(root.display().to_string());
                }
            }
        }

        if !watchers.is_empty() {
            let stop = Arc::clone(&stop);
            let pending = Arc::clone(&pending);
            let spawned = crate::shutdown::spawn("FileFinder_H watch", move || {
                while !stop.load(Ordering::SeqCst) && !crate::shutdown::is_shutting_down() {
                    std::thread::sleep(Duration::from_millis(250));
                    let fire = {
                        let mut p = pending.lock();
                        match (p.first, p.last) {
                            (Some(first), Some(last))
                                if should_fire(first, last, Instant::now()) =>
                            {
                                *p = Pending::default();
                                true
                            }
                            _ => false,
                        }
                    };
                    if fire && !stop.load(Ordering::SeqCst) && !crate::shutdown::is_shutting_down()
                    {
                        on_change();
                    }
                }
            });
            if let Err(e) = spawned {
                tracing::error!("FileFinder_H: 監視スレッドを起動できませんでした: {e}");
            }
        }

        Self {
            _watchers: watchers,
            stop,
            failed,
        }
    }
}

fn mark(pending: &Mutex<Pending>) {
    let now = Instant::now();
    let mut p = pending.lock();
    p.first.get_or_insert(now);
    p.last = Some(now);
}

/// 一覧に関係しうる変更か。対象外の拡張子のファイルだけの変更（一時ファイル等）は無視する。
/// 拡張子の無いパスはフォルダかもしれないので関係ありとみなす
fn relevant(path: &Path, extensions: &HashSet<String>) -> bool {
    match path.extension() {
        Some(ext) => extensions.contains(&ext.to_string_lossy().to_lowercase()),
        None => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waits_for_quiet_but_not_forever() {
        let t0 = Instant::now();
        // 変更の直後は待つ
        assert!(!should_fire(t0, t0, t0 + Duration::from_millis(500)));
        // 止んで QUIET 経てば読み込み直す
        assert!(should_fire(t0, t0, t0 + QUIET));
        // 書き込みが続いていても MAX_WAIT で読み込み直す
        let last = t0 + MAX_WAIT - Duration::from_millis(100);
        assert!(!should_fire(
            t0,
            last,
            t0 + MAX_WAIT - Duration::from_millis(1)
        ));
        assert!(should_fire(t0, last, t0 + MAX_WAIT));
    }

    #[test]
    fn ignores_unrelated_extensions() {
        let exts: HashSet<String> = ["png".to_string()].into_iter().collect();
        assert!(relevant(Path::new("C:/a/B.PNG"), &exts));
        assert!(!relevant(Path::new("C:/a/b.tmp"), &exts));
        assert!(relevant(Path::new("C:/a/新しいフォルダ"), &exts));
    }

    #[test]
    fn detects_new_file_in_real_folder() {
        let dir = std::env::temp_dir().join(format!("file_finder_h_watch_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let fired = Arc::new(AtomicBool::new(false));
        let config = Config {
            roots: vec![dir.clone()],
            ..Config::default()
        };
        let f = Arc::clone(&fired);
        let watcher = FolderWatcher::start(&config, move || f.store(true, Ordering::SeqCst));
        assert!(watcher.failed.is_empty());
        std::thread::sleep(Duration::from_millis(200));
        std::fs::write(dir.join("new.png"), b"x").unwrap();
        let deadline = Instant::now() + QUIET + Duration::from_secs(3);
        while !fired.load(Ordering::SeqCst) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(100));
        }
        drop(watcher);
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(
            fired.load(Ordering::SeqCst),
            "新しいファイルを検知できなかった"
        );
    }
}
