//! npm と Composer の依存の取得元が、公開レジストリ (registry.npmjs.org / Packagist) かを
//! プロジェクトの設定から判定する (#91)。
//!
//! 私設のレジストリやリポジトリを指す設定がある依存は、公開レジストリの版で判定・更新しない。
//! 同じ名前の公開パッケージの版を書き込むと、私設レジストリの構成によっては公開側を取り込む
//! (dependency confusion)。ミラーと私設レジストリは URL では区別できないので、どちらも外す。
//! 読めない設定や解釈できない値も、公開レジストリとみなさずに外す。
//!
//! 読むのはリポジトリ (なければ実行のルート) の中のファイルだけで、ユーザーの設定
//! (`~/.npmrc` など) と環境変数は読まない (Python の既定インデックスの判定と同じく、
//! プロジェクトに書かれた設定で判定する)。
use crate::domain::{Language, SkipReason};
use crate::manifest::ManifestInfo;
use std::path::Path;

mod composer;
mod node;

pub(crate) use composer::ComposerSources;
pub(crate) use node::NodeSources;

/// マニフェストの依存の取得元の設定
pub(crate) enum RegistrySources {
    Node(NodeSources),
    Composer(ComposerSources),
    /// 取得元をプロジェクトの設定で変えない言語
    None,
}

impl RegistrySources {
    /// `info` の依存に効く設定を読む。`root` は実行のルート。
    pub(crate) fn for_manifest(info: &ManifestInfo, root: &Path) -> Self {
        let directory = info.path.parent().unwrap_or(root);
        match info.language {
            Language::Node => Self::Node(NodeSources::load(directory, root)),
            Language::Php => Self::Composer(ComposerSources::read(&info.path)),
            _ => Self::None,
        }
    }

    /// `package` (実際のパッケージ名) を公開レジストリの版で判定しない理由
    pub(crate) fn skip_reason(&self, package: &str) -> Option<SkipReason> {
        match self {
            Self::Node(sources) => sources.verdict(package).skip_reason(),
            Self::Composer(sources) => sources.verdict(package).skip_reason(),
            Self::None => None,
        }
    }
}

/// 依存の取得元の判定
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Verdict {
    /// 公開レジストリ (取得元を変える設定がない)
    Public,
    /// 公開レジストリ以外を指す設定がある。どの設定かの説明を持つ
    NonPublic(String),
    /// 取得元の設定を読めない・解釈できない
    Unverified(String),
}

impl Verdict {
    pub(crate) fn skip_reason(self) -> Option<SkipReason> {
        match self {
            Self::Public => None,
            Self::NonPublic(origin) => Some(SkipReason::NonPublicSource(origin)),
            Self::Unverified(reason) => Some(SkipReason::SourceUnverified(reason)),
        }
    }
}

/// npm の公開レジストリか。registry.yarnpkg.com は Yarn が運用する同じ名前空間の入口。
/// ホストの完全一致に加え、ポート・認証情報・パス・クエリのない URL だけを公開とみなす
/// (似たホストやサブパスのミラーを公開と取り違えない)。
pub(crate) fn is_public_npm_registry(registry: &str) -> bool {
    reqwest::Url::parse(registry).is_ok_and(|url| {
        matches!(url.scheme(), "https" | "http")
            && matches!(
                url.host_str(),
                Some("registry.npmjs.org" | "registry.yarnpkg.com")
            )
            && url.port().is_none()
            && url.username().is_empty()
            && url.password().is_none()
            && url.path() == "/"
            && url.query().is_none()
            && url.fragment().is_none()
    })
}

/// Composer の公開リポジトリ (Packagist) の URL か。
fn is_packagist(url: &str) -> bool {
    reqwest::Url::parse(url).is_ok_and(|url| {
        matches!(url.scheme(), "https" | "http")
            && matches!(url.host_str(), Some("repo.packagist.org" | "packagist.org"))
            && url.port().is_none()
            && url.username().is_empty()
            && url.password().is_none()
            && matches!(url.path(), "/" | "")
            && url.query().is_none()
            && url.fragment().is_none()
    })
}

#[cfg(test)]
mod tests;
