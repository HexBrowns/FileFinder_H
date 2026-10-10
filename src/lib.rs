//! FileFinder_H — 指定フォルダ以下のファイルをあいまい検索して、タイムラインに挿入する。
//!
//! 画面の作りと検索の方式は aviutl2-rs の examples/scripts-search-plugin（Rusty Scripts Search）に倣った。

mod config;
mod dnd;
mod folder_dialog;
mod gui;
mod history;
mod index;
mod kana;
mod query;
mod shutdown;
mod store;
mod watcher;

use aviutl2::{AnyResult, tracing};

pub const WINDOW_NAME: &str = "ファイル検索";

pub static EDIT_HANDLE: aviutl2::generic::GlobalEditHandle =
    aviutl2::generic::GlobalEditHandle::new();

#[aviutl2::plugin(GenericPlugin)]
pub struct FileFinderPlugin {
    window: aviutl2_eframe::EframeWindow,
}

fn init_logging() {
    let level = if cfg!(debug_assertions) {
        tracing::Level::DEBUG
    } else {
        tracing::Level::INFO
    };
    let _ = aviutl2::tracing_subscriber::fmt()
        .with_max_level(level)
        .event_format(aviutl2::logger::AviUtl2Formatter)
        .with_writer(aviutl2::logger::AviUtl2LogWriter)
        .try_init();
}

impl aviutl2::generic::GenericPlugin for FileFinderPlugin {
    fn new(_info: aviutl2::AviUtl2Info) -> AnyResult<Self> {
        init_logging();
        // 裏のスレッドが残っても消えたコードを実行しないよう、DLL をプロセスの終わりまで残す（shutdown.rs）
        shutdown::pin_module();
        tracing::info!("FileFinder_H v{} 初期化", env!("CARGO_PKG_VERSION"));
        let window = aviutl2_eframe::EframeWindow::new("FileFinder_H", move |cc, handle| {
            Ok(Box::new(gui::FileFinderApp::new(cc, handle)))
        })?;
        Ok(Self { window })
    }

    fn plugin_info(&self) -> aviutl2::generic::GenericPluginTable {
        aviutl2::generic::GenericPluginTable {
            name: "FileFinder_H".to_string(),
            information: format!(
                "FileFinder_H v{} - 指定フォルダのファイルをあいまい検索して挿入 / by HexBrowns",
                env!("CARGO_PKG_VERSION")
            ),
        }
    }

    fn register(&mut self, registry: &mut aviutl2::generic::HostAppHandle) {
        match self.window.handle() {
            Ok(handle) => {
                if let Err(e) = registry.register_window_client(WINDOW_NAME, &handle) {
                    tracing::error!("FileFinder_H: ウィンドウを登録できませんでした: {e}");
                }
            }
            Err(e) => tracing::error!("FileFinder_H: ウィンドウを作れませんでした: {e}"),
        }
        EDIT_HANDLE.init(registry.create_edit_handle());
    }
}

impl Drop for FileFinderPlugin {
    fn drop(&mut self) {
        // 本体が DLL を外す前に、監視と走査のスレッドを止めて終わりを待つ（shutdown.rs）
        shutdown::shutdown(std::time::Duration::from_secs(2));
    }
}

aviutl2::register_generic_plugin!(FileFinderPlugin);
