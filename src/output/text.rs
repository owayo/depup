//! 人間が読みやすいテキスト出力フォーマッタ
//!
//! このモジュールが提供するもの:
//! - カラー付き更新結果の表示
//! - セマンティックバージョン変更種別の表示 (major/minor/patch)
//! - 本番/開発依存関係のグループ分け
//! - スキップされたパッケージの理由表示
//! - 詳細な内訳付きサマリ

#[cfg(test)]
use crate::domain::Language;
use crate::domain::{
    ChangeLevel, GitReference, GitSource, ManifestUpdateResult, SkipReason, UpdateResult,
    UpdateSummary,
};
use crate::orchestrator::OrchestratorResult;
use crate::output::{OutputFormatter, Verbosity};
use crate::update::numeric_core;
use chrono::{DateTime, Utc};
use colored::Colorize;
use std::io::Write;

/// セマンティックバージョン変更種別
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VersionChangeType {
    /// メジャーバージョン変更 (破壊的)
    Major,
    /// マイナーバージョン変更 (機能追加)
    Minor,
    /// パッチバージョン変更 (修正)
    Patch,
    /// 新規バージョン追加 (以前のバージョンなし)
    New,
    /// 不明またはパース不可
    Unknown,
}

impl VersionChangeType {
    /// 2つのバージョン間の変更種別を判定
    ///
    /// major/minor/patch の分類は `--max-change` の judge が使う
    /// `ChangeLevel::from_versions` (任意桁の数値コア比較) に委譲し、
    /// 表示ラベルと judge の分類が食い違わないようにする。
    pub fn from_versions(old: &str, new: &str) -> Self {
        // 旧バージョンが空か "-" なら新規追加
        if old.is_empty() || old == "-" {
            return VersionChangeType::New;
        }

        match ChangeLevel::from_versions(old, new) {
            Some(ChangeLevel::Major) => VersionChangeType::Major,
            Some(ChangeLevel::Minor) => VersionChangeType::Minor,
            Some(ChangeLevel::Patch) => VersionChangeType::Patch,
            // ChangeLevel は「数値コア同値」と「パース不能」の両方で None を返す。
            // 数値コアが両側で取れているなら先頭3セグメント同値 (旧実装と同じく
            // Patch 表示)、取れていなければ Unknown。
            None => {
                if numeric_core(old.trim()).is_empty() || numeric_core(new.trim()).is_empty() {
                    VersionChangeType::Unknown
                } else {
                    VersionChangeType::Patch
                }
            }
        }
    }

    /// カラー付き表示ラベルを取得
    pub fn colored_label(&self) -> String {
        match self {
            VersionChangeType::Major => "major".red().bold().to_string(),
            VersionChangeType::Minor => "minor".yellow().to_string(),
            VersionChangeType::Patch => "patch".green().to_string(),
            VersionChangeType::New => "new".cyan().to_string(),
            VersionChangeType::Unknown => "?".dimmed().to_string(),
        }
    }

    /// プレーンラベルを取得
    pub fn label(&self) -> &'static str {
        match self {
            VersionChangeType::Major => "major",
            VersionChangeType::Minor => "minor",
            VersionChangeType::Patch => "patch",
            VersionChangeType::New => "new",
            VersionChangeType::Unknown => "?",
        }
    }
}

/// verbose モードでのスキップ表示用パッケージ情報
struct SkipPackageInfo {
    name: String,
    version: String,
    released_at: Option<DateTime<Utc>>,
}

/// 条件付きスタイリング用の名前付きカラー
#[derive(Clone, Copy)]
enum Color {
    Red,
    Green,
    Yellow,
    Cyan,
    Dimmed,
}

/// 人間が読みやすい出力用テキストフォーマッタ
pub struct TextFormatter {
    /// 詳細度レベル
    verbosity: Verbosity,
    /// dry-run かどうか
    dry_run: bool,
    /// カラー表示を使用するか
    color: bool,
}

impl TextFormatter {
    /// 新しいテキストフォーマッタを作成
    pub fn new(verbosity: Verbosity, dry_run: bool) -> Self {
        Self {
            verbosity,
            dry_run,
            color: true,
        }
    }

    /// カラーオプション付きで新しいテキストフォーマッタを作成
    pub fn with_color(verbosity: Verbosity, dry_run: bool, color: bool) -> Self {
        Self {
            verbosity,
            dry_run,
            color,
        }
    }

    /// 該当する場合 dry-run プレフィックスを取得
    fn dry_run_prefix(&self) -> String {
        if self.dry_run {
            if self.color {
                format!("{} ", "(dry-run)".cyan())
            } else {
                "(dry-run) ".to_string()
            }
        } else {
            String::new()
        }
    }

    /// スキップ理由を表示用にフォーマット
    fn format_skip_reason(&self, reason: &SkipReason) -> String {
        match reason {
            SkipReason::Pinned => "pinned".to_string(),
            SkipReason::AlreadyLatest => "latest".to_string(),
            SkipReason::Excluded => "excluded".to_string(),
            SkipReason::NotInOnlyList => "not in --only".to_string(),
            SkipReason::FetchFailed(msg) => format!("fetch failed: {}", msg),
            SkipReason::LanguageFiltered => "filtered".to_string(),
            SkipReason::NoSuitableVersion => "no suitable version".to_string(),
            SkipReason::ParseError(msg) => format!("parse error: {}", msg),
            SkipReason::ChangeLevelLimited(level) => format!("max-change={}", level),
        }
    }

    /// 整列のためにパッケージ名の最大長を計算
    fn max_name_length(&self, results: &[&UpdateResult]) -> usize {
        results
            .iter()
            .map(|r| match r {
                UpdateResult::Update { dependency, .. } => dependency.name.len(),
                UpdateResult::Skip { dependency, .. } => dependency.name.len(),
            })
            .max()
            .unwrap_or(0)
    }

    /// 単一の更新行をフォーマット
    #[allow(clippy::too_many_arguments)]
    fn format_update_line(
        &self,
        name: &str,
        old_version: &str,
        new_version: &str,
        is_dev: bool,
        released_at: Option<DateTime<Utc>>,
        variable_name: Option<&str>,
        osv_skipped: &[String],
        osv_checked: bool,
        max_name_len: usize,
        writer: &mut dyn Write,
    ) -> std::io::Result<()> {
        let change_type = VersionChangeType::from_versions(old_version, new_version);
        let dev_marker = if is_dev { " 🔧" } else { "" };

        // リリース日をフォーマット
        let date_display = released_at
            .map(|d| format!(" ({})", d.format("%Y/%m/%d %H:%M")))
            .unwrap_or_default();

        // 変数インジケータをフォーマット
        let var_display = variable_name
            .map(|v| format!(" via ${}", v))
            .unwrap_or_default();

        // OSV インジケータ (採用版が OSV を通過した場合のみ)
        let osv_marker_plain = if osv_checked { " [osv-ok]" } else { "" };

        if self.color {
            let name_display = format!("{:width$}", name, width = max_name_len);
            let arrow = "→".dimmed();
            let change_label = change_type.colored_label();
            let dev_display = if is_dev {
                " 🔧".dimmed().to_string()
            } else {
                String::new()
            };
            let date_colored = released_at
                .map(|d| {
                    format!(" ({})", d.format("%Y/%m/%d %H:%M"))
                        .dimmed()
                        .to_string()
                })
                .unwrap_or_default();
            let var_colored = variable_name
                .map(|v| format!(" via ${}", v).cyan().to_string())
                .unwrap_or_default();
            let osv_colored = if osv_checked {
                " ✓ OSV".green().to_string()
            } else {
                String::new()
            };

            writeln!(
                writer,
                "  {} {} {} {} [{}]{}{}{}{}",
                name_display,
                old_version.dimmed(),
                arrow,
                new_version.bright_white().bold(),
                change_label,
                date_colored,
                var_colored,
                osv_colored,
                dev_display
            )?;
        } else {
            writeln!(
                writer,
                "  {:width$} {} -> {} [{}]{}{}{}{}",
                name,
                old_version,
                new_version,
                change_type.label(),
                date_display,
                var_display,
                osv_marker_plain,
                dev_marker,
                width = max_name_len
            )?;
        }

        // OSV で除外された候補があれば 1 行追記
        if !osv_skipped.is_empty() {
            let indent = " ".repeat(max_name_len + 4);
            let body = format!("↳ OSV skipped: {}", osv_skipped.join(", "));
            if self.color {
                writeln!(writer, "{}{}", indent, body.yellow())?;
            } else {
                writeln!(writer, "{}{}", indent, body)?;
            }
        }
        Ok(())
    }

    /// git 依存の更新行をフォーマットする
    /// 例: `  tree-sitter-xojo  branch=main  f41817b3 → 045c52a6  [git]`
    fn format_git_update_line(
        &self,
        name: &str,
        git: &GitSource,
        new_version: &str,
        is_dev: bool,
        max_name_len: usize,
        writer: &mut dyn Write,
    ) -> std::io::Result<()> {
        let ref_label = git.reference.display_name();
        let (old_display, new_display) = match &git.reference {
            GitReference::Tag(current) => (current.clone(), new_version.to_string()),
            _ => {
                let old_short = git
                    .short_current_commit()
                    .unwrap_or_else(|| "-".to_string());
                let new_short: String = new_version.chars().take(8).collect();
                (old_short, new_short)
            }
        };
        let dev_marker = if is_dev { " 🔧" } else { "" };
        if self.color {
            let name_display = format!("{:width$}", name, width = max_name_len);
            writeln!(
                writer,
                "  {} {} {} {} {} {}{}",
                name_display,
                ref_label.cyan(),
                old_display.dimmed(),
                "→".dimmed(),
                new_display.bright_white().bold(),
                "[git]".dimmed(),
                dev_marker.dimmed()
            )
        } else {
            writeln!(
                writer,
                "  {:width$} {} {} -> {} [git]{}",
                name,
                ref_label,
                old_display,
                new_display,
                dev_marker,
                width = max_name_len
            )
        }
    }

    /// 更新グループ (本番/開発) の各更新行を書き出す
    fn write_update_group(
        &self,
        updates: &[&UpdateResult],
        is_dev: bool,
        max_name_len: usize,
        writer: &mut dyn Write,
    ) -> std::io::Result<()> {
        for result in updates {
            if let UpdateResult::Update {
                dependency,
                new_version,
                released_at,
                osv_skipped,
                osv_checked,
            } = result
            {
                // git 依存は専用フォーマットで表示
                if let Some(git) = &dependency.git_source {
                    self.format_git_update_line(
                        &dependency.name,
                        git,
                        new_version,
                        is_dev,
                        max_name_len,
                        writer,
                    )?;
                    continue;
                }
                // バージョンなしの依存には "-" を表示
                let old_version = if dependency.version_spec.version.is_empty() {
                    "-"
                } else {
                    &dependency.version_spec.version
                };
                self.format_update_line(
                    &dependency.name,
                    old_version,
                    new_version,
                    is_dev,
                    *released_at,
                    dependency.variable_name.as_deref(),
                    osv_skipped,
                    *osv_checked,
                    max_name_len,
                    writer,
                )?;
            }
        }
        Ok(())
    }

    /// グループ化した更新でマニフェストをフォーマット
    fn format_manifest_grouped(
        &self,
        manifest: &ManifestUpdateResult,
        writer: &mut dyn Write,
    ) -> std::io::Result<()> {
        let prefix = self.dry_run_prefix();

        // 更新とスキップを収集
        let updates: Vec<_> = manifest.updates().collect();
        let skips: Vec<_> = manifest.skips().collect();

        // 更新もスキップもない完全に空のマニフェストを除外
        if updates.is_empty() && skips.is_empty() {
            return Ok(());
        }

        // 更新数とスキップ数をカウント
        let update_count = updates.len();
        let skip_count = skips.len();

        // 更新なしでスキップありの場合、スキップサマリを表示 (非 verbose モードでも)
        if updates.is_empty() && !skips.is_empty() {
            let path_display = manifest.path.display().to_string();
            let lang_display = format!("({})", manifest.language);
            // 更新ありの見出しと同じく `(dry-run)` 接頭辞を付ける。
            // ここだけ落とすと、両方が混在するプロジェクトで一部の行にしか付かない
            writeln!(
                writer,
                "{}{} {} — {} updates, {} {}",
                prefix,
                self.maybe_bold(&path_display),
                self.maybe_dimmed(&lang_display),
                self.maybe_dimmed("0"),
                self.apply_color(&skip_count.to_string(), Color::Yellow),
                if skip_count == 1 { "skip" } else { "skips" }
            )?;

            // スキップ理由のサマリを表示
            let skip_reasons = self.summarize_skip_reasons(&skips);
            self.write_skip_reasons(&skip_reasons, self.verbosity == Verbosity::Verbose, writer)?;
            writeln!(writer)?;
            return Ok(());
        }

        // 本番依存と開発依存を分離
        let (prod_updates, dev_updates): (Vec<&UpdateResult>, Vec<&UpdateResult>) =
            updates.into_iter().partition(|r| {
                if let UpdateResult::Update { dependency, .. } = r {
                    !dependency.is_dev
                } else {
                    true
                }
            });

        // カウント付きマニフェストヘッダを書き出す
        let path_display = manifest.path.display().to_string();
        let lang_display = format!("({})", manifest.language);
        writeln!(
            writer,
            "{}{} {} — {} {}, {} {}",
            prefix,
            self.maybe_bold(&path_display),
            self.maybe_dimmed(&lang_display),
            self.apply_color(&update_count.to_string(), Color::Green),
            if update_count == 1 {
                "update"
            } else {
                "updates"
            },
            self.maybe_dimmed(&skip_count.to_string()),
            if skip_count == 1 { "skip" } else { "skips" }
        )?;

        // 整列のために名前の最大長を取得 (分割済みベクタを使用)
        let all_results: Vec<&UpdateResult> = prod_updates
            .iter()
            .chain(dev_updates.iter())
            .copied()
            .collect();
        let max_name_len = self.max_name_length(&all_results).max(20);

        // 本番依存を書き出す
        if !prod_updates.is_empty() {
            self.write_update_group(&prod_updates, false, max_name_len, writer)?;
        }

        // 開発依存を書き出す
        if !dev_updates.is_empty() {
            self.write_update_group(&dev_updates, true, max_name_len, writer)?;
        }

        // verbose モードでスキップを書き出す
        if self.verbosity == Verbosity::Verbose && !skips.is_empty() {
            writeln!(writer)?;
            let skip_reasons = self.summarize_skip_reasons(&skips);
            self.write_skip_reasons(&skip_reasons, true, writer)?;
        }

        writeln!(writer)?;
        Ok(())
    }

    /// 変更種別ごとに更新数をカウント
    fn count_by_change_type(&self, summary: &UpdateSummary) -> (usize, usize, usize, usize, usize) {
        let mut major = 0;
        let mut minor = 0;
        let mut patch = 0;
        let mut new = 0;
        let mut unknown = 0;

        for manifest in &summary.manifests {
            for result in manifest.updates() {
                if let UpdateResult::Update {
                    dependency,
                    new_version,
                    ..
                } = result
                {
                    match VersionChangeType::from_versions(
                        &dependency.version_spec.version,
                        new_version,
                    ) {
                        VersionChangeType::Major => major += 1,
                        VersionChangeType::Minor => minor += 1,
                        VersionChangeType::Patch => patch += 1,
                        VersionChangeType::New => new += 1,
                        VersionChangeType::Unknown => unknown += 1,
                    }
                }
            }
        }

        (major, minor, patch, new, unknown)
    }

    /// 理由ごとにスキップ数をカウント
    fn count_by_skip_reason(&self, summary: &UpdateSummary) -> Vec<(String, usize)> {
        use std::collections::HashMap;
        let mut counts: HashMap<String, usize> = HashMap::new();

        for manifest in &summary.manifests {
            for result in manifest.skips() {
                if let UpdateResult::Skip { reason, .. } = result {
                    let key = self.format_skip_reason(reason);
                    *counts.entry(key).or_insert(0) += 1;
                }
            }
        }

        let mut result: Vec<_> = counts.into_iter().collect();
        // カウント降順。同数のときは HashMap のイテレーション順が実行ごとに変わる
        // (RandomState のシードがインスタンスごとに違う) ので、理由名を第 2 キーにして
        // 出力順を確定させる。CI のスナップショット比較で偽の差分が出るのを防ぐ
        result.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        result
    }

    /// カラー有効時にボールドスタイルを適用
    fn maybe_bold(&self, text: &str) -> String {
        if self.color {
            text.bold().to_string()
        } else {
            text.to_string()
        }
    }

    /// カラー有効時に暗いスタイルを適用
    fn maybe_dimmed(&self, text: &str) -> String {
        if self.color {
            text.dimmed().to_string()
        } else {
            text.to_string()
        }
    }

    /// カラー有効時にテキストに名前付きカラーを適用
    fn apply_color(&self, text: &str, color: Color) -> String {
        if self.color {
            match color {
                Color::Red => text.red().to_string(),
                Color::Green => text.green().to_string(),
                Color::Yellow => text.yellow().to_string(),
                Color::Cyan => text.cyan().to_string(),
                Color::Dimmed => text.dimmed().to_string(),
            }
        } else {
            text.to_string()
        }
    }

    /// スキップ理由行を書き出す (verbose: パッケージ詳細付き、normal: カウントのみ)
    fn write_skip_reasons(
        &self,
        skip_reasons: &[(String, usize, Vec<SkipPackageInfo>)],
        verbose: bool,
        writer: &mut dyn Write,
    ) -> std::io::Result<()> {
        for (reason, count, packages) in skip_reasons {
            if verbose {
                writeln!(
                    writer,
                    "  {} {}:",
                    self.apply_color(&count.to_string(), Color::Yellow),
                    self.maybe_dimmed(reason),
                )?;
                let max_name = packages
                    .iter()
                    .map(|p| p.name.len())
                    .max()
                    .unwrap_or(0)
                    .max(20);
                for pkg in packages {
                    let date_str = pkg
                        .released_at
                        .map(|d| format!("  ({})", d.format("%Y/%m/%d %H:%M")))
                        .unwrap_or_default();
                    // ANSI エスケープを含む文字列に {:width$} を使うと可視幅が
                    // ずれるため、先にパディングしてから着色する
                    let padded_name = format!("{:width$}", pkg.name, width = max_name);
                    writeln!(
                        writer,
                        "    {} {}{}",
                        self.maybe_dimmed(&padded_name),
                        self.maybe_dimmed(&pkg.version),
                        self.maybe_dimmed(&date_str),
                    )?;
                }
            } else {
                writeln!(
                    writer,
                    "  {} {}",
                    self.apply_color(&count.to_string(), Color::Yellow),
                    self.maybe_dimmed(reason)
                )?;
            }
        }
        Ok(())
    }

    /// スキップ結果のリストからスキップ理由をサマリ化
    /// 戻り値: Vec<(理由文字列, カウント, パッケージ情報)>
    fn summarize_skip_reasons(
        &self,
        skips: &[&UpdateResult],
    ) -> Vec<(String, usize, Vec<SkipPackageInfo>)> {
        use std::collections::HashMap;
        let mut groups: HashMap<String, Vec<SkipPackageInfo>> = HashMap::new();

        for result in skips {
            if let UpdateResult::Skip {
                dependency,
                reason,
                released_at,
            } = result
            {
                let key = self.format_skip_reason(reason);
                groups.entry(key).or_default().push(SkipPackageInfo {
                    name: dependency.name.clone(),
                    version: dependency.version_spec.version.clone(),
                    released_at: *released_at,
                });
            }
        }

        let mut result: Vec<_> = groups
            .into_iter()
            .map(|(reason, packages)| {
                let count = packages.len();
                (reason, count, packages)
            })
            .collect();
        // カウント降順。同数のときは理由名で確定させる (count_by_skip_reason と同方針)
        result.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        result
    }
}

impl OutputFormatter for TextFormatter {
    fn format(&self, result: &OrchestratorResult, writer: &mut dyn Write) -> std::io::Result<()> {
        // quiet モードではサマリのみ表示
        if self.verbosity == Verbosity::Quiet {
            return self.format_summary(&result.summary, writer);
        }

        // 各マニフェストをフォーマット
        for manifest in &result.summary.manifests {
            self.format_manifest_grouped(manifest, writer)?;
        }

        // エラーがあればフォーマット
        if !result.errors.is_empty() && self.verbosity != Verbosity::Quiet {
            if self.color {
                writeln!(writer, "{}:", "Errors".red().bold())?;
                for error in &result.errors {
                    writeln!(writer, "  {} {}", "✗".red(), error)?;
                }
            } else {
                writeln!(writer, "Errors:")?;
                for error in &result.errors {
                    writeln!(writer, "  - {}", error)?;
                }
            }
            writeln!(writer)?;
        }

        // サマリをフォーマット
        self.format_summary(&result.summary, writer)?;

        Ok(())
    }

    fn format_summary(
        &self,
        summary: &UpdateSummary,
        writer: &mut dyn Write,
    ) -> std::io::Result<()> {
        let prefix = self.dry_run_prefix();
        let updates = summary.total_updates();
        let skips = summary.total_skips();

        if self.verbosity == Verbosity::Quiet {
            // 最小限の出力
            if updates > 0 {
                writeln!(
                    writer,
                    "{}{} updated",
                    prefix,
                    self.apply_color(&updates.to_string(), Color::Green)
                )?;
            } else {
                writeln!(writer, "{}{}", prefix, self.maybe_dimmed("No updates"))?;
            }
            return Ok(());
        }

        // 変更種別ごとにカウント
        let (major, minor, patch, new, unknown) = self.count_by_change_type(summary);

        // Normal/Verbose 出力
        writeln!(writer, "{}{}:", prefix, self.maybe_bold("Summary"))?;

        // 更新の内訳
        if updates > 0 {
            let mut parts = Vec::new();
            let color_pairs: &[(usize, Color, &str)] = &[
                (major, Color::Red, "major"),
                (minor, Color::Yellow, "minor"),
                (patch, Color::Green, "patch"),
                (new, Color::Cyan, "new"),
                (unknown, Color::Dimmed, "other"),
            ];
            for &(count, color, label) in color_pairs {
                if count > 0 {
                    parts.push(format!(
                        "{} {}",
                        self.apply_color(&count.to_string(), color),
                        label
                    ));
                }
            }
            writeln!(
                writer,
                "  {} package(s) updated ({})",
                self.apply_color(&updates.to_string(), Color::Green),
                parts.join(", ")
            )?;
        } else {
            writeln!(writer, "  {}", self.maybe_dimmed("No packages updated"))?;
        }

        // スキップサマリ
        if skips > 0 {
            write!(
                writer,
                "  {} package(s) skipped",
                self.maybe_dimmed(&skips.to_string())
            )?;
            if self.verbosity == Verbosity::Verbose {
                let skip_counts = self.count_by_skip_reason(summary);
                if !skip_counts.is_empty() {
                    let parts: Vec<_> = skip_counts
                        .iter()
                        .map(|(reason, count)| format!("{} {}", count, reason))
                        .collect();
                    write!(writer, " ({})", self.maybe_dimmed(&parts.join(", ")))?;
                }
            }
            writeln!(writer)?;
        }

        // Verbose: 言語別の内訳を表示
        if self.verbosity == Verbosity::Verbose {
            writeln!(writer)?;
            writeln!(writer, "{}:", self.maybe_dimmed("By language"))?;
            for (language, lang_updates, lang_skips) in summary.language_breakdown() {
                let lang_name = if self.color {
                    language.to_string().cyan().to_string()
                } else {
                    language.to_string()
                };
                writeln!(
                    writer,
                    "  {}: {} updated, {} skipped",
                    lang_name,
                    self.apply_color(&lang_updates.to_string(), Color::Green),
                    self.maybe_dimmed(&lang_skips.to_string())
                )?;
            }
        }

        Ok(())
    }

    fn format_manifest(
        &self,
        manifest: &ManifestUpdateResult,
        writer: &mut dyn Write,
    ) -> std::io::Result<()> {
        self.format_manifest_grouped(manifest, writer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Dependency, VersionSpec, VersionSpecKind};
    use std::path::PathBuf;

    fn sample_dependency(name: &str, version: &str, is_dev: bool) -> Dependency {
        let spec = VersionSpec::new(VersionSpecKind::Caret, format!("^{}", version), version)
            .with_prefix("^");
        Dependency::new(name, spec, is_dev, Language::Node)
    }

    fn create_test_result() -> OrchestratorResult {
        let mut summary = UpdateSummary::new(false);
        let mut manifest = ManifestUpdateResult::new(PathBuf::from("package.json"), Language::Node);

        // 本番依存 - マイナー更新
        let dep1 = sample_dependency("lodash", "4.17.21", false);
        manifest.add_result(UpdateResult::update(dep1, "4.18.0"));

        // 開発依存 - パッチ更新
        let dep2 = sample_dependency("typescript", "5.0.0", true);
        manifest.add_result(UpdateResult::update(dep2, "5.0.1"));

        // スキップ
        let dep3 = sample_dependency("express", "4.18.0", false);
        manifest.add_result(UpdateResult::skip(dep3, SkipReason::AlreadyLatest));

        summary.add_manifest(manifest);

        OrchestratorResult {
            summary,
            write_results: Vec::new(),
            errors: Vec::new(),
        }
    }

    #[test]
    fn test_version_change_type_major() {
        assert_eq!(
            VersionChangeType::from_versions("1.0.0", "2.0.0"),
            VersionChangeType::Major
        );
        assert_eq!(
            VersionChangeType::from_versions("0.9.0", "1.0.0"),
            VersionChangeType::Major
        );
    }

    #[test]
    fn test_version_change_type_minor() {
        assert_eq!(
            VersionChangeType::from_versions("1.0.0", "1.1.0"),
            VersionChangeType::Minor
        );
        assert_eq!(
            VersionChangeType::from_versions("1.0.0", "1.5.0"),
            VersionChangeType::Minor
        );
    }

    #[test]
    fn test_version_change_type_patch() {
        assert_eq!(
            VersionChangeType::from_versions("1.0.0", "1.0.1"),
            VersionChangeType::Patch
        );
        assert_eq!(
            VersionChangeType::from_versions("1.0.0", "1.0.10"),
            VersionChangeType::Patch
        );
    }

    #[test]
    fn test_version_change_type_with_v_prefix() {
        assert_eq!(
            VersionChangeType::from_versions("v1.0.0", "v2.0.0"),
            VersionChangeType::Major
        );
    }

    #[test]
    fn test_version_change_type_short_versions() {
        assert_eq!(
            VersionChangeType::from_versions("1.0", "2.0"),
            VersionChangeType::Major
        );
        assert_eq!(
            VersionChangeType::from_versions("1", "2"),
            VersionChangeType::Major
        );
    }

    #[test]
    fn test_text_formatter_new() {
        let formatter = TextFormatter::new(Verbosity::Normal, false);
        assert_eq!(formatter.verbosity, Verbosity::Normal);
        assert!(!formatter.dry_run);
        assert!(formatter.color);
    }

    #[test]
    fn test_dry_run_prefix() {
        let formatter = TextFormatter::with_color(Verbosity::Normal, true, false);
        assert_eq!(formatter.dry_run_prefix(), "(dry-run) ");

        let formatter = TextFormatter::with_color(Verbosity::Normal, false, false);
        assert_eq!(formatter.dry_run_prefix(), "");
    }

    #[test]
    fn test_format_skip_reason() {
        let formatter = TextFormatter::new(Verbosity::Normal, false);

        assert_eq!(formatter.format_skip_reason(&SkipReason::Pinned), "pinned");
        assert_eq!(
            formatter.format_skip_reason(&SkipReason::AlreadyLatest),
            "latest"
        );
        assert_eq!(
            formatter.format_skip_reason(&SkipReason::Excluded),
            "excluded"
        );
        assert_eq!(
            formatter.format_skip_reason(&SkipReason::NotInOnlyList),
            "not in --only"
        );
        assert!(
            formatter
                .format_skip_reason(&SkipReason::FetchFailed("timeout".to_string()))
                .contains("fetch failed")
        );
    }

    #[test]
    fn test_format_normal() {
        let formatter = TextFormatter::with_color(Verbosity::Normal, false, false);
        let result = create_test_result();
        let mut output = Vec::new();

        formatter.format(&result, &mut output).unwrap();
        let output_str = String::from_utf8(output).unwrap();

        assert!(output_str.contains("package.json"));
        assert!(output_str.contains("lodash"));
        assert!(output_str.contains("4.17.21"));
        assert!(output_str.contains("4.18.0"));
        assert!(output_str.contains("[minor]"));
        assert!(output_str.contains("typescript"));
        assert!(output_str.contains("[patch]"));
        assert!(output_str.contains("🔧"));
        assert!(output_str.contains("Summary:"));
        assert!(output_str.contains("2 package(s) updated"));
    }

    #[test]
    fn test_format_quiet() {
        let formatter = TextFormatter::with_color(Verbosity::Quiet, false, false);
        let result = create_test_result();
        let mut output = Vec::new();

        formatter.format(&result, &mut output).unwrap();
        let output_str = String::from_utf8(output).unwrap();

        // quiet モードは最小限であるべき
        assert!(output_str.contains("2 updated"));
        assert!(!output_str.contains("Summary:"));
    }

    #[test]
    fn test_format_verbose() {
        let formatter = TextFormatter::with_color(Verbosity::Verbose, false, false);
        let result = create_test_result();
        let mut output = Vec::new();

        formatter.format(&result, &mut output).unwrap();
        let output_str = String::from_utf8(output).unwrap();

        // verbose モードではスキップされたパッケージのバージョンと言語別内訳を表示するべき
        assert!(output_str.contains("express"));
        assert!(output_str.contains("4.18.0")); // バージョンが表示される
        assert!(output_str.contains("latest"));
        assert!(output_str.contains("By language:"));
    }

    #[test]
    fn test_format_dry_run() {
        let formatter = TextFormatter::with_color(Verbosity::Normal, true, false);
        let result = create_test_result();
        let mut output = Vec::new();

        formatter.format(&result, &mut output).unwrap();
        let output_str = String::from_utf8(output).unwrap();

        assert!(output_str.contains("(dry-run)"));
    }

    #[test]
    fn test_format_summary_no_updates() {
        let formatter = TextFormatter::with_color(Verbosity::Normal, false, false);
        let summary = UpdateSummary::new(false);
        let mut output = Vec::new();

        formatter.format_summary(&summary, &mut output).unwrap();
        let output_str = String::from_utf8(output).unwrap();

        assert!(output_str.contains("No packages updated"));
    }

    #[test]
    fn test_format_summary_quiet_no_updates() {
        let formatter = TextFormatter::with_color(Verbosity::Quiet, false, false);
        let summary = UpdateSummary::new(false);
        let mut output = Vec::new();

        formatter.format_summary(&summary, &mut output).unwrap();
        let output_str = String::from_utf8(output).unwrap();

        assert!(output_str.contains("No updates"));
    }

    #[test]
    fn test_count_by_change_type() {
        let formatter = TextFormatter::new(Verbosity::Normal, false);
        let mut summary = UpdateSummary::new(false);
        let mut manifest = ManifestUpdateResult::new(PathBuf::from("package.json"), Language::Node);

        // メジャー
        let dep1 = sample_dependency("pkg1", "1.0.0", false);
        manifest.add_result(UpdateResult::update(dep1, "2.0.0"));

        // マイナー
        let dep2 = sample_dependency("pkg2", "1.0.0", false);
        manifest.add_result(UpdateResult::update(dep2, "1.1.0"));

        // パッチ
        let dep3 = sample_dependency("pkg3", "1.0.0", false);
        manifest.add_result(UpdateResult::update(dep3, "1.0.1"));

        summary.add_manifest(manifest);

        let (major, minor, patch, new, unknown) = formatter.count_by_change_type(&summary);
        assert_eq!(major, 1);
        assert_eq!(minor, 1);
        assert_eq!(patch, 1);
        assert_eq!(new, 0);
        assert_eq!(unknown, 0);
    }

    #[test]
    fn test_count_by_change_type_with_unknown() {
        let formatter = TextFormatter::new(Verbosity::Normal, false);
        let mut summary = UpdateSummary::new(false);
        let mut manifest = ManifestUpdateResult::new(PathBuf::from("package.json"), Language::Node);

        // 不明 (非 semver バージョン)
        let dep1 = sample_dependency("pkg1", "latest", false);
        manifest.add_result(UpdateResult::update(dep1, "2.0.0"));

        summary.add_manifest(manifest);

        let (major, minor, patch, new, unknown) = formatter.count_by_change_type(&summary);
        assert_eq!(major, 0);
        assert_eq!(minor, 0);
        assert_eq!(patch, 0);
        assert_eq!(new, 0);
        assert_eq!(unknown, 1);
    }

    #[test]
    fn test_count_by_change_type_with_new() {
        let formatter = TextFormatter::new(Verbosity::Normal, false);
        let mut summary = UpdateSummary::new(false);
        let mut manifest = ManifestUpdateResult::new(PathBuf::from("Gemfile"), Language::Ruby);

        // 新規 (以前のバージョンなし - 空文字列)
        let spec = VersionSpec::new(VersionSpecKind::Any, "", "");
        let dep1 = Dependency::new("rmagick", spec, false, Language::Ruby);
        manifest.add_result(UpdateResult::update(dep1, "6.1.5"));

        summary.add_manifest(manifest);

        let (major, minor, patch, new, unknown) = formatter.count_by_change_type(&summary);
        assert_eq!(major, 0);
        assert_eq!(minor, 0);
        assert_eq!(patch, 0);
        assert_eq!(new, 1);
        assert_eq!(unknown, 0);
    }

    #[test]
    fn test_version_change_type_new() {
        // 空のバージョンは新規追加
        assert_eq!(
            VersionChangeType::from_versions("", "1.0.0"),
            VersionChangeType::New
        );
        // ダッシュもバージョンなし扱い
        assert_eq!(
            VersionChangeType::from_versions("-", "1.0.0"),
            VersionChangeType::New
        );
    }

    #[test]
    fn test_version_change_type_with_prerelease() {
        // プレリリースサフィックス付きバージョン（-で分割されるため数値部分のみ比較）
        assert_eq!(
            VersionChangeType::from_versions("1.0.0-alpha", "2.0.0"),
            VersionChangeType::Major
        );
        assert_eq!(
            VersionChangeType::from_versions("1.0.0-beta.1", "1.1.0"),
            VersionChangeType::Minor
        );
    }

    #[test]
    fn test_version_change_type_unknown() {
        // パースできないバージョン
        assert_eq!(
            VersionChangeType::from_versions("latest", "2.0.0"),
            VersionChangeType::Unknown
        );
        assert_eq!(
            VersionChangeType::from_versions("abc", "def"),
            VersionChangeType::Unknown
        );
    }

    #[test]
    fn test_version_change_type_label() {
        // ラベル文字列の確認
        assert_eq!(VersionChangeType::Major.label(), "major");
        assert_eq!(VersionChangeType::Minor.label(), "minor");
        assert_eq!(VersionChangeType::Patch.label(), "patch");
        assert_eq!(VersionChangeType::New.label(), "new");
        assert_eq!(VersionChangeType::Unknown.label(), "?");
    }

    #[test]
    fn test_version_change_type_same_major_minor_different_patch() {
        // パッチのみ異なる場合
        assert_eq!(
            VersionChangeType::from_versions("1.2.3", "1.2.10"),
            VersionChangeType::Patch
        );
    }

    #[test]
    fn test_version_change_type_four_segment() {
        // 4セグメントバージョン（Ruby等、3セグメント目までで比較）
        assert_eq!(
            VersionChangeType::from_versions("1.2.3.4", "2.0.0.0"),
            VersionChangeType::Major
        );
    }

    /// 回帰テスト: ChangeLevel への委譲で judge (--max-change) と分類が揃う。
    /// 以前の独自 u64 パーサは u64 超の数値で Unknown、大文字 V 接頭辞で
    /// Unknown になり、judge の分類と食い違っていた。
    #[test]
    fn test_version_change_type_matches_change_level_semantics() {
        // u64 を超える数値コアでも任意桁10進で比較される
        assert_eq!(
            VersionChangeType::from_versions("18446744073709551616.0.0", "2.0.0"),
            VersionChangeType::Major
        );
        // 大文字 V 接頭辞も numeric_core 側で除去される
        assert_eq!(
            VersionChangeType::from_versions("V1.0.0", "V2.0.0"),
            VersionChangeType::Major
        );
        // qualifier セグメント (数値なし) は無視され Patch 扱い
        assert_eq!(
            VersionChangeType::from_versions("5.0.0.RELEASE", "5.0.1"),
            VersionChangeType::Patch
        );
    }
}
