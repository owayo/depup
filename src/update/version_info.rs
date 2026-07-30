//! レジストリからのバージョン情報
//!
//! このモジュールはリリース日付を伴うパッケージバージョンを表す
//! VersionInfo 構造体を提供する。

use chrono::{DateTime, Utc};
use pep440_rs::Version as Pep440Version;
use serde::{Deserialize, Serialize};

/// レジストリから取得したパッケージバージョンの情報
///
/// `Eq`/`Ord` はバージョン文字列のみで比較する（`released_at` は無視）。
/// これにより BTreeSet 等でも一貫した振る舞いになる。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionInfo {
    /// バージョン文字列 (例: "1.2.3")
    pub version: String,
    /// このバージョンがリリースされた日時
    pub released_at: DateTime<Utc>,
}

impl VersionInfo {
    /// 新しいVersionInfoを作成する
    pub fn new(version: impl Into<String>, released_at: DateTime<Utc>) -> Self {
        Self {
            version: version.into(),
            released_at,
        }
    }

    /// リリース日として現在時刻を使用してVersionInfoを作成する
    pub fn now(version: impl Into<String>) -> Self {
        Self {
            version: version.into(),
            released_at: Utc::now(),
        }
    }

    /// このバージョンがプレリリース (alpha, beta, rc, canary, dev 等) かチェックする
    pub fn is_prerelease(&self) -> bool {
        is_prerelease_version(&self.version)
    }
}

/// チェック対象のプレリリース識別子
///
/// 安定版として扱わない suffix を列挙する。semver の prerelease マーカーに加え、
/// `-deprecated` / `-yanked` のように作者が「更新非推奨」を示すためにリリース末尾へ
/// 付けるマーカーも除外対象に含める (例: `serde_yaml 0.9.34-deprecated`)。
const PRERELEASE_IDENTIFIERS: &[&str] = &[
    "alpha",
    "beta",
    "rc",
    "canary",
    "dev",
    "preview",
    "next",
    "nightly",
    "snapshot",
    "pre",
    "insiders",
    "experimental",
    // JVM 系の milestone 版 (`1.0.0-milestone1`)。短縮形 `M1` は
    // `contains_milestone_identifier` が別途判定する
    "milestone",
    // 非推奨マーカー (crates.io などで作者が自発的に付与)
    "deprecated",
    "obsolete",
    "retired",
    "yanked",
    "unmaintained",
];

/// セパレータ (`-`, `.`, `+`, 文字列境界) で区切られた単語としてマッチするかチェックする。
/// 部分文字列マッチによる誤検出 ("enterprise" に "pre" がマッチ等) を防止する。
fn contains_identifier_word(haystack: &str, needle: &str) -> bool {
    let bytes = haystack.as_bytes();
    let needle_len = needle.len();
    let mut start = 0;
    while let Some(pos) = haystack[start..].find(needle) {
        let abs = start + pos;
        // 前方境界: 文字列先頭、またはセパレータ文字
        let before_ok = abs == 0 || matches!(bytes[abs - 1], b'-' | b'.' | b'+' | b'_' | b' ');
        // 後方境界: 文字列末尾、セパレータ文字、または数字 (例: "alpha1" の "alpha" + "1")
        let end = abs + needle_len;
        let after_ok = end >= haystack.len()
            || matches!(bytes[end], b'-' | b'.' | b'+' | b'_' | b' ')
            || bytes[end].is_ascii_digit();
        if before_ok && after_ok {
            return true;
        }
        start = abs + 1;
        if start >= haystack.len() {
            break;
        }
    }
    false
}

/// JVM 系の milestone 短縮表記 (`4.0.0-M1` / `2.0.0.M3`) を判定する。
///
/// Spring / JUnit / AssertJ / Micronaut など JVM 系は安定版前の公開に `M<数字>` を使う。
/// Gradle の version ordering でも qualifier 付きは無印より小さい (= プレリリース) ため、
/// `alpha` / `rc` と同様に安定版利用者の更新候補からは除外する。
/// これが無いと `3.24.2` の利用者が `4.0.0-M1` へ、`5.10.0` が `5.13.0-M3` へ更新されていた。
///
/// `m` の直後が数字のトークンだけを対象にするため、`macos1` / `m2m` のような
/// 通常の識別子は milestone と誤判定しない。`.Final` / `-jre` / `.RELEASE` のような
/// JVM の安定版 qualifier も当然対象外 (Java で「qualifier があれば一律プレリリース」と
/// すると、これらを巻き込んで安定版が全滅する)。
fn contains_milestone_identifier(scan: &str) -> bool {
    scan.split(['-', '.', '_', ' ']).any(|token| {
        token.len() >= 2 && token.starts_with('m') && token[1..].bytes().all(|b| b.is_ascii_digit())
    })
}

/// 先頭の ASCII `v` / `V` 接頭辞を 1 文字だけ取り除く。
///
/// `v1.2.3` / `V1.2.3` のような接頭辞付きバージョン表記を比較・正規化の前に
/// 揃えるための共通ヘルパー。接頭辞がなければ入力をそのまま返す。
/// (Packagist の `normalize_version` は仕様に合わせて小文字 `v` のみを剥がす別処理)
pub(crate) fn strip_ascii_v_prefix(s: &str) -> &str {
    s.strip_prefix('v')
        .or_else(|| s.strip_prefix('V'))
        .unwrap_or(s)
}

/// バージョン文字列がプレリリースバージョンを表すかチェックする
pub fn is_prerelease_version(version: &str) -> bool {
    let lower = version.to_lowercase();
    // semver のビルドメタデータ (`+` 以降) はバージョンの優先度に影響しないため、
    // prerelease 判定の走査対象から除外する。これにより `1.0.0+sha.a1b2c3` の
    // メタデータ内 "digit+英字+digit" パターンや `1.0.0+pre.1` の識別子を
    // prerelease と誤判定しない。
    let scan = lower.split('+').next().unwrap_or("");

    // 単語境界ベースの識別子をチェック (alpha, beta, canary 等)
    // セパレータ (-._ またはバージョン境界) で区切られた単語としてマッチする
    if PRERELEASE_IDENTIFIERS
        .iter()
        .any(|id| contains_identifier_word(scan, id))
    {
        return true;
    }

    // JVM 系の milestone 短縮表記 (`4.0.0-M1` / `2.0.0.M3`)
    if contains_milestone_identifier(scan) {
        return true;
    }

    // Python/PEP 440 形式の短縮識別子をチェック:
    // - 26.1a1（アルファ）、21.12b0（ベータ）、1.0c1（リリース候補）
    // - 1.0rc1, 2.0.0rc1 (release candidate, セパレータなしの rc 表記)
    // パターン: 数字の後に 'rc'+数字、または 'a'/'b'/'c'+数字 が続く
    let chars: Vec<char> = scan.chars().collect();
    for i in 0..chars.len() {
        if !chars[i].is_ascii_digit() {
            continue;
        }
        // 'rc' + 数字 (例: 1.0rc1, 2.0.0rc1)。PEP 440 で最も一般的な rc 表記。
        if chars.get(i + 1) == Some(&'r')
            && chars.get(i + 2) == Some(&'c')
            && chars.get(i + 3).is_some_and(|c| c.is_ascii_digit())
        {
            return true;
        }
        // 'a'/'b'/'c' + 数字 (例: 26.1a1, 21.12b0, 1.0c1)
        if let Some(&next) = chars.get(i + 1)
            && matches!(next, 'a' | 'b' | 'c')
            && chars.get(i + 2).is_some_and(|c| c.is_ascii_digit())
        {
            return true;
        }
    }

    false
}

/// Python の PEP 440 正規化規則に従ってプレリリースかを判定する。
///
/// `_` 区切りや `preview` / `c` などの代替綴りも標準パーサで正規化する。
/// PEP 440 でない入力は、他エコシステムとの互換性を保つため共通判定へ戻す。
pub(crate) fn is_python_prerelease_version(version: &str) -> bool {
    version
        .parse::<Pep440Version>()
        .map(|parsed| parsed.any_prerelease())
        .unwrap_or_else(|_| is_prerelease_version(version))
}

impl PartialEq for VersionInfo {
    fn eq(&self, other: &Self) -> bool {
        compare_versions(&self.version, &other.version) == std::cmp::Ordering::Equal
    }
}

impl Eq for VersionInfo {}

impl Ord for VersionInfo {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // semver 風の比較でバージョンを比較
        compare_versions(&self.version, &other.version)
    }
}

impl PartialOrd for VersionInfo {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// 任意桁の非負整数を比較可能な正規形で保持する。
///
/// レジストリ由来のバージョンは `u64` の上限を超える場合があるため、数値を
/// 10 進文字列のまま保持し、桁数と辞書順で比較する。先頭ゼロは除去する。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NumericIdentifier(String);

impl NumericIdentifier {
    fn parse(value: &str) -> Option<Self> {
        if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }

        let normalized = value.trim_start_matches('0');
        Some(Self(if normalized.is_empty() {
            "0".to_string()
        } else {
            normalized.to_string()
        }))
    }
}

impl Default for NumericIdentifier {
    fn default() -> Self {
        Self("0".to_string())
    }
}

impl Ord for NumericIdentifier {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0
            .len()
            .cmp(&other.0.len())
            .then_with(|| self.0.cmp(&other.0))
    }
}

impl PartialOrd for NumericIdentifier {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// 文字列末尾の連続する数字を任意桁の数値として取り出す。
/// PEP 440 のポストリリース (例: `post1` → 1、`post` → None) の
/// 数値識別子抽出に使う。
fn trailing_number(s: &str) -> Option<NumericIdentifier> {
    let digits: String = s.chars().rev().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() {
        return None;
    }
    NumericIdentifier::parse(&digits.chars().rev().collect::<String>())
}

/// プレリリース識別子 1 個分の構造化表現 (semver 11.4 準拠の比較用)。
///
/// - `Numeric`: 数値のみの識別子 (例: `rc.1` の `1`)。数値として比較する。
/// - `Alpha`: 英字を含む識別子 (例: `alpha`, `rc`)。小文字化して格納し
///   大小文字差を吸収のうえ ASCII 辞書順で比較する。
#[derive(Debug, Clone, PartialEq, Eq)]
enum PreIdentifier {
    /// 数値識別子
    Numeric(NumericIdentifier),
    /// 英字を含む識別子 (小文字化済み)
    Alpha(String),
}

impl PreIdentifier {
    /// セパレータ区切りの識別子 1 個をパースする。
    /// 数値としてパースできれば `Numeric`、できなければ小文字化した `Alpha`。
    fn from_part(part: &str) -> Self {
        match NumericIdentifier::parse(part) {
            Some(n) => PreIdentifier::Numeric(n),
            None => PreIdentifier::Alpha(part.to_ascii_lowercase()),
        }
    }
}

impl Ord for PreIdentifier {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        use std::cmp::Ordering;
        match (self, other) {
            (PreIdentifier::Numeric(a), PreIdentifier::Numeric(b)) => a.cmp(b),
            // semver 11.4.3: 数値識別子は英数字識別子より常に小さい
            (PreIdentifier::Numeric(_), PreIdentifier::Alpha(_)) => Ordering::Less,
            (PreIdentifier::Alpha(_), PreIdentifier::Numeric(_)) => Ordering::Greater,
            (PreIdentifier::Alpha(a), PreIdentifier::Alpha(b)) => {
                // PEP 440 整合の特例: "dev" は他のどの Alpha よりも小さい
                // (PEP 440 では dev < a < b < rc。辞書順だと b < dev < rc になるため)
                match (a == "dev", b == "dev") {
                    (true, true) => Ordering::Equal,
                    (true, false) => Ordering::Less,
                    (false, true) => Ordering::Greater,
                    // ASCII 辞書順 (semver 11.4.4)
                    (false, false) => a.cmp(b),
                }
            }
        }
    }
}

impl PartialOrd for PreIdentifier {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum LocalIdentifier {
    /// 英字を含む local version セグメント (小文字化済み)
    Alpha(String),
    /// 数値のみの local version セグメント
    Numeric(NumericIdentifier),
}

impl LocalIdentifier {
    fn from_part(part: &str) -> Self {
        match NumericIdentifier::parse(part) {
            Some(n) => LocalIdentifier::Numeric(n),
            None => LocalIdentifier::Alpha(part.to_ascii_lowercase()),
        }
    }
}

impl Ord for LocalIdentifier {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        use std::cmp::Ordering;

        match (self, other) {
            (LocalIdentifier::Numeric(a), LocalIdentifier::Numeric(b)) => a.cmp(b),
            (LocalIdentifier::Alpha(a), LocalIdentifier::Alpha(b)) => a.cmp(b),
            // PEP 440 local version では数値セグメントが英字セグメントより大きい。
            (LocalIdentifier::Numeric(_), LocalIdentifier::Alpha(_)) => Ordering::Greater,
            (LocalIdentifier::Alpha(_), LocalIdentifier::Numeric(_)) => Ordering::Less,
        }
    }
}

impl PartialOrd for LocalIdentifier {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

type PythonVersionComponents = (
    NumericIdentifier,
    Vec<NumericIdentifier>,
    Option<Vec<PreIdentifier>>,
    Option<NumericIdentifier>,
    Option<Vec<LocalIdentifier>>,
);

/// セパレータなしの英数字混在文字列 (例: `rc1`, `dev1rc1`) を
/// 英字部と数値部の run へ分解して識別子列にする。
/// `rc1` → `[Alpha("rc"), Numeric(1)]` となり、`-rc.1` 形式と同値に比較できる。
fn decompose_alnum_runs(s: &str) -> Vec<PreIdentifier> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut cur_is_digit: Option<bool> = None;
    for ch in s.chars() {
        let is_digit = ch.is_ascii_digit();
        if cur_is_digit.is_some_and(|d| d != is_digit) {
            out.push(PreIdentifier::from_part(&cur));
            cur.clear();
        }
        cur_is_digit = Some(is_digit);
        cur.push(ch);
    }
    if !cur.is_empty() {
        out.push(PreIdentifier::from_part(&cur));
    }
    out
}

/// dot 区切りセグメントが既知の prerelease 識別子で始まるか判定する。
///
/// PEP 440 の `.devN` や Ruby 風 `7.0.0.alpha.2` を prerelease として取り込む一方、
/// Java の `5.0.0.RELEASE` / `4.0.0.Final` のような qualifier を prerelease と
/// 誤認しないため、既知識別子 (`PRERELEASE_IDENTIFIERS` と PEP 440 の `a`/`b`/`c`)
/// に限定する。
fn is_known_prerelease_segment(segment: &str) -> bool {
    let alpha_end = segment
        .find(|c: char| !c.is_ascii_alphabetic())
        .unwrap_or(segment.len());
    let head = segment[..alpha_end].to_ascii_lowercase();
    if head.is_empty() {
        return false;
    }
    matches!(head.as_str(), "a" | "b" | "c") || PRERELEASE_IDENTIFIERS.contains(&head.as_str())
}

/// 比較用の数値コア (`v` 接頭辞 / ビルドメタデータ / エポック / プレリリース /
/// qualifier を除いた `.` 区切り数値列) を抽出する。
///
/// 各セグメントは先頭の数値プレフィックスのみを取り (例: `0rc1` → 0)、
/// 数値が全く無いセグメント以降は無視する。`ChangeLevel::from_versions` と
/// `compare_versions` で同じ抽出規則を共有するための pub(crate) ヘルパー。
pub(crate) fn numeric_core(s: &str) -> Vec<NumericIdentifier> {
    parse_version_components(s).1
}

/// PEP 440 / semver を統合した比較用にバージョン文字列を構成要素へ分解する。
///
/// 戻り値は `(epoch, core, pre, post)`:
/// - `epoch`: PEP 440 エポック (`N!` 接頭辞)。なければ 0。
/// - `core`: 数値コア (例: `[1, 2, 3]`)。不足パートは比較時に 0 補完する。
/// - `pre`: プレリリース識別子列 (semver 11.4 準拠の構造化表現)。なければ `None`。
///   semver の `-rc.1` / セパレータなし PEP 440 の `rc1` / dot 区切りの `.dev1` を扱う。
/// - `post`: PEP 440 ポストリリース番号 (`.postN`)。なければ `None`。
///
/// 注意: `compare_versions` は全エコシステム共通のため、`1.0.0-1` のような
/// ハイフン + 純数字は semver の数値プレリリースと衝突しうる。誤判定を避けるため
/// post は曖昧さのない `.post` トークン形式のみを対象とする。
fn parse_version_components(
    s: &str,
) -> (
    NumericIdentifier,
    Vec<NumericIdentifier>,
    Option<Vec<PreIdentifier>>,
    Option<NumericIdentifier>,
) {
    parse_version_components_with_options(s, false)
}

fn parse_version_components_with_options(
    s: &str,
    numeric_hyphen_is_prerelease: bool,
) -> (
    NumericIdentifier,
    Vec<NumericIdentifier>,
    Option<Vec<PreIdentifier>>,
    Option<NumericIdentifier>,
) {
    // 先頭の 'v' または 'V' を除去
    let s = strip_ascii_v_prefix(s);
    // ビルドメタデータ (+...) を除去
    let s = s.split('+').next().unwrap_or(s);
    // PEP 440 エポック (`N!`) を切り出す。なければ 0。
    let (epoch, s) = match s.split_once('!') {
        Some((e, rest)) => (NumericIdentifier::parse(e).unwrap_or_default(), rest),
        None => (NumericIdentifier::default(), s),
    };
    // 数値コアとプレリリース部に分離する。最初の '-' 以前が数値コアの候補。
    let mut split = s.splitn(2, '-');
    let core = split.next().unwrap_or("");
    let hyphen_pre = split.next();

    let (nums, core_pre, post) = scan_core_segments(core);
    let pre = resolve_hyphen_prerelease(core_pre, hyphen_pre, numeric_hyphen_is_prerelease);

    (epoch, nums, pre, post)
}

/// 数値コアを '.' 区切りで走査し、数値列・コア内 prerelease・post を取り出す。
///
/// 各セグメントは先頭の数値部分のみを取り、英字が続くセグメントは種類に応じて解釈する:
///   - "post" で始まれば ポストリリース (例: "post1")
///   - 数字が先行していれば PEP 440 のセパレータなし prerelease (例: "0rc1" の "rc1")
///   - 既知の prerelease 識別子で始まれば dot 区切り prerelease
///     (例: "1.0.1.dev1" の "dev1"、Ruby 風 "7.0.0.alpha.2" の "alpha" 以降)
///   - それ以外の英字 qualifier (Java の RELEASE / Final 等) はそこで走査を終える
fn scan_core_segments(
    core: &str,
) -> (
    Vec<NumericIdentifier>,
    Option<Vec<PreIdentifier>>,
    Option<NumericIdentifier>,
) {
    let segments: Vec<&str> = core.split('.').collect();
    let mut nums = Vec::new();
    let mut core_pre: Option<Vec<PreIdentifier>> = None;
    let mut post: Option<NumericIdentifier> = None;
    for (idx, segment) in segments.iter().enumerate() {
        let digit_end = segment
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(segment.len());
        let (digits, rest) = segment.split_at(digit_end);
        if let Some(value) = NumericIdentifier::parse(digits) {
            nums.push(value);
        }
        if rest.is_empty() {
            continue;
        }
        // ポストリリース (例: ".post1" / "post1")。曖昧さがないため大文字小文字を許容。
        // "post" は ASCII なのでバイト列で比較する。`rest[..4]` の文字列スライスは
        // rest が多バイト文字を含むと byte index 4 が文字境界に一致せず panic するため、
        // 常に安全な as_bytes()[..4] を使う (rest.len() >= 4 でバイト長は保証済み)。
        if rest.len() >= 4 && rest.as_bytes()[..4].eq_ignore_ascii_case(b"post") {
            post = Some(trailing_number(rest).unwrap_or_default());
            break;
        }
        // 数字が先行する英字付きセグメント (例: "0rc1") は prerelease。
        // 英字部と数値部へ分解し `-rc.1` 形式と同値に比較できるようにする。
        if !digits.is_empty() {
            core_pre = Some(decompose_alnum_runs(rest));
            // PEP 440 では prerelease の後ろに `.postN` / `.devN` が続くケースがある
            // (例: `1.0a1.post1`)。post は後段の比較で別キーとして使うので
            // 取りこぼさないように後続セグメントを走査する。
            for follow in &segments[idx + 1..] {
                // rest 側と同様にバイト列比較で多バイト境界 panic を防ぐ。
                if follow.len() >= 4 && follow.as_bytes()[..4].eq_ignore_ascii_case(b"post") {
                    post = Some(trailing_number(follow).unwrap_or_default());
                    break;
                }
            }
            break;
        }
        // 英字開始セグメントが既知の prerelease 識別子 (dev / alpha / rc 等) なら、
        // このセグメント以降を prerelease 識別子列として取り込む
        // (PEP 440 の `.devN`、Ruby のドット区切り `7.0.0.alpha.2` に対応)。
        if is_known_prerelease_segment(rest) {
            let mut ids = Vec::new();
            for seg in &segments[idx..] {
                ids.extend(decompose_alnum_runs(seg));
            }
            core_pre = Some(ids);
            break;
        }
        // 純粋な英字 qualifier (RELEASE / Final 等) → 数値コアにもプレ/ポストにも
        // 含めず、位置ずれ比較を防ぐためここで走査を終える
        break;
    }
    (nums, core_pre, post)
}

/// プレリリース識別子列を確定する。
///
/// - 数値コア内で見つかった prerelease (`core_pre`) を優先する
/// - '-' 区切り表記 (`hyphen_pre`, 例: "canary-456" → [canary, 456], "rc.1" → [rc, 1])
///   は数値パース成功 → Numeric、失敗 → Alpha (小文字化) として構造化する
///
/// Java では純粋な数値サフィックスを安定版の追加パートとして扱う一方、SemVer
/// では数値だけでも prerelease になる。`numeric_hyphen_is_prerelease` で切り替える。
fn resolve_hyphen_prerelease(
    core_pre: Option<Vec<PreIdentifier>>,
    hyphen_pre: Option<&str>,
    numeric_hyphen_is_prerelease: bool,
) -> Option<Vec<PreIdentifier>> {
    if core_pre.is_some() {
        return core_pre;
    }
    hyphen_pre.and_then(|p| {
        if !p.is_empty()
            && (numeric_hyphen_is_prerelease
                || p.split('.')
                    .any(|part| NumericIdentifier::parse(part).is_none()))
        {
            Some(
                p.split(['.', '-'])
                    .filter(|part| !part.is_empty())
                    .map(PreIdentifier::from_part)
                    .collect(),
            )
        } else {
            None
        }
    })
}

fn parse_local_identifiers(local: &str) -> Option<Vec<LocalIdentifier>> {
    let ids: Vec<LocalIdentifier> = local
        .split(['.', '-', '_'])
        .filter(|part| !part.is_empty())
        .map(LocalIdentifier::from_part)
        .collect();

    if ids.is_empty() { None } else { Some(ids) }
}

fn parse_python_version_components(s: &str) -> PythonVersionComponents {
    let (public, local) = s
        .split_once('+')
        .map(|(public, local)| (public, parse_local_identifiers(local)))
        .unwrap_or((s, None));
    let (epoch, core, pre, post) = parse_version_components(public);

    (epoch, core, pre, post, local)
}

/// semver / PEP 440 風ルールでバージョン文字列を比較する。
///
/// 比較の優先順位:
/// 1. PEP 440 エポック (`N!`) が大きい方が新しい (例: `0!2.0 < 1!1.0`)
/// 2. 数値コア (不足パートは 0 として扱う。例: `"1.0" == "1.0.0"`)
/// 3. プレリリース有無: プレリリース付き (`1.0.0-rc.1` / `2.0.0rc1`) は
///    プレリリースなしより小さい (semver 11.4.3 / PEP 440)。
///    両方プレリリースなら識別子を左から順に構造化比較する (semver 11.4):
///    数値同士は数値比較 (`rc.1 < rc.2`)、Numeric < Alpha、Alpha 同士は
///    ASCII 辞書順 (`alpha < beta < rc`、特例で `dev` は最弱)。前方一致で
///    等しい場合は識別子数が少ない方が小さい (`alpha < alpha.1`)
/// 4. ポストリリース (`1.0.post1`) は対応する release より新しい (PEP 440)
///
/// ビルドメタデータ (`+` 以降) は semver 仕様に従い無視する。
pub fn compare_versions(a: &str, b: &str) -> std::cmp::Ordering {
    compare_core_pre_post(parse_version_components(a), parse_version_components(b))
}

/// SemVer 2.0.0 の優先順位で比較する。
///
/// Node.js / Rust / Go / Swift では `-1` も数値 prerelease である。通常は
/// `semver` クレートを使い、実装上限を超える任意桁の数値だけ独自比較へ戻す。
pub fn compare_semver_versions(a: &str, b: &str) -> std::cmp::Ordering {
    if let (Ok(a), Ok(b)) = (
        semver::Version::parse(strip_ascii_v_prefix(a)),
        semver::Version::parse(strip_ascii_v_prefix(b)),
    ) {
        return a.cmp_precedence(&b);
    }

    compare_core_pre_post(
        parse_version_components_with_options(a, true),
        parse_version_components_with_options(b, true),
    )
}

/// SemVer の prerelease かどうかを判定する。
pub(crate) fn is_semver_prerelease_version(version: &str) -> bool {
    let stripped = strip_ascii_v_prefix(version);
    if let Ok(parsed) = semver::Version::parse(stripped) {
        return !parsed.pre.is_empty();
    }

    let public = stripped.split('+').next().unwrap_or(stripped);
    public
        .split_once('-')
        .is_some_and(|(_, prerelease)| !prerelease.is_empty())
        || is_prerelease_version(version)
}

/// Composer の patch alias をポストリリースへ正規化して比較する。
pub(crate) fn compare_composer_versions(a: &str, b: &str) -> std::cmp::Ordering {
    fn normalize(value: &str) -> String {
        let Some((base, suffix)) = value.rsplit_once('-') else {
            return value.to_string();
        };
        let lower = suffix.to_ascii_lowercase();
        for prefix in ["patch", "pl", "p"] {
            if let Some(number) = lower.strip_prefix(prefix)
                && NumericIdentifier::parse(number).is_some()
            {
                return format!("{base}.post{number}");
            }
        }
        value.to_string()
    }

    compare_versions(&normalize(a), &normalize(b))
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum EcosystemPart {
    Numeric(NumericIdentifier),
    Text(String),
}

fn alphanumeric_parts(value: &str, ruby_hyphen: bool) -> Vec<EcosystemPart> {
    let normalized = if ruby_hyphen {
        value.replace('-', ".pre.")
    } else {
        value.to_string()
    };
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut digit_run = None;

    let flush =
        |current: &mut String, digit_run: &mut Option<bool>, parts: &mut Vec<EcosystemPart>| {
            if current.is_empty() {
                return;
            }
            if digit_run == &Some(true) {
                if let Some(number) = NumericIdentifier::parse(current) {
                    parts.push(EcosystemPart::Numeric(number));
                }
            } else {
                parts.push(EcosystemPart::Text(current.clone()));
            }
            current.clear();
            *digit_run = None;
        };

    for ch in normalized.chars() {
        if !ch.is_ascii_alphanumeric() {
            flush(&mut current, &mut digit_run, &mut parts);
            continue;
        }
        let is_digit = ch.is_ascii_digit();
        if digit_run.is_some_and(|previous| previous != is_digit) {
            flush(&mut current, &mut digit_run, &mut parts);
        }
        digit_run = Some(is_digit);
        current.push(ch);
    }
    flush(&mut current, &mut digit_run, &mut parts);
    parts
}

/// RubyGems の規則に従い、英字またはハイフンを含む版をプレリリースと判定する。
pub(crate) fn is_ruby_prerelease_version(version: &str) -> bool {
    let version = strip_ascii_v_prefix(version);
    version.contains('-') || version.bytes().any(|byte| byte.is_ascii_alphabetic())
}

/// RubyGems の `Gem::Version` と同じ数値・英字セグメント規則で比較する。
pub(crate) fn compare_ruby_versions(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;

    let a = alphanumeric_parts(a, true);
    let b = alphanumeric_parts(b, true);
    let zero = EcosystemPart::Numeric(NumericIdentifier::default());
    for index in 0..a.len().max(b.len()) {
        let a_part = a.get(index).unwrap_or(&zero);
        let b_part = b.get(index).unwrap_or(&zero);
        let ordering = match (a_part, b_part) {
            (EcosystemPart::Numeric(a), EcosystemPart::Numeric(b)) => a.cmp(b),
            (EcosystemPart::Text(a), EcosystemPart::Text(b)) => a.cmp(b),
            (EcosystemPart::Numeric(_), EcosystemPart::Text(_)) => Ordering::Greater,
            (EcosystemPart::Text(_), EcosystemPart::Numeric(_)) => Ordering::Less,
        };
        if ordering != Ordering::Equal {
            return ordering;
        }
    }
    Ordering::Equal
}

fn gradle_text_order(a: &str, b: &str) -> std::cmp::Ordering {
    fn special_rank(value: &str) -> Option<u8> {
        match value.to_ascii_lowercase().as_str() {
            "dev" => Some(0),
            "rc" => Some(2),
            "snapshot" => Some(3),
            "final" => Some(4),
            "ga" => Some(5),
            "release" => Some(6),
            "sp" => Some(7),
            _ => None,
        }
    }

    match (special_rank(a), special_rank(b)) {
        (Some(a), Some(b)) => a.cmp(&b),
        (Some(0), None) => std::cmp::Ordering::Less,
        (None, Some(0)) => std::cmp::Ordering::Greater,
        (Some(_), None) => std::cmp::Ordering::Greater,
        (None, Some(_)) => std::cmp::Ordering::Less,
        (None, None) => a.cmp(b),
    }
}

/// Gradle の公式 version ordering で比較する。
pub(crate) fn compare_gradle_versions(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;

    let a = alphanumeric_parts(a, false);
    let b = alphanumeric_parts(b, false);
    for (a_part, b_part) in a.iter().zip(&b) {
        let ordering = match (a_part, b_part) {
            (EcosystemPart::Numeric(a), EcosystemPart::Numeric(b)) => a.cmp(b),
            (EcosystemPart::Text(a), EcosystemPart::Text(b)) => gradle_text_order(a, b),
            (EcosystemPart::Numeric(_), EcosystemPart::Text(_)) => Ordering::Greater,
            (EcosystemPart::Text(_), EcosystemPart::Numeric(_)) => Ordering::Less,
        };
        if ordering != Ordering::Equal {
            return ordering;
        }
    }

    match (a.get(b.len()), b.get(a.len())) {
        (None, None) => Ordering::Equal,
        (Some(EcosystemPart::Numeric(_)), None) => Ordering::Greater,
        (Some(EcosystemPart::Text(_)), None) => Ordering::Less,
        (None, Some(EcosystemPart::Numeric(_))) => Ordering::Less,
        (None, Some(EcosystemPart::Text(_))) => Ordering::Greater,
        _ => Ordering::Equal,
    }
}

/// epoch → 数値コア → プレリリース → ポストリリースの順でバージョン成分を比較する。
///
/// semver (`compare_versions`) と PEP 440 (`compare_python_versions`) が共有する
/// 比較コア。PEP 440 側は戻り値が `Ordering::Equal` のときのみ local version 比較へ続ける。
fn compare_core_pre_post(
    a: (
        NumericIdentifier,
        Vec<NumericIdentifier>,
        Option<Vec<PreIdentifier>>,
        Option<NumericIdentifier>,
    ),
    b: (
        NumericIdentifier,
        Vec<NumericIdentifier>,
        Option<Vec<PreIdentifier>>,
        Option<NumericIdentifier>,
    ),
) -> std::cmp::Ordering {
    use std::cmp::Ordering;

    let (epoch_a, core_a, pre_a, post_a) = a;
    let (epoch_b, core_b, pre_b, post_b) = b;

    // 1. エポック比較
    match epoch_a.cmp(&epoch_b) {
        Ordering::Equal => {}
        other => return other,
    }

    // 2. 数値コア比較 (不足パートは 0 として扱う)
    let max_len = core_a.len().max(core_b.len());
    let zero = NumericIdentifier::default();
    for i in 0..max_len {
        let pa = core_a.get(i).unwrap_or(&zero);
        let pb = core_b.get(i).unwrap_or(&zero);
        match pa.cmp(pb) {
            Ordering::Equal => continue,
            other => return other,
        }
    }

    // 3. プレリリース比較: 片方のみ prerelease なら release/post > prerelease
    match (&pre_a, &pre_b) {
        (None, None) => {}
        (None, Some(_)) => return Ordering::Greater,
        (Some(_), None) => return Ordering::Less,
        (Some(a_ids), Some(b_ids)) => {
            // semver 11.4: 識別子を左から順に比較する
            for (pa, pb) in a_ids.iter().zip(b_ids.iter()) {
                match pa.cmp(pb) {
                    Ordering::Equal => continue,
                    other => return other,
                }
            }
            // 前方一致で等しい場合は識別子数が少ない方が小さい (semver 11.4.4)
            match a_ids.len().cmp(&b_ids.len()) {
                // プレリリース識別子が同一なら post まで踏み込んで比較する
                // PEP 440 では `1.0a1.post1 > 1.0a1`
                Ordering::Equal => {}
                other => return other,
            }
        }
    }

    // 4. ポストリリース比較: post 付きは対応する release より新しい
    match (post_a, post_b) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Less,
        (Some(_), None) => Ordering::Greater,
        (Some(a_post), Some(b_post)) => a_post.cmp(&b_post),
    }
}

/// PEP 440 の local version ordering まで含めて Python バージョンを比較する。
///
/// 共通の `compare_versions` は semver の build metadata として `+...` を無視する。
/// Python では `+...` が local version で、同じ public version より新しく扱うため、
/// Python 依存の最新候補選択ではこの関数を使う。
pub fn compare_python_versions(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;

    if let (Ok(a), Ok(b)) = (a.parse::<Pep440Version>(), b.parse::<Pep440Version>()) {
        return a.cmp(&b);
    }

    let (epoch_a, core_a, pre_a, post_a, local_a) = parse_python_version_components(a);
    let (epoch_b, core_b, pre_b, post_b, local_b) = parse_python_version_components(b);

    match compare_core_pre_post(
        (epoch_a, core_a, pre_a, post_a),
        (epoch_b, core_b, pre_b, post_b),
    ) {
        Ordering::Equal => {}
        other => return other,
    }

    match (&local_a, &local_b) {
        (None, None) => Ordering::Equal,
        (Some(_), None) => Ordering::Greater,
        (None, Some(_)) => Ordering::Less,
        (Some(a_ids), Some(b_ids)) => {
            for (pa, pb) in a_ids.iter().zip(b_ids.iter()) {
                match pa.cmp(pb) {
                    Ordering::Equal => continue,
                    other => return other,
                }
            }
            a_ids.len().cmp(&b_ids.len())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn numeric(value: &str) -> NumericIdentifier {
        NumericIdentifier::parse(value).expect("テスト用の数値文字列であること")
    }

    #[test]
    fn test_version_info_new() {
        let date = Utc.with_ymd_and_hms(2024, 1, 15, 10, 0, 0).unwrap();
        let info = VersionInfo::new("1.2.3", date);
        assert_eq!(info.version, "1.2.3");
        assert_eq!(info.released_at, date);
    }

    #[test]
    fn test_version_info_now() {
        let before = Utc::now();
        let info = VersionInfo::now("1.0.0");
        let after = Utc::now();

        assert_eq!(info.version, "1.0.0");
        assert!(info.released_at >= before);
        assert!(info.released_at <= after);
    }

    #[test]
    fn test_version_info_eq_consistent_with_ord() {
        // Eq と Ord がバージョン文字列のみで比較されること
        let date1 = Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();
        let date2 = Utc.with_ymd_and_hms(2025, 6, 15, 0, 0, 0).unwrap();
        let a = VersionInfo::new("1.0.0", date1);
        let b = VersionInfo::new("1.0.0", date2);

        // 同じバージョン文字列は released_at が異なっても等しい
        assert_eq!(a, b);
        assert_eq!(a.cmp(&b), std::cmp::Ordering::Equal);
    }

    #[test]
    fn test_compare_versions_multibyte_no_panic() {
        // 数値プレフィックスに続く多バイト文字を含むバージョン文字列でも
        // panic せず比較できること (バイト境界 panic の回帰防止)。
        // 以前は parse_version_components の `rest[..4]` / `follow[..4]` が
        // マルチバイト文字の途中を切って panic していた
        // (例: "0abcé" → rest="abcé" の byte index 4 は 'é' の内部)。
        use std::cmp::Ordering;
        // rest[..4] 経路: 数値コア末尾セグメントが digits + 多バイト。
        assert_eq!(compare_versions("1.0.0", "1.0.0.0abcé"), Ordering::Greater);
        assert_eq!(compare_versions("1.0.0.0abcé", "1.0.0"), Ordering::Less);
        // follow[..4] 経路: prerelease セグメントの後続に多バイトセグメント。
        let _ = compare_versions("1.0a1.abcé", "1.0");
        // Python 比較経路も同じ parse_version_components を通る。
        let _ = compare_python_versions("1.0.0.1xyzé", "1.0.0");
    }

    #[test]
    fn test_version_comparison_simple() {
        let v1 = VersionInfo::now("1.0.0");
        let v2 = VersionInfo::now("2.0.0");
        assert!(v1 < v2);
    }

    #[test]
    fn test_version_comparison_minor() {
        let v1 = VersionInfo::now("1.0.0");
        let v2 = VersionInfo::now("1.1.0");
        assert!(v1 < v2);
    }

    #[test]
    fn test_version_comparison_patch() {
        let v1 = VersionInfo::now("1.0.0");
        let v2 = VersionInfo::now("1.0.1");
        assert!(v1 < v2);
    }

    #[test]
    fn test_version_comparison_equal() {
        let v1 = VersionInfo::now("1.0.0");
        let v2 = VersionInfo::now("1.0.0");
        assert_eq!(v1.cmp(&v2), std::cmp::Ordering::Equal);
    }

    #[test]
    fn test_version_comparison_with_v_prefix() {
        let v1 = VersionInfo::now("v1.0.0");
        let v2 = VersionInfo::now("v2.0.0");
        assert!(v1 < v2);
    }

    #[test]
    fn test_version_comparison_mixed_prefix() {
        let v1 = VersionInfo::now("1.0.0");
        let v2 = VersionInfo::now("v1.0.0");
        // 等しいはず (v接頭辞は除去される)
        assert_eq!(v1.cmp(&v2), std::cmp::Ordering::Equal);
    }

    #[test]
    fn test_strip_ascii_v_prefix() {
        assert_eq!(strip_ascii_v_prefix("v1.2.3"), "1.2.3");
        assert_eq!(strip_ascii_v_prefix("V1.2.3"), "1.2.3");
        assert_eq!(strip_ascii_v_prefix("1.2.3"), "1.2.3");
        // 剥がすのは先頭の 1 文字だけ
        assert_eq!(strip_ascii_v_prefix("vv1.0"), "v1.0");
        // 接頭辞以外の位置の v / V は触らない
        assert_eq!(strip_ascii_v_prefix("1.0.0-v2"), "1.0.0-v2");
        assert_eq!(strip_ascii_v_prefix(""), "");
    }

    #[test]
    fn test_version_comparison_different_lengths() {
        let v1 = VersionInfo::now("1.0");
        let v2 = VersionInfo::now("1.0.0");
        // 1.0 は 1.0.0 と等価 (不足パートは0として扱う)
        assert_eq!(v1.cmp(&v2), std::cmp::Ordering::Equal);
    }

    #[test]
    fn test_version_comparison_semver_equivalence() {
        // 様々なsemver等価バージョンのテスト
        assert_eq!(
            compare_versions("0.15", "0.15.0"),
            std::cmp::Ordering::Equal
        );
        assert_eq!(compare_versions("1", "1.0"), std::cmp::Ordering::Equal);
        assert_eq!(compare_versions("1", "1.0.0"), std::cmp::Ordering::Equal);
        assert_eq!(
            compare_versions("2.0", "2.0.0.0"),
            std::cmp::Ordering::Equal
        );

        // 異なるバージョンは異なるままであるべき
        assert_eq!(compare_versions("0.15", "0.15.1"), std::cmp::Ordering::Less);
        assert_eq!(
            compare_versions("0.16", "0.15.1"),
            std::cmp::Ordering::Greater
        );
        assert_eq!(compare_versions("1", "1.0.1"), std::cmp::Ordering::Less);
        assert_eq!(compare_versions("2", "1.9.9"), std::cmp::Ordering::Greater);
    }

    #[test]
    fn test_version_comparison_prerelease() {
        // semver 11.4 準拠の構造化比較: 英字識別子は ASCII 辞書順で比較される
        // (以前は数値識別子しか見ておらず alpha == beta だったが、
        //  プレリリース利用者が beta → alpha へ実質ダウングレードされる不具合を修正)
        let v1 = VersionInfo::now("1.0.0-alpha");
        let v2 = VersionInfo::now("1.0.0-beta");
        assert_eq!(v1.cmp(&v2), std::cmp::Ordering::Less);
    }

    // 回帰テスト: PEP 440 prerelease + post の組み合わせ比較
    // 以前は `1.0a1.post1` を `1.0a1` と等価 (Ordering::Equal) と誤判定していたため、
    // α版に post が出てもユーザーが更新を取りこぼす不具合があった。
    #[test]
    fn test_compare_versions_pep440_prerelease_with_post() {
        // post 付きは対応する prerelease より新しい
        assert_eq!(
            compare_versions("1.0a1.post1", "1.0a1"),
            std::cmp::Ordering::Greater
        );
        assert_eq!(
            compare_versions("1.0a1", "1.0a1.post1"),
            std::cmp::Ordering::Less
        );
        // 異なる post 番号での比較
        assert_eq!(
            compare_versions("1.0a1.post2", "1.0a1.post1"),
            std::cmp::Ordering::Greater
        );
        // セパレータ付きハイフン形式 (`1.0-rc.1.post1`) でも同様に比較できる
        assert_eq!(
            compare_versions("1.0-rc.1.post1", "1.0-rc.1"),
            std::cmp::Ordering::Greater
        );
        // 既存の挙動: post 付きはリリースより新しい (回帰防止)
        assert_eq!(
            compare_versions("1.0.post1", "1.0"),
            std::cmp::Ordering::Greater
        );
    }

    #[test]
    fn test_compare_versions_basic() {
        assert_eq!(
            compare_versions("1.0.0", "1.0.0"),
            std::cmp::Ordering::Equal
        );
        assert_eq!(compare_versions("1.0.0", "2.0.0"), std::cmp::Ordering::Less);
        assert_eq!(
            compare_versions("2.0.0", "1.0.0"),
            std::cmp::Ordering::Greater
        );
    }

    #[test]
    fn test_compare_versions_multi_digit() {
        assert!(compare_versions("1.9.0", "1.10.0") == std::cmp::Ordering::Less);
        assert!(compare_versions("10.0.0", "9.0.0") == std::cmp::Ordering::Greater);
    }

    #[test]
    fn test_serde_version_info() {
        let date = Utc.with_ymd_and_hms(2024, 1, 15, 10, 0, 0).unwrap();
        let info = VersionInfo::new("1.2.3", date);

        let json = serde_json::to_string(&info).unwrap();
        let parsed: VersionInfo = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.version, "1.2.3");
        assert_eq!(parsed.released_at, date);
    }

    #[test]
    fn test_version_info_clone() {
        let date = Utc.with_ymd_and_hms(2024, 1, 15, 10, 0, 0).unwrap();
        let info = VersionInfo::new("1.2.3", date);
        let cloned = info.clone();

        assert_eq!(info, cloned);
    }

    #[test]
    fn test_version_sorting() {
        let mut versions = [
            VersionInfo::now("2.0.0"),
            VersionInfo::now("1.0.0"),
            VersionInfo::now("1.5.0"),
            VersionInfo::now("1.0.1"),
        ];

        versions.sort();

        assert_eq!(versions[0].version, "1.0.0");
        assert_eq!(versions[1].version, "1.0.1");
        assert_eq!(versions[2].version, "1.5.0");
        assert_eq!(versions[3].version, "2.0.0");
    }

    #[test]
    fn test_find_max_version() {
        let versions = [
            VersionInfo::now("1.0.0"),
            VersionInfo::now("2.5.0"),
            VersionInfo::now("2.0.0"),
            VersionInfo::now("1.9.9"),
        ];

        let max = versions.iter().max().unwrap();
        assert_eq!(max.version, "2.5.0");
    }

    #[test]
    fn test_is_prerelease_stable_versions() {
        // 安定版バージョンはプレリリースとして検出されてはいけない
        assert!(!is_prerelease_version("1.0.0"));
        assert!(!is_prerelease_version("2.5.3"));
        assert!(!is_prerelease_version("v1.0.0"));
        assert!(!is_prerelease_version("10.20.30"));
    }

    #[test]
    fn test_is_prerelease_alpha_beta_rc() {
        assert!(is_prerelease_version("1.0.0-alpha"));
        assert!(is_prerelease_version("1.0.0-alpha.1"));
        assert!(is_prerelease_version("1.0.0-beta"));
        assert!(is_prerelease_version("1.0.0-beta.2"));
        assert!(is_prerelease_version("1.0.0-rc.1"));
        assert!(is_prerelease_version("2.0.0-RC1"));
    }

    #[test]
    fn test_is_prerelease_canary_dev() {
        // React風のcanaryバージョン
        assert!(is_prerelease_version("19.3.0-canary-52684925-20251110"));
        // TypeScript風のdevバージョン
        assert!(is_prerelease_version("6.0.0-dev.20260103"));
        // Vite風のbetaバージョン
        assert!(is_prerelease_version("8.0.0-beta.5"));
        // Prettier風のalpha
        assert!(is_prerelease_version("4.0.0-alpha.13"));
    }

    #[test]
    fn test_is_prerelease_other_identifiers() {
        assert!(is_prerelease_version("1.0.0-preview"));
        assert!(is_prerelease_version("1.0.0-next"));
        assert!(is_prerelease_version("1.0.0-nightly"));
        assert!(is_prerelease_version("1.0.0-snapshot"));
        assert!(is_prerelease_version("1.0.0-pre.1"));
        assert!(is_prerelease_version("1.0.0-insiders"));
        assert!(is_prerelease_version("1.0.0-experimental"));
    }

    /// 回帰テスト: JVM 系の milestone 版が安定版として扱われ、`3.24.2` の利用者が
    /// `4.0.0-M1` へ、`5.10.0` が `5.13.0-M3` へ、`5.3.23` が `7.0.0-M6` へ
    /// 更新されていた (Java/PHP は semver 判定ではなく識別子リストを使うため)。
    #[test]
    fn test_is_prerelease_jvm_milestone() {
        // ハイフン区切り (JUnit / AssertJ / Micronaut)
        assert!(is_prerelease_version("4.0.0-M1"));
        assert!(is_prerelease_version("5.13.0-M3"));
        assert!(is_prerelease_version("7.0.0-M6"));
        // ドット区切り (旧 Spring Boot の `2.0.0.M1` 表記)
        assert!(is_prerelease_version("2.0.0.M1"));
        assert!(is_prerelease_version("1.5.0.M7"));
        // 小文字・複数桁
        assert!(is_prerelease_version("3.0.0-m12"));
        // 綴りきりの milestone
        assert!(is_prerelease_version("1.0.0-milestone1"));
        assert!(is_prerelease_version("1.0.0-milestone"));
    }

    /// milestone 判定が JVM の「安定版 qualifier」を巻き込まないこと。
    /// Java で「qualifier があれば一律プレリリース」にすると、これらが全滅する。
    #[test]
    fn test_is_prerelease_jvm_stable_qualifiers_not_matched() {
        assert!(!is_prerelease_version("4.1.100.Final"));
        assert!(!is_prerelease_version("31.1-jre"));
        assert!(!is_prerelease_version("33.4.8-jre"));
        assert!(!is_prerelease_version("31.1-android"));
        assert!(!is_prerelease_version("5.3.23.RELEASE"));
        assert!(!is_prerelease_version("1.0.0.GA"));
        assert!(!is_prerelease_version("1.0.0-SP1"));
        // `m` の直後が数字でないトークンは milestone ではない
        assert!(!is_prerelease_version("1.0.0-macos1"));
        assert!(!is_prerelease_version("1.0.0-m2m"));
        assert!(!is_prerelease_version("1.0.0-mysql8"));
        // 数値のみのセグメントは当然対象外
        assert!(!is_prerelease_version("20030203.000550"));
    }

    /// 回帰テスト: milestone 判定は `token[1..]` でバイトスライスするため、
    /// 多バイト UTF-8 の文字境界を割らないこと。過去に同じスライスパターンで
    /// 非 ASCII のバージョン文字列 1 件がプロセス全体をクラッシュさせた事例がある。
    #[test]
    fn test_is_prerelease_multibyte_does_not_panic() {
        for version in [
            "1.0.0-mé",
            "1.0.0-m1é",
            "1.0.0.0abcé",
            "1.0.0-日本語",
            "m",
            "mé",
            "1.0.0-m",
            "é",
            "",
        ] {
            // panic しないことが本質 (戻り値は問わない)
            let _ = is_prerelease_version(version);
        }
        // 多バイト文字が続くトークンは milestone ではない
        assert!(!is_prerelease_version("1.0.0-mé"));
    }

    #[test]
    fn test_is_prerelease_python_pep440_style() {
        // Python/PEP 440 形式: 数字 + a/b/c + 数字
        // アルファリリース
        assert!(is_prerelease_version("26.1a1"));
        assert!(is_prerelease_version("18.3a0"));
        assert!(is_prerelease_version("1.0a1"));
        // ベータリリース
        assert!(is_prerelease_version("21.12b0"));
        assert!(is_prerelease_version("21.11b1"));
        assert!(is_prerelease_version("1.0b2"));
        // リリース候補 ('c' 使用)
        assert!(is_prerelease_version("1.0c1"));
        assert!(is_prerelease_version("2.5c0"));
        // 安定版バージョンはマッチしないべき
        assert!(!is_prerelease_version("25.12.0"));
        assert!(!is_prerelease_version("1.2.3"));
        assert!(!is_prerelease_version("2024.1.1"));
    }

    #[test]
    fn test_version_info_is_prerelease() {
        let stable = VersionInfo::now("1.0.0");
        assert!(!stable.is_prerelease());

        let canary = VersionInfo::now("19.3.0-canary-52684925-20251110");
        assert!(canary.is_prerelease());

        let beta = VersionInfo::now("8.0.0-beta.5");
        assert!(beta.is_prerelease());
    }

    #[test]
    fn test_compare_versions_ignores_build_metadata() {
        // semver ビルドメタデータ (+...) はバージョン優先度に影響しないべき
        assert_eq!(
            compare_versions("1.0.0", "1.0.0+spec-1.1.0"),
            std::cmp::Ordering::Equal
        );
        assert_eq!(
            compare_versions("1.0.0+spec-1.1.0", "1.0.0"),
            std::cmp::Ordering::Equal
        );
        assert_eq!(
            compare_versions("1.0.0+build.1", "1.0.0+build.2"),
            std::cmp::Ordering::Equal
        );
        // 実際のバージョン差は引き続き機能するべき
        assert_eq!(
            compare_versions("1.0.0+build", "1.0.1"),
            std::cmp::Ordering::Less
        );
    }

    #[test]
    fn test_compare_python_versions_local_version_ordering() {
        use std::cmp::Ordering;

        // PEP 440 local version は同じ public version より新しい
        assert_eq!(
            compare_python_versions("1.0+local", "1.0"),
            Ordering::Greater
        );
        // public version の post release は対応する local final release より新しい
        assert_eq!(
            compare_python_versions("1.0+local", "1.0.post1"),
            Ordering::Less
        );
        // 数値 local セグメントは英字 local セグメントより大きい
        assert_eq!(
            compare_python_versions("1.0+1", "1.0+abc"),
            Ordering::Greater
        );
        // local セグメントは小文字化して比較し、前方一致なら長い方が大きい
        assert_eq!(
            compare_python_versions("1.0+ABC.1", "1.0+abc"),
            Ordering::Greater
        );
        assert_eq!(
            compare_python_versions("1.0+abc.2", "1.0+abc.1"),
            Ordering::Greater
        );
        assert_eq!(
            compare_python_versions("1.0rc1+abc.2", "1.0rc1+abc.1"),
            Ordering::Greater
        );
    }

    #[test]
    fn test_compare_python_versions_pep440_normalization_and_suffix_ordering() {
        use std::cmp::Ordering;

        assert_eq!(
            compare_python_versions("1.0_alpha1", "1.0a1"),
            Ordering::Equal
        );
        assert_eq!(
            compare_python_versions("1.0alpha1", "1.0a2"),
            Ordering::Less
        );
        assert_eq!(compare_python_versions("1.0-1", "1.0"), Ordering::Greater);
        assert_eq!(
            compare_python_versions("1.0a1.dev1", "1.0a1"),
            Ordering::Less
        );
        assert_eq!(
            compare_python_versions("1.0preview1", "1.0rc1"),
            Ordering::Equal
        );
    }

    #[test]
    fn test_python_prerelease_detection_accepts_pep440_alternative_spellings() {
        assert!(is_python_prerelease_version("1.0_alpha1"));
        assert!(is_python_prerelease_version("1.0preview1"));
        assert!(is_python_prerelease_version("1.0_dev"));
        assert!(!is_python_prerelease_version("1.0-r4"));
    }

    #[test]
    fn test_compare_versions_four_part_versions() {
        // 一部のエコシステムは4パートバージョンを使用 (例: Java SNAPSHOT, .NET)
        assert_eq!(
            compare_versions("1.0.0.0", "1.0.0.1"),
            std::cmp::Ordering::Less
        );
        assert_eq!(
            compare_versions("1.0.0.1", "1.0.0.0"),
            std::cmp::Ordering::Greater
        );
        assert_eq!(
            compare_versions("1.0.0.0", "1.0.0.0"),
            std::cmp::Ordering::Equal
        );
    }

    #[test]
    fn test_compare_versions_large_numbers() {
        // CalVer形式の大きなバージョン番号
        assert_eq!(
            compare_versions("2024.1.1", "2025.1.1"),
            std::cmp::Ordering::Less
        );
        assert_eq!(
            compare_versions("2025.12.31", "2025.12.31"),
            std::cmp::Ordering::Equal
        );
        assert_eq!(
            compare_versions("20260226", "20260227"),
            std::cmp::Ordering::Less
        );
    }

    #[test]
    fn test_compare_versions_numbers_larger_than_u64() {
        use std::cmp::Ordering;

        assert_eq!(
            compare_versions("18446744073709551616.0.0", "2.0.0"),
            Ordering::Greater
        );
        assert_eq!(
            compare_versions(
                "1.0.0-rc.18446744073709551616",
                "1.0.0-rc.9223372036854775808"
            ),
            Ordering::Greater
        );
        assert_eq!(
            compare_versions("18446744073709551616!1.0", "2!999.0"),
            Ordering::Greater
        );
        assert_eq!(
            compare_versions(
                "1.0.post18446744073709551616",
                "1.0.post9223372036854775808"
            ),
            Ordering::Greater
        );
    }

    #[test]
    fn test_is_prerelease_false_positives_avoided() {
        // プレリリースに似た部分文字列を含むがプレリリースではないバージョン
        // "1.0.0" にはプレリリース識別子が含まれない
        assert!(!is_prerelease_version("1.0.0"));
        // ハイフン後が数値のみのバージョン
        assert!(!is_prerelease_version("1.0.0-1"));
        // CalVer日付はプレリリースをトリガーしないべき
        assert!(!is_prerelease_version("2024.1.15"));
        assert!(!is_prerelease_version("25.12.0"));
    }

    #[test]
    fn test_is_prerelease_pep440_edge_cases() {
        // ポストリリース (PEP 440) - プレリリースではない
        // 注: 現在の実装はポストリリースを数字+文字+数字パターンで特別処理する;
        // 'p' は a/b/c ではないためマッチしないべき
        assert!(!is_prerelease_version("1.0.0.post1"));
        // dev0 はプレリリース ("dev" を含む)
        assert!(is_prerelease_version("1.0.0.dev0"));
        // 複合: dev + rc
        assert!(is_prerelease_version("1.0.0.dev1rc1"));
    }

    #[test]
    fn test_compare_versions_single_component() {
        // 単一コンポーネントバージョン (例: Rust crate "1")
        assert_eq!(compare_versions("1", "2"), std::cmp::Ordering::Less);
        assert_eq!(compare_versions("10", "9"), std::cmp::Ordering::Greater);
        assert_eq!(compare_versions("1", "1"), std::cmp::Ordering::Equal);
    }

    #[test]
    fn test_version_info_ordering_consistency() {
        // Ord/PartialOrd の一貫性検証
        let v1 = VersionInfo::now("1.0.0");
        let v2 = VersionInfo::now("1.0.0");
        let v3 = VersionInfo::now("2.0.0");

        // 反射的
        assert_eq!(v1.cmp(&v2), std::cmp::Ordering::Equal);
        // 反対称
        assert_eq!(v1.cmp(&v3), std::cmp::Ordering::Less);
        assert_eq!(v3.cmp(&v1), std::cmp::Ordering::Greater);
        // PartialOrd は Ord と一致する
        assert_eq!(v1.partial_cmp(&v3), Some(std::cmp::Ordering::Less));
    }

    #[test]
    fn test_compare_versions_empty_string() {
        // 空文字列は 0 として扱われる
        assert_eq!(compare_versions("", ""), std::cmp::Ordering::Equal);
        assert_eq!(compare_versions("", "1.0.0"), std::cmp::Ordering::Less);
    }

    #[test]
    fn test_compare_versions_qualifier_suffix() {
        // Java 風の qualifier (RELEASE, Final) は非数値部で終了する
        assert_eq!(
            compare_versions("5.0.0", "5.0.0.RELEASE"),
            std::cmp::Ordering::Equal
        );
    }

    #[test]
    fn test_is_prerelease_java_snapshot() {
        assert!(is_prerelease_version("1.0.0-SNAPSHOT"));
    }

    #[test]
    fn test_is_prerelease_release_suffix_not_prerelease() {
        // RELEASE サフィックスはプレリリースではない
        assert!(!is_prerelease_version("5.0.0.RELEASE"));
        assert!(!is_prerelease_version("4.0.0.Final"));
    }

    #[test]
    fn test_is_prerelease_case_insensitive() {
        // 大文字小文字混在でもプレリリースとして検出される
        assert!(is_prerelease_version("1.0.0-ALPHA"));
        assert!(is_prerelease_version("1.0.0-Beta.1"));
        assert!(is_prerelease_version("1.0.0-RC1"));
        assert!(is_prerelease_version("1.0.0-CANARY"));
    }

    #[test]
    fn test_is_prerelease_non_version_strings() {
        // バージョン表記ではない文字列の処理
        assert!(!is_prerelease_version("hello"));
        assert!(!is_prerelease_version(""));
        assert!(!is_prerelease_version("abc"));
        // "development" は "dev" を部分文字列として含むが、
        // 単語境界チェックにより誤検出されない
        assert!(!is_prerelease_version("development"));
    }

    #[test]
    fn test_compare_versions_v_prefix_mixed() {
        // v/V 接頭辞が混在していても正しく比較できる
        assert_eq!(
            compare_versions("v1.0.0", "V1.0.0"),
            std::cmp::Ordering::Equal
        );
        assert_eq!(
            compare_versions("v2.0.0", "1.0.0"),
            std::cmp::Ordering::Greater
        );
        assert_eq!(
            compare_versions("1.0.0", "V2.0.0"),
            std::cmp::Ordering::Less
        );
    }

    #[test]
    fn test_contains_identifier_word_basic() {
        // 基本的な単語境界マッチ
        assert!(contains_identifier_word("1.0.0-alpha", "alpha"));
        assert!(contains_identifier_word("1.0.0-dev.1", "dev"));
        assert!(contains_identifier_word("1.0.0+pre.1", "pre"));
        assert!(contains_identifier_word("alpha-1.0.0", "alpha"));
    }

    #[test]
    fn test_contains_identifier_word_false_positives_prevented() {
        // 部分文字列マッチによる誤検出が防止される
        assert!(!contains_identifier_word("1.0.0-enterprise", "pre"));
        assert!(!contains_identifier_word("1.0.0-deprecated", "pre"));
        assert!(!contains_identifier_word("1.0.0-spread", "pre"));
        assert!(!contains_identifier_word("development", "dev"));
        assert!(!contains_identifier_word("1.0.0-nextcloud", "next"));
        assert!(!contains_identifier_word("salpha", "alpha"));
        assert!(!contains_identifier_word("preemptive", "pre"));
    }

    #[test]
    fn test_contains_identifier_word_separators() {
        // 各種セパレータで区切られた場合にマッチする
        assert!(contains_identifier_word("1.0.0-dev", "dev")); // ハイフン
        assert!(contains_identifier_word("1.0.0.dev", "dev")); // ドット
        assert!(contains_identifier_word("1.0.0+dev", "dev")); // プラス
        assert!(contains_identifier_word("1.0.0_dev", "dev")); // アンダースコア
        assert!(contains_identifier_word("dev", "dev")); // 文字列全体
    }

    #[test]
    fn test_contains_identifier_word_digit_boundary() {
        // 識別子の後に数字が続く場合もマッチする (例: alpha1)
        assert!(contains_identifier_word("1.0.0-alpha1", "alpha"));
        assert!(contains_identifier_word("1.0.0-beta2", "beta"));
        assert!(contains_identifier_word("1.0.0-rc1", "rc"));
        assert!(contains_identifier_word("1.0.0-dev0", "dev"));
    }

    #[test]
    fn test_contains_identifier_word_edge_cases() {
        // 空文字列や境界ケース
        assert!(!contains_identifier_word("", "dev"));
        assert!(!contains_identifier_word("abc", "development"));
        assert!(contains_identifier_word("dev", "dev"));
        assert!(!contains_identifier_word("d", "dev"));
    }

    #[test]
    fn test_is_prerelease_word_boundary_regression() {
        // Bug回帰テスト: 部分文字列マッチによる誤検出が修正されている
        // "enterprise" は "pre" を部分文字列として含むがプレリリースではない
        assert!(!is_prerelease_version("1.0.0-enterprise"));
        // これらは正しくプレリリースと判定される
        assert!(is_prerelease_version("1.0.0-pre"));
        assert!(is_prerelease_version("1.0.0-pre.1"));
        assert!(is_prerelease_version("1.0.0-pre1"));
        // "dev" の境界チェック
        assert!(!is_prerelease_version("1.0.0-devtools"));
        assert!(is_prerelease_version("1.0.0-dev"));
        assert!(is_prerelease_version("1.0.0-dev.1"));
        assert!(is_prerelease_version("1.0.0-dev0"));
    }

    /// バグ回帰テスト: semver 仕様に従い、数値コアが等しい場合
    /// プレリリース付きは安定版より小さく扱われる。
    /// 以前は `1.0.0-rc.1 == 1.0.0` と判定されていたため、
    /// `pick_older_within_age` が安定版を候補としてスキップしてしまう不具合があった。
    #[test]
    fn test_compare_versions_prerelease_is_less_than_stable() {
        assert_eq!(
            compare_versions("1.0.0-rc.1", "1.0.0"),
            std::cmp::Ordering::Less
        );
        assert_eq!(
            compare_versions("1.0.0", "1.0.0-rc.1"),
            std::cmp::Ordering::Greater
        );
        assert_eq!(
            compare_versions("2.0.0-beta.2", "2.0.0"),
            std::cmp::Ordering::Less
        );
        assert_eq!(
            compare_versions("1.0.0-alpha", "1.0.0"),
            std::cmp::Ordering::Less
        );
        // 異なる数値コアならプレリリースの有無に関わらず数値が優先される
        assert_eq!(
            compare_versions("1.0.0", "0.9.0-rc.1"),
            std::cmp::Ordering::Greater
        );
        assert_eq!(
            compare_versions("1.0.1", "1.0.0-rc.1"),
            std::cmp::Ordering::Greater
        );
    }

    #[test]
    fn test_compare_versions_both_prerelease_numeric_identifier_ordered() {
        // 両方ともプレリリースの場合、プレリリース部の数値識別子で順序付けする
        // 識別子内の数値は数値順で比較する（canary-123 < canary-456、rc.1 < rc.2）
        assert_eq!(
            compare_versions("1.0.0-rc.1", "1.0.0-rc.2"),
            std::cmp::Ordering::Less
        );
        assert_eq!(
            compare_versions("19.3.0-canary-123", "19.3.0-canary-456"),
            std::cmp::Ordering::Less
        );
        // 英字識別子は ASCII 辞書順で順序付けされる (semver 11.4.4)
        // (以前は数値しか見ておらず alpha == beta と判定されていた)
        assert_eq!(
            compare_versions("1.0.0-alpha", "1.0.0-beta"),
            std::cmp::Ordering::Less
        );
    }

    #[test]
    fn test_compare_versions_prerelease_with_build_metadata() {
        // build metadata はバージョン比較で無視されるが、prerelease 判定は維持される
        assert_eq!(
            compare_versions("1.0.0-rc.1+build", "1.0.0"),
            std::cmp::Ordering::Less
        );
        assert_eq!(
            compare_versions("1.0.0+build", "1.0.0-rc.1"),
            std::cmp::Ordering::Greater
        );
    }

    #[test]
    fn test_is_prerelease_deprecation_markers() {
        // 作者が「更新非推奨」を示すために付けたマーカーは prerelease 扱いで
        // デフォルト更新対象から外す (例: `serde_yaml 0.9.34-deprecated`)
        assert!(is_prerelease_version("0.9.34-deprecated"));
        assert!(is_prerelease_version("1.0.0-DEPRECATED"));
        assert!(is_prerelease_version("1.0.0-obsolete"));
        assert!(is_prerelease_version("1.0.0-retired"));
        assert!(is_prerelease_version("1.0.0-yanked"));
        assert!(is_prerelease_version("1.0.0-unmaintained"));
        // 単語境界チェック: "deprecated" を部分文字列として含む別の語は除外しない
        assert!(!is_prerelease_version("1.0.0-undeprecated"));
    }

    #[test]
    fn test_is_prerelease_pep440_rc_without_separator() {
        // 回帰テスト: PEP 440 のセパレータなし rc 表記 (X.Y.Zrc1) を prerelease と判定する。
        // 以前は "数字+a/b/c+数字" しか見ておらず "rc" の先頭 'r' を取りこぼし、
        // 安定版ユーザーが rc 版へ誤更新されていた。
        assert!(is_prerelease_version("2.0.0rc1"));
        assert!(is_prerelease_version("1.0rc1"));
        assert!(is_prerelease_version("21.0rc2"));
        assert!(is_prerelease_version("3.0.0RC1")); // 大文字でも検出する
        // セパレータなしの a/b も従来どおり検出する
        assert!(is_prerelease_version("2.0.0a1"));
        assert!(is_prerelease_version("2.0.0b1"));
        // 安定版を rc 表記と誤検出しない
        assert!(!is_prerelease_version("2.0.0"));
        assert!(!is_prerelease_version("1.2.3"));
    }

    #[test]
    fn test_compare_versions_pep440_rc_without_separator_is_less_than_stable() {
        // 回帰テスト: セパレータなし rc は対応する安定版より小さい (semver / PEP 440)
        assert_eq!(
            compare_versions("2.0.0rc1", "2.0.0"),
            std::cmp::Ordering::Less
        );
        assert_eq!(
            compare_versions("2.0.0", "2.0.0rc1"),
            std::cmp::Ordering::Greater
        );
        assert_eq!(compare_versions("1.0rc1", "1.0"), std::cmp::Ordering::Less);
        // セパレータなし a/b も同様
        assert_eq!(
            compare_versions("1.0.0a1", "1.0.0"),
            std::cmp::Ordering::Less
        );
        // 数値コアが異なる場合はコアが優先される
        assert_eq!(
            compare_versions("2.0.0rc1", "1.9.0"),
            std::cmp::Ordering::Greater
        );
    }

    #[test]
    fn test_compare_versions_pep440_rc_ordering() {
        // セパレータなし rc 同士は末尾の数値識別子で順序付けする
        assert_eq!(
            compare_versions("2.0.0rc1", "2.0.0rc2"),
            std::cmp::Ordering::Less
        );
        assert_eq!(
            compare_versions("2.0.0rc2", "2.0.0rc1"),
            std::cmp::Ordering::Greater
        );
        assert_eq!(
            compare_versions("2.0.0rc1", "2.0.0rc1"),
            std::cmp::Ordering::Equal
        );
    }

    #[test]
    fn test_trailing_number_extraction() {
        assert_eq!(super::trailing_number("rc1"), Some(numeric("1")));
        assert_eq!(super::trailing_number("rc12"), Some(numeric("12")));
        assert_eq!(super::trailing_number("a0"), Some(numeric("0")));
        assert_eq!(super::trailing_number("rc"), None);
        assert_eq!(super::trailing_number(""), None);
    }

    #[test]
    fn test_compare_versions_qualifier_suffix_still_equal() {
        // 回帰テスト: 英字のみで始まる Java qualifier は数値コアに影響せず安定版と等価のまま
        // (embedded prerelease 導入で壊れていないことを確認)
        assert_eq!(
            compare_versions("5.0.0", "5.0.0.RELEASE"),
            std::cmp::Ordering::Equal
        );
        assert_eq!(
            compare_versions("4.0.0.Final", "4.0.0"),
            std::cmp::Ordering::Equal
        );
    }

    #[test]
    fn test_compare_versions_pep440_post_release_is_greater_than_release() {
        // PEP 440: ポストリリースは対応する release より新しい
        assert_eq!(
            compare_versions("1.0.post1", "1.0"),
            std::cmp::Ordering::Greater
        );
        assert_eq!(
            compare_versions("1.0", "1.0.post1"),
            std::cmp::Ordering::Less
        );
        assert_eq!(
            compare_versions("1.2.3.post2", "1.2.3"),
            std::cmp::Ordering::Greater
        );
        // post は次の release より小さい
        assert_eq!(
            compare_versions("1.0.post1", "1.1"),
            std::cmp::Ordering::Less
        );
        // post 同士は post 番号で比較する
        assert_eq!(
            compare_versions("1.0.post1", "1.0.post2"),
            std::cmp::Ordering::Less
        );
        // post は prerelease より新しい (prerelease < release < post)
        assert_eq!(
            compare_versions("1.0rc1", "1.0.post1"),
            std::cmp::Ordering::Less
        );
    }

    #[test]
    fn test_compare_versions_pep440_epoch() {
        // PEP 440: エポックが大きい方が常に新しい
        assert_eq!(
            compare_versions("1!2.0", "2.0"),
            std::cmp::Ordering::Greater
        );
        assert_eq!(
            compare_versions("1!1.0", "9.9"),
            std::cmp::Ordering::Greater
        );
        assert_eq!(compare_versions("2.0", "1!2.0"), std::cmp::Ordering::Less);
        // 同一エポック内は数値コアで比較する
        assert_eq!(compare_versions("1!2.0", "1!2.1"), std::cmp::Ordering::Less);
        // エポックなし同士は従来どおり
        assert_eq!(compare_versions("2.0", "2.0"), std::cmp::Ordering::Equal);
    }

    #[test]
    fn test_parse_version_components_breakdown() {
        use super::PreIdentifier::{Alpha, Numeric};
        // 構成要素分解の単体確認
        assert_eq!(
            super::parse_version_components("1.2.3"),
            (
                numeric("0"),
                vec![numeric("1"), numeric("2"), numeric("3")],
                None,
                None
            )
        );
        // セパレータなし prerelease は英字部と数値部へ分解される
        assert_eq!(
            super::parse_version_components("2.0.0rc1"),
            (
                numeric("0"),
                vec![numeric("2"), numeric("0"), numeric("0")],
                Some(vec![Alpha("rc".to_string()), Numeric(numeric("1"))]),
                None
            )
        );
        assert_eq!(
            super::parse_version_components("1.0.post2"),
            (
                numeric("0"),
                vec![numeric("1"), numeric("0")],
                None,
                Some(numeric("2"))
            )
        );
        assert_eq!(
            super::parse_version_components("1!2.3"),
            (numeric("1"), vec![numeric("2"), numeric("3")], None, None)
        );
        // RELEASE qualifier は数値コアにもステージにも影響しない
        assert_eq!(
            super::parse_version_components("5.0.0.RELEASE"),
            (
                numeric("0"),
                vec![numeric("5"), numeric("0"), numeric("0")],
                None,
                None
            )
        );
        // ハイフン区切り prerelease は識別子ごとに Numeric / Alpha (小文字化) へ構造化される
        assert_eq!(
            super::parse_version_components("1.0.0-RC.1"),
            (
                numeric("0"),
                vec![numeric("1"), numeric("0"), numeric("0")],
                Some(vec![Alpha("rc".to_string()), Numeric(numeric("1"))]),
                None
            )
        );
        // dot 区切りの既知 prerelease 識別子 (.devN) も prerelease として取り込む
        assert_eq!(
            super::parse_version_components("1.0.1.dev1"),
            (
                numeric("0"),
                vec![numeric("1"), numeric("0"), numeric("1")],
                Some(vec![Alpha("dev".to_string()), Numeric(numeric("1"))]),
                None
            )
        );
    }

    /// characterization テスト: 分解ロジックのリファクタで挙動が変わらないことを
    /// 固定する。前処理 (v/V・ビルドメタデータ・エポック)、コアセグメント走査の
    /// 各分岐 (post / セパレータなし prerelease + 後続 post / dot 区切り prerelease /
    /// qualifier)、多バイト境界の panic セーフを component レベルで検証する。
    #[test]
    fn test_parse_version_components_preprocessing_and_scan_branches() {
        use super::PreIdentifier::{Alpha, Numeric};
        // v/V 接頭辞とビルドメタデータ (+...) は分解前に除去される
        assert_eq!(
            super::parse_version_components("V1.2.3+build.5"),
            (
                numeric("0"),
                vec![numeric("1"), numeric("2"), numeric("3")],
                None,
                None
            )
        );
        // post は大文字小文字を区別しない
        assert_eq!(
            super::parse_version_components("1.0.POST2"),
            (
                numeric("0"),
                vec![numeric("1"), numeric("0")],
                None,
                Some(numeric("2"))
            )
        );
        // セパレータなし prerelease の後続セグメントから post を取りこぼさない
        // (PEP 440 の `1.0a1.post1`)
        assert_eq!(
            super::parse_version_components("1.0a1.post1"),
            (
                numeric("0"),
                vec![numeric("1"), numeric("0")],
                Some(vec![Alpha("a".to_string()), Numeric(numeric("1"))]),
                Some(numeric("1"))
            )
        );
        // Ruby 風ドット区切り prerelease は識別子列として複数セグメントを取り込む
        assert_eq!(
            super::parse_version_components("7.0.0.alpha.2"),
            (
                numeric("0"),
                vec![numeric("7"), numeric("0"), numeric("0")],
                Some(vec![Alpha("alpha".to_string()), Numeric(numeric("2"))]),
                None
            )
        );
        // 多バイト文字を含む post セグメントはバイト列比較で panic せず、
        // 末尾数字なしのため post は既定値 0 になる
        assert_eq!(
            super::parse_version_components("1.0.0.posté"),
            (
                numeric("0"),
                vec![numeric("1"), numeric("0"), numeric("0")],
                None,
                Some(numeric("0"))
            )
        );
        // 数値プレフィックス付き多バイトセグメントはセパレータなし prerelease として
        // 文字境界安全に分解される
        assert_eq!(
            super::parse_version_components("1.0.0.0abcé"),
            (
                numeric("0"),
                vec![numeric("1"), numeric("0"), numeric("0"), numeric("0")],
                Some(vec![Alpha("abcé".to_string())]),
                None
            )
        );
        // u64 を超える数値コアも任意桁で保持される
        assert_eq!(
            super::parse_version_components("18446744073709551616.0"),
            (
                numeric("0"),
                vec![numeric("18446744073709551616"), numeric("0")],
                None,
                None
            )
        );
    }

    /// characterization テスト: ハイフン区切り prerelease の確定規則を固定する。
    /// `numeric_hyphen_is_prerelease` フラグ (semver: 純数字も prerelease /
    /// Java: 安定版の追加パート) と、コア内 prerelease が優先される規則、
    /// ネストしたハイフン・空 prerelease の扱いを検証する。
    #[test]
    fn test_parse_version_components_hyphen_prerelease_resolution() {
        use super::PreIdentifier::{Alpha, Numeric};
        // 純数字ハイフンはデフォルト (Java 等) では prerelease にならない
        assert_eq!(
            super::parse_version_components_with_options("1.0.0-1", false),
            (
                numeric("0"),
                vec![numeric("1"), numeric("0"), numeric("0")],
                None,
                None
            )
        );
        // semver 系 (numeric_hyphen_is_prerelease=true) では純数字も prerelease
        assert_eq!(
            super::parse_version_components_with_options("1.0.0-1", true),
            (
                numeric("0"),
                vec![numeric("1"), numeric("0"), numeric("0")],
                Some(vec![Numeric(numeric("1"))]),
                None
            )
        );
        // 英字を含むハイフン prerelease はフラグに関係なく取り込まれ、
        // 2 個目以降のハイフンも識別子区切りとして分解される
        assert_eq!(
            super::parse_version_components_with_options("1.0.0-canary-456", false),
            (
                numeric("0"),
                vec![numeric("1"), numeric("0"), numeric("0")],
                Some(vec![Alpha("canary".to_string()), Numeric(numeric("456"))]),
                None
            )
        );
        // 末尾ハイフンのみ (空 prerelease) は None
        assert_eq!(
            super::parse_version_components_with_options("1.2.3-", true),
            (
                numeric("0"),
                vec![numeric("1"), numeric("2"), numeric("3")],
                None,
                None
            )
        );
        // コアセグメント内で prerelease が確定した場合はハイフン側より優先される
        assert_eq!(
            super::parse_version_components_with_options("1.0rc1-beta", true),
            (
                numeric("0"),
                vec![numeric("1"), numeric("0")],
                Some(vec![Alpha("rc".to_string()), Numeric(numeric("1"))]),
                None
            )
        );
    }

    /// 回帰テスト (semver 11.4): プレリリースの英字識別子を ASCII 辞書順で比較する。
    /// 以前は数値識別子しか見ておらず `6.0.0-alpha.24 > 6.0.0-beta.2` となり、
    /// プレリリース利用者が beta → alpha へ実質ダウングレードされていた。
    #[test]
    fn test_compare_versions_prerelease_alpha_identifiers_ordered() {
        use std::cmp::Ordering;
        // alpha.24 < beta.2 (英字識別子が先に比較され、数値はその後)
        assert_eq!(
            compare_versions("6.0.0-alpha.24", "6.0.0-beta.2"),
            Ordering::Less
        );
        assert_eq!(
            compare_versions("6.0.0-beta.2", "6.0.0-alpha.24"),
            Ordering::Greater
        );
        // ベータ2はリリース候補1より古い
        assert_eq!(
            compare_versions("1.0.0-beta.2", "1.0.0-rc.1"),
            Ordering::Less
        );
        // 前方一致で等しい場合は識別子数が少ない方が小さい (semver 11.4.4)
        assert_eq!(
            compare_versions("1.0.0-alpha", "1.0.0-alpha.1"),
            Ordering::Less
        );
        // 大小文字差は吸収される
        assert_eq!(
            compare_versions("1.0.0-Alpha.1", "1.0.0-alpha.1"),
            Ordering::Equal
        );
        // 数値識別子 < 英字識別子（semver 11.4.3）
        assert_eq!(
            compare_versions("1.0.0-alpha.1", "1.0.0-alpha.beta"),
            Ordering::Less
        );
    }

    /// semver 11.4 のスペック例そのままの順序チェーン:
    /// 優先順位: alpha < alpha.1 < alpha.beta < beta < beta.2 < beta.11 < rc.1 < 安定版
    #[test]
    fn test_compare_versions_semver_spec_precedence_chain() {
        let chain = [
            "1.0.0-alpha",
            "1.0.0-alpha.1",
            "1.0.0-alpha.beta",
            "1.0.0-beta",
            "1.0.0-beta.2",
            "1.0.0-beta.11",
            "1.0.0-rc.1",
            "1.0.0",
        ];
        for pair in chain.windows(2) {
            assert_eq!(
                compare_versions(pair[0], pair[1]),
                std::cmp::Ordering::Less,
                "{} < {} であるべき",
                pair[0],
                pair[1]
            );
        }
    }

    /// 回帰テスト: セパレータなし PEP 440 形式は `-` 区切り形式と同値に比較される
    /// (`rc1` を `[Alpha("rc"), Numeric(1)]` へ分解するため)。
    #[test]
    fn test_compare_versions_separatorless_equals_hyphen_form() {
        use std::cmp::Ordering;
        assert_eq!(compare_versions("2.0.0rc1", "2.0.0-rc.1"), Ordering::Equal);
        assert_eq!(compare_versions("2.0.0-rc.1", "2.0.0rc1"), Ordering::Equal);
        // 数値識別子部分の順序付けも形式をまたいで機能する
        assert_eq!(compare_versions("2.0.0rc1", "2.0.0-rc.2"), Ordering::Less);
        assert_eq!(
            compare_versions("2.0.0-rc.2", "2.0.0rc1"),
            Ordering::Greater
        );
    }

    /// 回帰テスト (PEP 440 整合): `dev` は他のどの Alpha 識別子よりも小さい
    /// (PEP 440 では dev < a < b < rc。辞書順だと b < dev < rc になるため特例)。
    #[test]
    fn test_compare_versions_dev_is_weakest_prerelease() {
        use std::cmp::Ordering;
        // 1.0.0.dev1 < 1.0.0a1 (dev リリースは alpha より古い)
        assert_eq!(compare_versions("1.0.0.dev1", "1.0.0a1"), Ordering::Less);
        assert_eq!(compare_versions("1.0.0a1", "1.0.0.dev1"), Ordering::Greater);
        // dev < beta / rc も同様
        assert_eq!(compare_versions("1.0.0.dev1", "1.0.0b1"), Ordering::Less);
        assert_eq!(compare_versions("1.0.0.dev1", "1.0.0rc1"), Ordering::Less);
        // ハイフン形式の dev も最弱
        assert_eq!(
            compare_versions("1.0.0-dev.1", "1.0.0-alpha.1"),
            Ordering::Less
        );
        // dev 同士は数値識別子で比較する
        assert_eq!(compare_versions("1.0.0.dev1", "1.0.0.dev2"), Ordering::Less);
    }

    /// 回帰テスト: 純粋に数値だけの `-` サフィックス (Java SNAPSHOT 等の qualifier)
    /// は prerelease 扱いしない、という既存の意図的設計を維持する。
    #[test]
    fn test_compare_versions_pure_numeric_hyphen_suffix_stays_stable() {
        use std::cmp::Ordering;
        // 1.0.1-1 は stable のまま (prerelease ではない)
        assert!(!is_prerelease_version("1.0.1-1"));
        assert_eq!(compare_versions("1.0.1-1", "1.0.1"), Ordering::Equal);
        assert_eq!(compare_versions("1.0.1", "1.0.1-1"), Ordering::Equal);
        // prerelease 付きより大きい (stable 扱いのため)
        assert_eq!(compare_versions("1.0.1-1", "1.0.1-rc.1"), Ordering::Greater);
    }

    /// 回帰テスト: ビルドメタデータ (`+` 以降) は prerelease 判定の走査対象外。
    /// 以前は `1.0.0+sha.a1b2c3` (メタデータ内の digit+英字+digit) や
    /// `1.0.0+pre.1` を prerelease と誤判定していた。
    #[test]
    fn test_is_prerelease_ignores_build_metadata() {
        assert!(!is_prerelease_version("1.0.0+sha.a1b2c3"));
        assert!(!is_prerelease_version("1.0.0+pre.1"));
        assert!(!is_prerelease_version("1.0.0+build.123"));
        assert!(!is_prerelease_version("1.0.0+20251110a1"));
        // バージョン本体側の prerelease は引き続き検出する
        assert!(is_prerelease_version("1.0.0-rc.1+build"));
        assert!(is_prerelease_version("1.0.0-beta+sha.a1b2c3"));
        assert!(is_prerelease_version("2.0.0rc1+build"));
    }

    /// 回帰テスト (PEP 440): `.devN` セグメントは比較で無視されず、
    /// 対応する release より小さい (以前は `1.0.1.dev1 == 1.0.1` だった)。
    #[test]
    fn test_compare_versions_pep440_dev_segment_is_less_than_release() {
        use std::cmp::Ordering;
        assert_eq!(compare_versions("1.0.1.dev1", "1.0.1"), Ordering::Less);
        assert_eq!(compare_versions("1.0.1", "1.0.1.dev1"), Ordering::Greater);
        // 数値コアが異なる場合はコアが優先される
        assert_eq!(compare_versions("1.0.1.dev1", "1.0.0"), Ordering::Greater);
        // dev は post より小さい (dev < release < post)
        assert_eq!(
            compare_versions("1.0.1.dev1", "1.0.1.post1"),
            Ordering::Less
        );
    }

    /// 回帰テスト: Ruby のドット区切り prerelease (`7.0.0.alpha.2`) は
    /// 対応する release より小さく、識別子列で順序付けされる。
    #[test]
    fn test_compare_versions_ruby_dot_separated_prerelease() {
        use std::cmp::Ordering;
        assert_eq!(compare_versions("7.0.0.alpha.2", "7.0.0"), Ordering::Less);
        assert_eq!(
            compare_versions("7.0.0.alpha.2", "7.0.0.alpha.3"),
            Ordering::Less
        );
        assert_eq!(
            compare_versions("7.0.0.alpha.2", "7.0.0.beta.1"),
            Ordering::Less
        );
        assert_eq!(compare_versions("1.0.0.pre.1", "1.0.0"), Ordering::Less);
        // Java qualifier (RELEASE / Final) は prerelease 扱いされず release と等価のまま
        assert_eq!(compare_versions("5.0.0.RELEASE", "5.0.0"), Ordering::Equal);
        assert_eq!(compare_versions("4.0.0.Final", "4.0.0"), Ordering::Equal);
    }

    #[test]
    fn test_pre_identifier_ordering_rules() {
        use super::PreIdentifier::{Alpha, Numeric};
        // Numeric 同士は数値比較
        assert!(Numeric(numeric("1")) < Numeric(numeric("2")));
        assert!(Numeric(numeric("2")) < Numeric(numeric("11")));
        // 数値識別子 < 英字識別子（semver 11.4.3）
        assert!(Numeric(numeric("999")) < Alpha("alpha".to_string()));
        // Alpha 同士は ASCII 辞書順
        assert!(Alpha("alpha".to_string()) < Alpha("beta".to_string()));
        assert!(Alpha("beta".to_string()) < Alpha("rc".to_string()));
        // dev は他のどの Alpha よりも小さい (PEP 440 整合の特例)
        assert!(Alpha("dev".to_string()) < Alpha("a".to_string()));
        assert!(Alpha("dev".to_string()) < Alpha("alpha".to_string()));
        assert!(Alpha("dev".to_string()) < Alpha("rc".to_string()));
        assert_eq!(Alpha("dev".to_string()), Alpha("dev".to_string()));
    }

    #[test]
    fn test_decompose_alnum_runs() {
        use super::PreIdentifier::{Alpha, Numeric};
        assert_eq!(
            super::decompose_alnum_runs("rc1"),
            vec![Alpha("rc".to_string()), Numeric(numeric("1"))]
        );
        assert_eq!(
            super::decompose_alnum_runs("dev1rc1"),
            vec![
                Alpha("dev".to_string()),
                Numeric(numeric("1")),
                Alpha("rc".to_string()),
                Numeric(numeric("1"))
            ]
        );
        assert_eq!(
            super::decompose_alnum_runs("alpha"),
            vec![Alpha("alpha".to_string())]
        );
        assert_eq!(
            super::decompose_alnum_runs("2"),
            vec![Numeric(numeric("2"))]
        );
        assert_eq!(super::decompose_alnum_runs(""), Vec::<PreIdentifier>::new());
    }

    #[test]
    fn test_numeric_core_extraction() {
        // ChangeLevel と共有する数値コア抽出ヘルパー
        let expected = || vec![numeric("1"), numeric("2"), numeric("3")];
        assert_eq!(super::numeric_core("1.2.3"), expected());
        assert_eq!(super::numeric_core("v1.2.3"), expected());
        assert_eq!(super::numeric_core("1.2.3-rc.1"), expected());
        assert_eq!(super::numeric_core("1.2.3+sha.abc"), expected());
        // セグメントは先頭の数値プレフィックスのみを取る
        assert_eq!(
            super::numeric_core("1.0.0rc1"),
            vec![numeric("1"), numeric("0"), numeric("0")]
        );
        // 数値が全く無いセグメント以降は無視する
        assert_eq!(
            super::numeric_core("5.0.0.RELEASE"),
            vec![numeric("5"), numeric("0"), numeric("0")]
        );
        assert_eq!(
            super::numeric_core("1.2.RELEASE.3"),
            vec![numeric("1"), numeric("2")]
        );
        // エポックは数値コアに含めない
        assert_eq!(
            super::numeric_core("1!2.3"),
            vec![numeric("2"), numeric("3")]
        );
        assert_eq!(super::numeric_core("abc"), Vec::<NumericIdentifier>::new());
    }

    #[test]
    fn test_semver_numeric_prerelease_ordering() {
        use std::cmp::Ordering;

        assert_eq!(compare_semver_versions("1.0.0-1", "1.0.0"), Ordering::Less);
        assert_eq!(
            compare_semver_versions("1.0.0-18446744073709551616", "1.0.0-2"),
            Ordering::Greater
        );
        assert!(is_semver_prerelease_version("1.0.0-1"));
    }

    #[test]
    fn test_rubygems_version_ordering() {
        use std::cmp::Ordering;

        assert_eq!(compare_ruby_versions("1.0.zeta", "1.0"), Ordering::Less);
        assert_eq!(compare_ruby_versions("1.0.0-1", "1.0.0"), Ordering::Less);
        assert_eq!(compare_ruby_versions("1.0.0", "1.0.0.0"), Ordering::Equal);
        assert_eq!(compare_ruby_versions("1.0.A", "1.0.a"), Ordering::Less);
        assert!(is_ruby_prerelease_version("1.0.zeta"));
        assert!(is_ruby_prerelease_version("1.0.0-1"));
        assert!(!is_ruby_prerelease_version("1.0.0"));
    }

    #[test]
    fn test_composer_patch_alias_ordering() {
        use std::cmp::Ordering;

        for alias in ["1.0.0-p1", "1.0.0-pl1", "1.0.0-patch1"] {
            assert_eq!(compare_composer_versions(alias, "1.0.0"), Ordering::Greater);
        }
    }

    #[test]
    fn test_gradle_version_ordering() {
        use std::cmp::Ordering;

        assert_eq!(
            compare_gradle_versions("1.0-zeta", "1.0-rc"),
            Ordering::Less
        );
        assert_eq!(compare_gradle_versions("1.1", "1.1.0"), Ordering::Less);
        assert_eq!(compare_gradle_versions("1.1.a", "1.1"), Ordering::Less);
        assert_eq!(compare_gradle_versions("1a1", "1-a+1"), Ordering::Equal);
        assert_eq!(
            compare_gradle_versions("1.0-RC-1", "1.0.rc.1"),
            Ordering::Equal
        );
    }
}
