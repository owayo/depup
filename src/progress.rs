//! 依存関係更新のプログレス表示
//!
//! indicatif を使用して更新ワークフロー中の視覚的フィードバックを提供する。

use indicatif::{ProgressBar, ProgressStyle};
use std::time::Duration;

/// 更新ワークフローのプログレスレポーター
pub struct Progress {
    /// プログレス表示が有効か (quiet モードでは無効)
    enabled: bool,
    /// 現在のプログレスバー
    bar: Option<ProgressBar>,
}

impl Progress {
    /// 新しいプログレスレポーターを作成する
    pub fn new(enabled: bool) -> Self {
        Self { enabled, bar: None }
    }

    /// 無効なプログレスレポーターを作成する
    pub fn disabled() -> Self {
        Self::new(false)
    }

    /// 不定操作用のスピナーをメッセージ付きで表示する
    pub fn spinner(&mut self, message: &str) {
        if !self.enabled {
            return;
        }

        let spinner = ProgressBar::new_spinner();
        spinner.set_style(
            ProgressStyle::default_spinner()
                .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏")
                .template("{spinner:.cyan} {msg}")
                .expect("Invalid template"),
        );
        spinner.set_message(message.to_string());
        spinner.enable_steady_tick(Duration::from_millis(80));
        self.bar = Some(spinner);
    }

    /// 既知の件数に対するプログレスバーを開始する
    pub fn start(&mut self, total: u64, message: &str) {
        if !self.enabled {
            return;
        }

        let bar = ProgressBar::new(total);
        bar.set_style(
            ProgressStyle::default_bar()
                .template("{spinner:.cyan} {msg} [{bar:30.cyan/blue}] {pos}/{len} ({eta})")
                .expect("Invalid template")
                .progress_chars("█▓▒░"),
        );
        bar.set_message(message.to_string());
        bar.enable_steady_tick(Duration::from_millis(100));
        self.bar = Some(bar);
    }

    /// プログレスを1つ進める
    pub fn inc(&self) {
        if let Some(ref bar) = self.bar {
            bar.inc(1);
        }
    }

    /// 内部の `ProgressBar` を取得する (並列タスクから直接 `inc` / `set_message` するため)。
    ///
    /// プログレス表示が無効・未開始の場合は `None`。返される `ProgressBar` は
    /// 内部で `Arc` を持つ Clone で、複数の async タスク間で安全に共有できる。
    pub fn bar(&self) -> Option<ProgressBar> {
        self.bar.clone()
    }

    /// プログレスバーを一時的に隠してクロージャを実行する
    ///
    /// バーの描画中に `eprintln!` すると出力行とバーが混ざるため、進行中に
    /// 警告や詳細ログを出したいときはこれで包む。バーが無い (quiet / 未開始) 場合は
    /// クロージャをそのまま実行する。
    pub fn suspend<F: FnOnce() -> R, R>(&self, f: F) -> R {
        match self.bar {
            Some(ref bar) => bar.suspend(f),
            None => f(),
        }
    }

    /// メッセージを更新する
    pub fn set_message(&self, message: &str) {
        if let Some(ref bar) = self.bar {
            bar.set_message(message.to_string());
        }
    }

    /// メッセージ付きで現在のプログレスバーを完了する
    pub fn finish(&mut self, message: &str) {
        if let Some(ref bar) = self.bar {
            bar.finish_with_message(message.to_string());
        }
        self.bar = None;
    }

    /// 現在のプログレスバーを完了してクリアする
    pub fn finish_and_clear(&mut self) {
        if let Some(ref bar) = self.bar {
            bar.finish_and_clear();
        }
        self.bar = None;
    }
}

impl Default for Progress {
    fn default() -> Self {
        Self::new(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_progress_disabled() {
        let mut progress = Progress::disabled();
        progress.spinner("test");
        progress.start(10, "test");
        progress.inc();
        progress.set_message("test");
        progress.finish("done");
    }

    #[test]
    fn test_progress_enabled() {
        let mut progress = Progress::new(true);
        progress.start(3, "Processing");
        progress.inc();
        progress.set_message("item 1");
        progress.inc();
        progress.finish_and_clear();
    }

    #[test]
    fn test_progress_default_is_enabled() {
        let mut progress = Progress::default();
        // デフォルトは有効; スピナーでバーが作成されるはず
        progress.spinner("loading");
        progress.finish("done");
    }

    #[test]
    fn test_progress_spinner_then_finish_and_clear() {
        let mut progress = Progress::new(true);
        progress.spinner("scanning");
        progress.set_message("updated message");
        progress.finish_and_clear();
        // finish_and_clear 後は inc/set_message はノーオペになる
        progress.inc();
        progress.set_message("no-op");
    }

    #[test]
    fn test_progress_disabled_operations_are_noop() {
        let mut progress = Progress::disabled();
        // 無効時は全操作が安全なノーオペであること
        progress.spinner("test");
        progress.inc();
        progress.set_message("msg");
        progress.finish_and_clear();
        progress.start(5, "test");
        progress.inc();
        progress.finish("done");
    }
}
