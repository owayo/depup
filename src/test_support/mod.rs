//! テスト専用の土台 (偽の crates.io と、それを使う Cargo プロジェクト、決めた応答を返す HTTP サーバ、
//! 偽のコマンドの書き込み)
#[cfg(unix)]
pub(crate) mod exec;
pub(crate) mod fake_crates_io;
pub(crate) mod http;
pub(crate) mod local_registry;
