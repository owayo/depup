//! 各パッケージエコシステムのバージョン指定パーサ
//!
//! 以下の言語のバージョン指定をパースする:
//! - Node.js（npm/yarn/pnpm）向け
//! - Python（pip/poetry）向け
//! - Rust（cargo）向け
//! - Go（go mod）向け
//! - Ruby（bundler）向け
//! - PHP（composer）向け
//! - Java（gradle）向け
//! - Swift（SPM）向け

mod go;
mod java;
mod node;
mod php;
mod python;
mod ruby;
mod rust;
mod swift;

pub use go::GoVersionParser;
pub use java::JavaVersionParser;
pub use node::NodeVersionParser;
pub use php::PhpVersionParser;
pub use python::PythonVersionParser;
pub use ruby::RubyVersionParser;
pub use rust::RustVersionParser;
pub use swift::SwiftVersionParser;

use crate::domain::{Language, VersionSpec};

/// 入力が完全浮動ワイルドカード (`*`, `*.*`, `x.x`, `v*`, `V*` 等の数値アンカーなし) かを判定する。
/// 数値アンカーがないとレジストリ最新版を埋め込んでも形が崩れ、書き換え結果が
/// raw と同一になる phantom update の原因になる。Node / PHP のワイルドカード判定で共有する。
pub(crate) fn is_fully_floating_wildcard(raw: &str) -> bool {
    !raw.chars().any(|ch| ch.is_ascii_digit())
}

/// 演算子付きアンカー正規表現パターンを生成する。
/// 各言語パーサの GTE/GT/LTE/LT (および同形の caret/tilde) ラッパが共用し、
/// CORE 定数だけ差し替えても「1本だけパターンがずれる」不整合を防ぐ。
pub(crate) fn anchored_op_pattern(op: &str, core: &str) -> String {
    format!(r"^{op}\s*({core})$")
}

/// バージョン指定のパースを行うトレイト
pub trait VersionParser {
    /// バージョン指定文字列をパースする
    fn parse(&self, version_str: &str) -> Option<VersionSpec>;

    /// このパーサが対応する言語を返す
    fn language(&self) -> Language;

    /// 演算子なしのバージョン文字列が完全一致ピンを意味するコンテキスト向けの解析。
    ///
    /// Poetry の `tool.poetry.dependencies` では `requests = "2.28.0"` のような
    /// 演算子なしの記述が完全一致ピン (公式ドキュメントの "Exact requirements"、
    /// `==2.28.0` と同義) を意味する。一方 pip / PEP 508 の依存指定では演算子が
    /// 必須なので、この解釈はマニフェスト側が Poetry コンテキストと分かっている
    /// 箇所からのみ呼ぶ。デフォルトは通常の `parse` に委譲する (追加解釈なし)。
    fn parse_exact_pin(&self, version_str: &str) -> Option<VersionSpec> {
        self.parse(version_str)
    }
}

/// 指定された言語に対応するバージョンパーサを取得する
pub fn get_parser(language: Language) -> Box<dyn VersionParser> {
    match language {
        Language::Node => Box::new(NodeVersionParser),
        Language::Python => Box::new(PythonVersionParser),
        Language::Rust => Box::new(RustVersionParser),
        Language::Go => Box::new(GoVersionParser),
        Language::Ruby => Box::new(RubyVersionParser),
        Language::Php => Box::new(PhpVersionParser),
        Language::Java => Box::new(JavaVersionParser),
        Language::Swift => Box::new(SwiftVersionParser),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_parser_node() {
        let parser = get_parser(Language::Node);
        assert_eq!(parser.language(), Language::Node);
    }

    #[test]
    fn test_get_parser_python() {
        let parser = get_parser(Language::Python);
        assert_eq!(parser.language(), Language::Python);
    }

    #[test]
    fn test_get_parser_rust() {
        let parser = get_parser(Language::Rust);
        assert_eq!(parser.language(), Language::Rust);
    }

    #[test]
    fn test_get_parser_go() {
        let parser = get_parser(Language::Go);
        assert_eq!(parser.language(), Language::Go);
    }

    #[test]
    fn test_anchored_op_pattern_shape() {
        // 生成形 `^{op}\s*({core})$` を固定する (op は正規表現片としてそのまま埋め込まれる)
        assert_eq!(anchored_op_pattern(r">=", "X"), r"^>=\s*(X)$");
        assert_eq!(anchored_op_pattern(r"\^", "X"), r"^\^\s*(X)$");
        assert_eq!(anchored_op_pattern(r"~>?", "X"), r"^~>?\s*(X)$");
    }
}
