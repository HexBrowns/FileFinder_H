//! プラグインが外されるときに、裏で動くスレッドを止める。
//!
//! 本体は終了時に各プラグインの DLL を外す（FreeLibrary）。0.2.0 までは、フォルダの監視（250ms ごとに起きる）や
//! 走査のスレッドが「止める印」を立てられるだけで終わりを待たれず、DLL が外れた後に目を覚まして、消えたコードを
//! 実行してアクセス違反で落ちていた。落ちるのは本体が `aviutl2.ini` を書く前後なので、ウィンドウの配置などの
//! 設定が保存されなかった（2026-09-27、ProcDump のダンプで違反の位置が解放済みの FileFinder_H.aux2 の中だった。
//! `AI/plugins/FileFinder_H/issues/20260927_background_thread_outlives_unload.md`）。
//!
//! 対策は二段:
//! 1. `shutdown()`（プラグインの Drop から呼ぶ）で停止の印を立て、`spawn` で起こしたスレッドの終わりを待つ
//! 2. `pin_module()` で DLL を外させない。notify の内部スレッドのように待てないものが残っても、コードは残る

use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use aviutl2::tracing;
use parking_lot::Mutex;

static SHUTDOWN: AtomicBool = AtomicBool::new(false);
static THREADS: Mutex<Vec<JoinHandle<()>>> = Mutex::new(Vec::new());

/// 終了に入ったか（裏のスレッドはこれを見て抜ける）
pub fn is_shutting_down() -> bool {
    SHUTDOWN.load(Ordering::SeqCst)
}

/// 名前付きのスレッドを起こし、終了時に待つ対象として覚える。終了に入った後は起こさない
pub fn spawn(name: &str, f: impl FnOnce() + Send + 'static) -> std::io::Result<()> {
    if is_shutting_down() {
        return Err(std::io::Error::other("FileFinder_H は終了中"));
    }
    let handle = std::thread::Builder::new()
        .name(name.to_string())
        .spawn(f)?;
    let mut threads = THREADS.lock();
    threads.retain(|h| !h.is_finished());
    threads.push(handle);
    Ok(())
}

/// 停止の印を立て、`spawn` で起こしたスレッドが終わるのを `timeout` まで待つ
pub fn shutdown(timeout: Duration) {
    SHUTDOWN.store(true, Ordering::SeqCst);
    let threads = std::mem::take(&mut *THREADS.lock());
    let deadline = Instant::now() + timeout;
    let mut left = 0usize;
    for h in threads {
        while !h.is_finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        if h.is_finished() {
            let _ = h.join();
        } else {
            left += 1;
        }
    }
    if left > 0 {
        tracing::warn!(
            "FileFinder_H: 終了時に {left} 本のスレッドが時間内に止まらなかった（DLL は pin してあるので落ちない）"
        );
    }
}

/// この DLL をプロセスが終わるまで外させない（GetModuleHandleExW の PIN）
pub fn pin_module() {
    use windows::Win32::System::LibraryLoader::{
        GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_PIN, GetModuleHandleExW,
    };
    let mut module = windows::Win32::Foundation::HMODULE::default();
    let addr = pin_module as *const () as *const u16;
    let result = unsafe {
        GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_PIN | GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS,
            windows::core::PCWSTR(addr),
            &mut module,
        )
    };
    if let Err(e) = result {
        tracing::warn!("FileFinder_H: DLL を pin できませんでした: {e}");
    }
}
