//! 各パッケージエコシステムで使うバージョン指定型。
//!
//! 例:
//! - Node.js の例: `^1.2.3`, `~1.2.3`, `>=1.0.0`, `1.2.3`
//! - Python の例: `^1.2.3`, `~1.2.3`, `>=1.2.3`, `==1.2.3`
//! - Rust の例: `1.2.3`, `^1.2.3`, `~1.2.3`, `=1.2.3`
//! - Go の例: `v1.2.3`, `// pinned`

use serde::{Deserialize, Serialize};
use std::fmt;

/// バージョン指定の種類
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VersionSpecKind {
    /// 固定バージョン。例: Node の `1.2.3`、Python の `==1.2.3`、Rust の `=1.2.3`
    Exact,
    /// Caret レンジ。例: `^1.2.3`
    Caret,
    /// Tilde レンジ。例: `~1.2.3`
    Tilde,
    /// 以上。例: `>=1.2.3`
    GreaterOrEqual,
    /// より大きい。例: `>1.2.3`
    Greater,
    /// 以下。例: `<=1.2.3`
    LessOrEqual,
    /// より小さい。例: `<1.2.3`
    Less,
    /// ワイルドカード。例: `1.2.*`, `1.2.x`, `1.2.+`
    Wildcard,
    /// 複合レンジ。例: `>=1.0.0 <2.0.0`
    Range,
    /// `// pinned` コメント付き Go バージョン
    GoPinned,
    /// プレフィックス選択。例: mise の `node = "26"` / `"26.7"` / `"prefix:26"`
    ///
    /// ワイルドカード文字を持たずに部分バージョンだけを書き、
    /// 「その前方一致に当てはまる最新版」を指す形式。更新時は元のセグメント数を保つ
    /// (`26` → `27`、`26.7` → `26.8`)。セグメント数が許容幅そのものなので、
    /// レジストリの完全版をそのまま書き戻すと制約が黙って狭まる。
    Prefix,
    /// 制約なし。例: `gem 'rails'`
    Any,
}

impl VersionSpecKind {
    /// 固定バージョンとして扱う種類かどうかを返す
    pub fn is_pinned(&self) -> bool {
        matches!(self, VersionSpecKind::Exact | VersionSpecKind::GoPinned)
    }
}

/// 元の文字列表現も保持したバージョン指定
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionSpec {
    /// バージョン指定の種類
    pub kind: VersionSpecKind,
    /// マニフェスト上の元の文字列
    pub raw: String,
    /// 抽出したバージョン番号。prefix/suffix は含めない
    pub version: String,
    /// 更新時に保持する接頭辞。例: `^`, `~`, `>=`
    pub prefix: Option<String>,
    /// 更新時に保持する接尾辞。例: コメント
    pub suffix: Option<String>,
    /// 更新候補から除外するバージョン。例: Gradle rich version の `reject`
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rejected_versions: Vec<String>,
}

fn extract_numeric_parts(new_version: &str) -> Option<Vec<String>> {
    let numeric_head = new_version
        .strip_prefix('v')
        .or_else(|| new_version.strip_prefix('V'))
        .unwrap_or(new_version)
        .split(|ch: char| !ch.is_ascii_digit() && ch != '.')
        .next()
        .unwrap_or("");

    let parts: Vec<String> = numeric_head
        .split('.')
        .filter(|part| !part.is_empty())
        .map(ToString::to_string)
        .collect();

    if parts.is_empty() { None } else { Some(parts) }
}

/// バージョントークンがワイルドカードセグメント (`x` / `X` / `*` / `+`) を含むかを判定する。
///
/// `format_wildcard_like` が受理する形と同じ基準 (演算子と `v`/`V` 接頭辞を除いた
/// ドット区切りセグメントが厳密に `x` / `X` / `*` / `+` のいずれか) で判定するため、
/// 「ワイルドカードとして再構成できるか」の判断が呼び出し側とずれない。
///
/// 単純な `contains(['x', 'X', '*'])` にすると `1.0.0-linux` のように英字 `x` を含む
/// 通常のバージョンまでワイルドカード扱いになり、`format_wildcard_like` が `None` を
/// 返して更新ごと落ちる。
fn has_wildcard_segment(token: &str) -> bool {
    let trimmed = token.trim();
    let op_len = trimmed
        .bytes()
        .take_while(|b| matches!(b, b'^' | b'~' | b'='))
        .count();
    let body = trimmed[op_len..].trim_start();
    let body = body
        .strip_prefix('v')
        .or_else(|| body.strip_prefix('V'))
        .unwrap_or(body);

    body.split('.')
        .any(|segment| matches!(segment, "*" | "x" | "X" | "+"))
}

fn format_wildcard_like(raw: &str, new_version: &str) -> Option<String> {
    let trimmed = raw.trim();
    // npm の `^1.x` / `~1.2.*` や Cargo の `=1.*` / `^1.*` のような演算子付きワイルドカードでは、
    // 先頭の `^` / `~` / `=` 演算子を切り出して保持し、残りをワイルドカードとして再構成する。
    // 既存のワイルドカード (`1.x` / `1.2.*` / `v1.*` / `1.+`) は演算子を持たないため
    // op_prefix は空となり、従来どおりの挙動になる。
    let op_len = trimmed
        .bytes()
        .take_while(|b| matches!(b, b'^' | b'~' | b'='))
        .count();
    let op_prefix = &trimmed[..op_len];
    let body = trimmed[op_len..].trim_start();

    if matches!(body, "*" | "x" | "X") {
        return Some(format!("{op_prefix}{body}"));
    }

    let Some(mut parts) = extract_numeric_parts(new_version) else {
        return Some(trimmed.to_string());
    };

    let segments: Vec<&str> = body.split('.').collect();
    while parts.len() < segments.len() {
        parts.push("0".to_string());
    }

    let mut rebuilt = Vec::with_capacity(segments.len());
    let mut has_numeric_anchor = false;

    for (index, segment) in segments.iter().enumerate() {
        let (prefix, core) = if index == 0 {
            if let Some(rest) = segment.strip_prefix('v') {
                ("v", rest)
            } else if let Some(rest) = segment.strip_prefix('V') {
                ("V", rest)
            } else {
                ("", *segment)
            }
        } else {
            ("", *segment)
        };

        let rebuilt_segment = if !core.is_empty() && core.chars().all(|ch| ch.is_ascii_digit()) {
            has_numeric_anchor = true;
            format!("{}{}", prefix, parts[index])
        } else if matches!(core, "*" | "x" | "X" | "+") {
            format!("{}{}", prefix, core)
        } else {
            return None;
        };

        rebuilt.push(rebuilt_segment);
    }

    if !has_numeric_anchor {
        return Some(trimmed.to_string());
    }

    Some(format!("{op_prefix}{}", rebuilt.join(".")))
}

fn format_partial_version_like(raw: &str, new_version: &str) -> Option<String> {
    let trimmed = raw.trim();
    let (op_prefix, body) = if let Some(rest) = trimmed.strip_prefix('=') {
        ("=", rest.trim_start())
    } else {
        ("", trimmed)
    };
    let (version_prefix, core) = if let Some(rest) = body.strip_prefix('v') {
        ("v", rest)
    } else if let Some(rest) = body.strip_prefix('V') {
        ("V", rest)
    } else {
        ("", body)
    };

    if core.is_empty()
        || !core
            .split('.')
            .all(|segment| !segment.is_empty() && segment.chars().all(|ch| ch.is_ascii_digit()))
    {
        return None;
    }

    let segment_count = core.split('.').count();
    let mut parts = extract_numeric_parts(new_version)?;
    while parts.len() < segment_count {
        parts.push("0".to_string());
    }

    Some(format!(
        "{}{}{}",
        op_prefix,
        version_prefix,
        parts[..segment_count].join(".")
    ))
}

/// プレフィックス選択 (mise の `node = "26"` / `"26.7"`) を、元のセグメント数を
/// 保ったまま新しいバージョンへ書き換える。
///
/// `current_version` は演算子・`prefix:` セレクタ・ベンダー接頭辞を取り除いた
/// 数値部 (`26` / `26.7`) を渡す。接頭辞・接尾辞の再付与は呼び出し側
/// (`wrap_with_affixes`) が行う。
///
/// 更新先から数値セグメントを取り出せない場合は `None` を返し、
/// 呼び出し側で元の表記を保つ (捏造した値を書き戻さない)。
fn format_prefix_like(current_version: &str, new_version: &str) -> Option<String> {
    let segment_count = current_version
        .split('.')
        .take_while(|segment| !segment.is_empty() && segment.bytes().all(|b| b.is_ascii_digit()))
        .count();
    if segment_count == 0 {
        return None;
    }

    let mut parts = extract_numeric_parts(new_version)?;
    // 更新先が元より短い場合 (`26.7` → `27`) は 0 埋めして幅を保つ
    while parts.len() < segment_count {
        parts.push("0".to_string());
    }

    Some(parts[..segment_count].join("."))
}

/// 文字列の先頭にある数値セグメント列 (`1.2.3`) のセグメント数を数える。
///
/// 演算子・空白・`v` 接頭辞は読み飛ばし、数値と `.` 以外が現れた時点で打ち切る。
/// `~> 7.0` → 2、`~1.2.3@beta` → 3、`~> 1.0.0.pre` → 3、`^1.x` → 1。
/// 数値が 1 つも無ければ `None`。
fn leading_numeric_segment_count(text: &str) -> Option<usize> {
    let start = text.find(|c: char| c.is_ascii_digit())?;
    let token = text[start..]
        .split(|c: char| !(c.is_ascii_digit() || c == '.'))
        .next()
        .unwrap_or_default();
    let count = token
        .split('.')
        .take_while(|segment| !segment.is_empty() && segment.bytes().all(|b| b.is_ascii_digit()))
        .count();
    (count > 0).then_some(count)
}

/// Tilde 制約を、元の指定と同じセグメント数へ揃えた更新後バージョンにする。
///
/// Tilde の許容幅は元のセグメント数で決まる:
/// - npm / Cargo / Poetry: `~1.2` = `>=1.2.0 <1.3.0`、`~1` = `>=1.0.0 <2.0.0`
/// - Composer: `~1.2` = `>=1.2 <2.0.0`、`~1.2.3` = `>=1.2.3 <1.3.0`
/// - RubyGems: `~> 7.0` = `>= 7.0, < 8.0`、`~> 7.1.3` = `>= 7.1.3, < 7.2`
///
/// レジストリの完全版 (3 セグメント) をそのまま書き戻すと、`~4.4` → `~6.4.7` や
/// `~> 7.0` → `~> 7.1.3.2` のように許容幅が黙って狭まり、以後の
/// `composer update` / `bundle update` がマイナー系列を跨げなくなる。
/// PEP 440 の `~=` では既にセグメント数を保持しているため (`format_range_like`)、
/// Tilde もそれに揃える。
///
/// セグメント数の根拠には比較用の `version` ではなく生表記 (`raw`) を使う。
/// Node のパーサは比較用バージョンを 3 セグメントへ 0 埋め正規化する
/// (`~10.3` → `version = "10.3.0"`) ため、`version` から数えると元の粒度が失われ、
/// Node だけこの保持が無効化されていた (`~1` → `~2.5.3` で major 幅 `<2.0.0` が
/// minor 幅 `<2.6.0` へ縮む)。書き戻しの根拠は常に生表記側から取る。
///
/// 次の場合は情報を落とさないよう `None` を返し、呼び出し側で完全版を使う:
/// - 元の指定からセグメント数を数えられない
/// - 更新先がプレリリース / ビルドメタデータを含む (`2.0.0-rc.1` 等。
///   切り詰めると識別子ごと消えて別の制約になる)
fn format_tilde_like(raw: &str, current_version: &str, new_version: &str) -> Option<String> {
    fn numeric_segments(value: &str) -> Option<Vec<&str>> {
        if value.is_empty() {
            return None;
        }
        let segments: Vec<&str> = value.split('.').collect();
        segments
            .iter()
            .all(|segment| !segment.is_empty() && segment.bytes().all(|byte| byte.is_ascii_digit()))
            .then_some(segments)
    }

    let current = current_version.trim();
    let version_prefix = if current.starts_with('v') {
        "v"
    } else if current.starts_with('V') {
        "V"
    } else {
        ""
    };
    // 演算子・空白・`v` 接頭辞を読み飛ばし、先頭の数値セグメント列だけを数える
    // (`~> 7.0` → 2、`~1.2.3@beta` → 3、`~> 1.0.0.pre` → 3)。
    let segment_count = leading_numeric_segment_count(raw)
        .or_else(|| leading_numeric_segment_count(current_version))?;

    let new_core = new_version
        .trim()
        .strip_prefix('v')
        .or_else(|| new_version.trim().strip_prefix('V'))
        .unwrap_or_else(|| new_version.trim());
    let mut parts: Vec<String> = numeric_segments(new_core)?
        .into_iter()
        .map(ToString::to_string)
        .collect();
    while parts.len() < segment_count {
        parts.push("0".to_string());
    }

    Some(format!(
        "{}{}",
        version_prefix,
        parts[..segment_count].join(".")
    ))
}

fn preserve_version_prefix(template: &str, new_version: &str) -> String {
    let stripped = new_version
        .strip_prefix('v')
        .or_else(|| new_version.strip_prefix('V'))
        .unwrap_or(new_version);

    if template.starts_with('V') {
        format!("V{}", stripped)
    } else if template.starts_with('v') {
        format!("v{}", stripped)
    } else {
        stripped.to_string()
    }
}

fn find_first_version_token(raw: &str) -> Option<(usize, usize)> {
    let chars: Vec<(usize, char)> = raw.char_indices().collect();

    for (index, &(start, ch)) in chars.iter().enumerate() {
        let looks_like_start = ch.is_ascii_digit()
            || ((ch == 'v' || ch == 'V')
                && chars
                    .get(index + 1)
                    .map(|(_, next)| next.is_ascii_digit())
                    .unwrap_or(false));
        if !looks_like_start {
            continue;
        }

        let mut end = raw.len();
        for (scan_idx, &(candidate_end, candidate)) in chars.iter().enumerate().skip(index + 1) {
            // 連続するドット (..) はレンジ演算子 (..< / ...) なのでトークンを終端する
            if candidate == '.'
                && chars
                    .get(scan_idx + 1)
                    .map(|(_, next)| *next == '.')
                    .unwrap_or(false)
            {
                end = candidate_end;
                break;
            }
            // `!` は PEP 440 の epoch 区切り (`1!1.0`)。これを許可しないと
            // `>=1!1.0,<3` の下限トークンが `1` で切れ、比較基準が壊れるうえ
            // `>=2.5!1.0,<3` のような不正な制約を書き戻す。
            // `!=` を含む raw は `contains_not_equal_operator` が先に弾くため、
            // ここで `!` を許容しても除外制約を書き換える経路は増えない。
            if !(candidate.is_ascii_alphanumeric()
                || matches!(candidate, '.' | '*' | '+' | '-' | '_' | '!'))
            {
                end = candidate_end;
                break;
            }
        }

        return Some((start, end));
    }

    None
}

fn replace_version_token(raw: &str, start: usize, end: usize, new_version: &str) -> Option<String> {
    let token = &raw[start..end];
    // ワイルドカード判定は `has_wildcard_segment` に一本化する。以前ここだけが
    // `contains('*')` で `x` / `X` を見落としており、`~1.x <2.0.0` の下限が
    // `~1.9.3` へ展開されて Tilde の許容幅が黙って縮んでいた
    // (同義の `~1.* <2.0.0` は形が保たれるという非対称があった)。
    let replacement = if has_wildcard_segment(token) {
        format_wildcard_like(token, new_version)?
    } else {
        preserve_version_prefix(token, new_version)
    };

    Some(format!("{}{}{}", &raw[..start], replacement, &raw[end..]))
}

fn replace_version_token_preserving_shape(
    raw: &str,
    start: usize,
    end: usize,
    new_version: &str,
) -> Option<String> {
    let token = &raw[start..end];
    let replacement = if has_wildcard_segment(token) {
        format_wildcard_like(token, new_version)?
    } else {
        // セグメント数を保てない形 (プレリリース識別子やビルドメタデータを含む
        // `~1.2.3-rc.1` 等) では識別子を落とさないよう完全版へフォールバックする。
        // 単体 Tilde (`format_tilde_like` 経由) は同じ状況で完全版を使うため、
        // ここで諦めると「上限が付いて Range になった途端に更新できなくなる」
        // という非対称が生まれる。
        format_partial_version_like(token, new_version)
            .unwrap_or_else(|| preserve_version_prefix(token, new_version))
    };

    Some(format!("{}{}{}", &raw[..start], replacement, &raw[end..]))
}

fn find_version_token_at(raw: &str, offset: usize) -> Option<(usize, usize)> {
    let rest = raw.get(offset..)?;
    let whitespace_len = rest.len() - rest.trim_start().len();
    let token_start = offset + whitespace_len;
    let token_rest = raw.get(token_start..)?;
    let (start, end) = find_first_version_token(token_rest)?;
    if start == 0 {
        Some((token_start, token_start + end))
    } else {
        None
    }
}

fn find_gradle_strict_prefer_token(raw: &str) -> Option<(usize, usize)> {
    let bang_index = raw.find("!!")?;
    let strict_part = raw[..bang_index].trim();
    if !matches!(strict_part.chars().next(), Some('[' | '(' | ']')) {
        return None;
    }

    find_version_token_at(raw, bang_index + 2)
}

/// 包含下限のトークンを、直前の演算子とともに返す。
///
/// 演算子を返すのは、PEP 440 の compatible release (`~=`) だけセグメント数を保って
/// 書き換える必要があるため (セグメント数が暗黙上限の幅を決める)。
fn find_inclusive_lower_bound_token(raw: &str) -> Option<(&'static str, usize, usize)> {
    let operators = [">=", "~=", "==", "=", "^", "~"];
    let mut index = 0;

    while index < raw.len() {
        let rest = &raw[index..];
        let is_operator_continuation =
            index > 0 && matches!(raw.as_bytes()[index - 1], b'<' | b'>' | b'!' | b'=' | b'~');
        if is_operator_continuation {
            let ch = rest.chars().next()?;
            index += ch.len_utf8();
            continue;
        }

        for operator in operators {
            if rest.starts_with(operator) {
                let after_operator = index + operator.len();
                if let Some((start, end)) = find_version_token_at(raw, after_operator) {
                    return Some((operator, start, end));
                }
            }
        }

        let ch = rest.chars().next()?;
        index += ch.len_utf8();
    }

    None
}

fn find_bare_lower_bound_token(raw: &str) -> Option<(usize, usize)> {
    let leading_ws_len = raw.len() - raw.trim_start().len();
    find_version_token_at(raw, leading_ws_len)
}

/// レンジ文字列から比較基準にする包含下限のバージョン文字列を返す。
///
/// `>=` / `~=` / `==` / `=` / `^` / `~` の直後、または裸の下限トークンを、
/// カンマ/空白区切りの記述順に依存せず探して返す。書き換え側 (`format_range_like`)
/// と同じトークン探索を使うため、judge が使う比較基準 version と、実際に writer が
/// 書き換えるトークンが必ず一致する。これにより上限を先に書いたレンジ
/// (`<1.5, >=1.2.2` など) でも下限 `1.2.2` を基準にでき、AlreadyLatest 誤判定による
/// 更新取りこぼしを防ぐ。包含下限が無い場合 (厳密下限 `>1.0` のみ等) は `None` を返し、
/// 呼び出し側の従来ロジックにフォールバックさせる。
pub fn range_lower_bound_version(raw: &str) -> Option<String> {
    let (start, end) = find_inclusive_lower_bound_token(raw)
        .map(|(_, start, end)| (start, end))
        .or_else(|| find_bare_lower_bound_token(raw))?;
    Some(raw[start..end].to_string())
}

fn contains_not_equal_operator(raw: &str) -> bool {
    // `!==` は各エコシステムの有効な制約ではないが、`!=` を含むので同じく拒否する。
    // Composer (composer/semver) は not-equal を `!=` と `<>` の両方で綴れる
    // (演算子パターン `(<>|!=|>=?|<=?|==?)`)。`<>` を含む制約も除外制約なので、
    // `!=` と同様に安全側でスキップする (下限だけ書き換えると除外バージョンを
    // 選んで充足不能な制約 `>=1.5.0 <>1.5.0 <2.0` を生む恐れがあるため)。
    raw.as_bytes()
        .windows(2)
        .any(|window| window == b"!=" || window == b"<>")
}

fn format_range_like(raw: &str, new_version: &str) -> Option<String> {
    let trimmed = raw.trim();
    let leading_ws_len = raw.len() - raw.trim_start().len();

    if let Some((start, end)) = find_gradle_strict_prefer_token(raw) {
        return replace_version_token(raw, start, end, new_version);
    }

    // OR 結合された制約は「どちらかの分岐を満たせばよい」という和集合なので、
    // 片方の分岐の下限だけを進めると意味が変わる (`>=1.0 <2.0 | >=3.0` の下限を
    // 上げると 1.x を許す分岐が消える)。安全に書き換えられないため丸ごと諦める。
    //
    // 判定は `|` 1 文字で行う。composer/semver の `parseConstraints` は
    // `preg_split('{\s*\|\|?\s*}')` で `|` と `||` を同格の OR として扱うため、
    // `||` だけを見ていると `>=1.0 <2.0 | >=3.0` のような後方互換表記が素通りする。
    // `|` を制約構文の別の用途で使うエコシステムは無い。
    if trimmed.contains('|') || contains_not_equal_operator(trimmed) || trimmed.starts_with("===") {
        return None;
    }

    // 単一制約の `~=1.2.3` のみ、演算子を保持しつつセグメント数を維持して下限を進める。
    // セグメント数を変えると上限の意味が変わる (`~=1.2` の上限 <2.0 が `~=1.9.0` だと
    // <1.10.0 になる) ため、format_partial_version_like でセグメント数を保つ。
    // `~=1.2, <1.5` のような複合制約は横取りせず、下の find_inclusive_lower_bound_token
    // 経路に任せる (横取りすると body にカンマ以降が混ざり format に失敗する)。
    if !trimmed.contains(',')
        && let Some(rest) = trimmed.strip_prefix("~=")
    {
        let spacing_len = rest.len() - rest.trim_start().len();
        let spacing = &rest[..spacing_len];
        let body = rest.trim();
        return format_partial_version_like(body, new_version)
            .map(|formatted| format!("~={spacing}{formatted}"));
    }

    if let Some(rest) = trimmed.strip_prefix("==") {
        let spacing_len = rest.len() - rest.trim_start().len();
        let spacing = &rest[..spacing_len];
        let body = rest.trim();
        if body.contains('*') {
            return format_wildcard_like(body, new_version)
                .map(|formatted| format!("=={}{}", spacing, formatted));
        }
    }

    let has_explicit_range_syntax = trimmed.contains('<')
        || trimmed.contains('>')
        || trimmed.contains(',')
        || trimmed.contains(" - ")
        || trimmed.contains("..<")
        || trimmed.contains("...");

    if !has_explicit_range_syntax
        && !trimmed.starts_with('[')
        && !trimmed.starts_with('(')
        && !trimmed.starts_with(']')
    {
        return format_partial_version_like(trimmed, new_version);
    }

    if matches!(trimmed.chars().next(), Some('[' | '(' | ']')) {
        let comma_index = trimmed.find(',')?;
        let lower = trimmed[1..comma_index].trim();
        if lower.is_empty() || !trimmed.starts_with('[') {
            return None;
        }

        let lower_offset = leading_ws_len + 1;
        let lower_start = find_version_token_at(raw, lower_offset)?;
        if lower_start.0 >= leading_ws_len + comma_index {
            return None;
        }

        return replace_version_token(raw, lower_start.0, lower_start.1, new_version);
    }

    if trimmed.contains(" - ") || trimmed.contains("..<") || trimmed.contains("...") {
        let (start, end) = find_first_version_token(raw)?;
        return replace_version_token(raw, start, end, new_version);
    }

    // カンマ区切りの複数要件で上限 (`<` / `<=`) がない場合 (例: `>=1.2.3, ^1.3`)、
    // 包含下限だけを進めると充足不能なレンジになり得るため安全に書き換えられない。
    // (単一の包含下限 `>=1.0` は上限がなくても最新へ進められるので対象外)
    if trimmed.contains(',') && !trimmed.contains('<') {
        return None;
    }

    if let Some((operator, start, end)) = find_inclusive_lower_bound_token(raw) {
        // tilde 系の演算子はセグメント数が暗黙上限の幅を決めるため、複合制約
        // (`~=1.2, <5.0` / `~1 <2.0.0`) でもセグメント数を保って書き換える。
        // 完全版をそのまま埋めると PEP 440 の `~=1.2` (上限 <2.0) が `~=4.9.0`
        // (上限 <4.10.0) へ、npm/Composer/Cargo の `~1` (上限 <2.0.0) が `~1.9.3`
        // (上限 <1.10.0) へ黙って狭まり、以後マイナー系列を跨げなくなる。
        // 単体の Tilde は format_tilde_like がセグメント数を保つのに、comparator set へ
        // 入った途端に保護が外れる非対称を防ぐ。
        // `^` は上限がセグメント数に依存しない (`^1` も `^1.9.3` も上限は <2.0.0) ため対象外。
        if operator == "~=" || operator == "~" {
            return replace_version_token_preserving_shape(raw, start, end, new_version);
        }
        return replace_version_token(raw, start, end, new_version);
    }

    let (start, end) = find_bare_lower_bound_token(raw)?;

    replace_version_token_preserving_shape(raw, start, end, new_version)
}

impl VersionSpec {
    /// 新しい VersionSpec を作る
    pub fn new(kind: VersionSpecKind, raw: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            kind,
            raw: raw.into(),
            version: version.into(),
            prefix: None,
            suffix: None,
            rejected_versions: Vec::new(),
        }
    }

    /// 接頭辞付きの VersionSpec を作る
    pub fn with_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.prefix = Some(prefix.into());
        self
    }

    /// 接尾辞付きの VersionSpec を作る
    pub fn with_suffix(mut self, suffix: impl Into<String>) -> Self {
        self.suffix = Some(suffix.into());
        self
    }

    /// 拒否バージョン一覧付きの VersionSpec を作る
    pub fn with_rejected_versions<I, S>(mut self, versions: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.rejected_versions = versions.into_iter().map(Into::into).collect();
        self
    }

    /// 既定では更新しない固定バージョンかどうかを返す
    pub fn is_pinned(&self) -> bool {
        self.kind.is_pinned()
    }

    /// 報告 (テキスト / JSON 出力) で「現在のバージョン」として見せる文字列。
    ///
    /// `version` は比較用に正規化した値なので、Go のようにバージョン文字列自体が
    /// `v` を含むエコシステムでは接頭辞が落ちる。一方で更新先はレジストリが返す
    /// 生の値 (`v1.6.0`) なので、そのまま並べると `1.3.0 → v1.6.0` という不揃いな
    /// 表示になり、JSON を機械処理する側でも `from` と `to` の書式が食い違う。
    ///
    /// 対象は `v` / `V` の接頭辞だけ。`^` / `~` / `>=` は制約を表す演算子であって
    /// バージョンの一部ではないため含めない (含めると他言語の表示が変わる)。
    pub fn display_version(&self) -> String {
        match self.prefix.as_deref() {
            Some(prefix @ ("v" | "V")) if !self.version.is_empty() => {
                format!("{prefix}{}", self.version)
            }
            _ => self.version.clone(),
        }
    }

    /// 安全に更新後の文字列表現を組み立てられる場合だけ返す
    pub fn try_format_updated(&self, new_version: &str) -> Option<String> {
        match self.kind {
            VersionSpecKind::Wildcard => {
                let raw = self
                    .suffix
                    .as_deref()
                    .and_then(|suffix| self.raw.strip_suffix(suffix))
                    .unwrap_or(&self.raw);
                format_wildcard_like(raw, new_version).map(|body| self.wrap_with_affixes(&body))
            }
            VersionSpecKind::Range => format_range_like(&self.raw, new_version),
            VersionSpecKind::Greater | VersionSpecKind::LessOrEqual | VersionSpecKind::Less => None,
            // Prefix は元のセグメント数が「どこまで固定するか」を表すため、更新後も
            // セグメント数を保つ (`26` → `27`、`26.7` → `26.8`)。完全版を書き戻すと
            // mise の `node = "26"` (26 系の最新を都度解決) が `node = "27.1.0"` の
            // 完全ピンへ黙って変わってしまう。
            VersionSpecKind::Prefix => {
                let body = format_prefix_like(&self.version, new_version)?;
                Some(self.wrap_with_affixes(&body))
            }
            // Tilde は元のセグメント数が許容幅を決めるため、部分指定 (`~1.2` / `~> 7.0`)
            // は更新後もセグメント数を保つ。切り詰められない入力では完全版を使う。
            VersionSpecKind::Tilde => {
                let body = format_tilde_like(&self.raw, &self.version, new_version)
                    .unwrap_or_else(|| new_version.to_string());
                Some(self.wrap_with_affixes(&body))
            }
            _ => Some(self.wrap_with_affixes(new_version)),
        }
    }

    /// バージョン本体を元の接頭辞・接尾辞で挟んで書き戻し用の文字列にする。
    ///
    /// 本体が既に接頭辞・接尾辞を含んでいる場合は二重付与しない。Go のレジストリは
    /// `v1.9.1` / `v2.1.0+incompatible` のように接頭辞・接尾辞込みのバージョンを返すが、
    /// Go の `VersionSpec` は `v` を prefix、`+incompatible` を suffix に持つため、
    /// 素朴に連結すると `--diff` が `vv1.9.1` という go.mod として無効な文字列を
    /// 表示してしまう (実書き込み側の `go_mod::update_version` は正規化済み)。
    fn wrap_with_affixes(&self, body: &str) -> String {
        let mut body = body;
        if let Some(prefix) = self.prefix.as_deref().filter(|p| !p.is_empty())
            && let Some(rest) = body.strip_prefix(prefix)
        {
            body = rest;
        }
        if let Some(suffix) = self.suffix.as_deref().filter(|s| !s.is_empty())
            && let Some(rest) = body.strip_suffix(suffix)
        {
            body = rest;
        }

        let mut result = String::new();
        if let Some(ref prefix) = self.prefix {
            result.push_str(prefix);
        }
        result.push_str(body);
        if let Some(ref suffix) = self.suffix {
            result.push_str(suffix);
        }
        result
    }

    /// 元の書式を保ちながら新しいバージョン文字列を組み立てる
    pub fn format_updated(&self, new_version: &str) -> String {
        self.try_format_updated(new_version)
            .unwrap_or_else(|| self.raw.clone())
    }
}

impl fmt::Display for VersionSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.raw)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_spec_kind_is_pinned() {
        assert!(VersionSpecKind::Exact.is_pinned());
        assert!(VersionSpecKind::GoPinned.is_pinned());
        assert!(!VersionSpecKind::Caret.is_pinned());
        assert!(!VersionSpecKind::Tilde.is_pinned());
        assert!(!VersionSpecKind::GreaterOrEqual.is_pinned());
        assert!(!VersionSpecKind::Range.is_pinned());
        assert!(!VersionSpecKind::Any.is_pinned());
    }

    #[test]
    fn test_version_spec_kind_any() {
        let spec = VersionSpec::new(VersionSpecKind::Any, "", "");
        assert_eq!(spec.kind, VersionSpecKind::Any);
        assert!(!spec.is_pinned());
        // Any は新しい値をそのまま返す
        assert_eq!(spec.format_updated("1.2.3"), "1.2.3");
    }

    #[test]
    fn test_version_spec_new() {
        let spec = VersionSpec::new(VersionSpecKind::Caret, "^1.2.3", "1.2.3");
        assert_eq!(spec.kind, VersionSpecKind::Caret);
        assert_eq!(spec.raw, "^1.2.3");
        assert_eq!(spec.version, "1.2.3");
        assert!(spec.prefix.is_none());
        assert!(spec.suffix.is_none());
    }

    #[test]
    fn test_version_spec_with_prefix() {
        let spec = VersionSpec::new(VersionSpecKind::Caret, "^1.2.3", "1.2.3").with_prefix("^");
        assert_eq!(spec.prefix, Some("^".to_string()));
    }

    /// 回帰テスト: 表示用バージョンは `v` 接頭辞だけを復元する。
    ///
    /// Go の更新先はレジストリが返す `v1.6.0` なのに、現在版は比較用に `v` を
    /// 剥がした `1.3.0` を出していたため `1.3.0 → v1.6.0` と不揃いになり、
    /// JSON の `from` / `to` も書式が食い違っていた。
    #[test]
    fn test_display_version_restores_v_prefix_only() {
        // Go: `v` はバージョン文字列の一部なので復元する
        let spec = VersionSpec::new(VersionSpecKind::Exact, "v1.3.0", "1.3.0").with_prefix("v");
        assert_eq!(spec.display_version(), "v1.3.0");

        let spec = VersionSpec::new(VersionSpecKind::Exact, "V1.3.0", "1.3.0").with_prefix("V");
        assert_eq!(spec.display_version(), "V1.3.0");

        // 演算子は制約であってバージョンの一部ではないので含めない
        for (raw, prefix) in [("^1.2.3", "^"), ("~1.2.3", "~"), (">=1.2.3", ">=")] {
            let spec = VersionSpec::new(VersionSpecKind::Caret, raw, "1.2.3").with_prefix(prefix);
            assert_eq!(spec.display_version(), "1.2.3", "raw={raw}");
        }

        // prefix なし・バージョンなしはそのまま
        let spec = VersionSpec::new(VersionSpecKind::Exact, "1.2.3", "1.2.3");
        assert_eq!(spec.display_version(), "1.2.3");
        let spec = VersionSpec::new(VersionSpecKind::Any, "", "");
        assert_eq!(spec.display_version(), "");
    }

    #[test]
    fn test_version_spec_with_suffix() {
        let spec = VersionSpec::new(VersionSpecKind::GoPinned, "v1.2.3 // pinned", "1.2.3")
            .with_suffix(" // pinned");
        assert_eq!(spec.suffix, Some(" // pinned".to_string()));
    }

    #[test]
    fn test_version_spec_is_pinned() {
        let pinned = VersionSpec::new(VersionSpecKind::Exact, "1.2.3", "1.2.3");
        assert!(pinned.is_pinned());

        let not_pinned = VersionSpec::new(VersionSpecKind::Caret, "^1.2.3", "1.2.3");
        assert!(!not_pinned.is_pinned());
    }

    #[test]
    fn test_format_updated_simple() {
        let spec = VersionSpec::new(VersionSpecKind::Exact, "1.2.3", "1.2.3");
        assert_eq!(spec.format_updated("2.0.0"), "2.0.0");
    }

    #[test]
    fn test_format_updated_with_prefix() {
        let spec = VersionSpec::new(VersionSpecKind::Caret, "^1.2.3", "1.2.3").with_prefix("^");
        assert_eq!(spec.format_updated("2.0.0"), "^2.0.0");
    }

    #[test]
    fn test_format_updated_with_prefix_and_suffix() {
        let spec = VersionSpec::new(VersionSpecKind::GoPinned, "v1.2.3 // pinned", "1.2.3")
            .with_prefix("v")
            .with_suffix(" // pinned");
        assert_eq!(spec.format_updated("2.0.0"), "v2.0.0 // pinned");
    }

    #[test]
    fn test_format_updated_tilde() {
        let spec = VersionSpec::new(VersionSpecKind::Tilde, "~1.2.3", "1.2.3").with_prefix("~");
        assert_eq!(spec.format_updated("1.3.0"), "~1.3.0");
    }

    /// 回帰テスト: Tilde の許容幅は元のセグメント数で決まるため、部分指定を
    /// 完全版へ展開して制約を黙って狭めない。
    /// - Composer `~4.4` = `>=4.4 <5.0` を `~6.4.7` (= `<6.5.0`) にしない
    /// - RubyGems `~> 7.0` = `>= 7.0, < 8.0` を `~> 7.1.3.2` (= `< 7.1.4`) にしない
    /// - npm / Cargo `~1` = `>=1.0.0 <2.0.0` を `~2.5.3` (= `<2.6.0`) にしない
    #[test]
    fn test_format_updated_tilde_preserves_segment_count() {
        let two_segment = VersionSpec::new(VersionSpecKind::Tilde, "~4.4", "4.4").with_prefix("~");
        assert_eq!(two_segment.format_updated("6.4.7"), "~6.4");

        let ruby = VersionSpec::new(VersionSpecKind::Tilde, "~> 7.0", "7.0").with_prefix("~> ");
        assert_eq!(ruby.format_updated("7.1.3.2"), "~> 7.1");

        let one_segment = VersionSpec::new(VersionSpecKind::Tilde, "~1", "1").with_prefix("~");
        assert_eq!(one_segment.format_updated("2.5.3"), "~2");
    }

    /// 更新先より元の指定の方がセグメントが多い場合は 0 埋めして幅を保つ。
    #[test]
    fn test_format_updated_tilde_pads_shorter_new_version() {
        let spec =
            VersionSpec::new(VersionSpecKind::Tilde, "~> 1.2.3.4", "1.2.3.4").with_prefix("~> ");
        assert_eq!(spec.format_updated("1.9"), "~> 1.9.0.0");
    }

    /// 更新先がプレリリース / ビルドメタデータを含む場合は、切り詰めで識別子を
    /// 落とさないよう完全版をそのまま使う。
    #[test]
    fn test_format_updated_tilde_keeps_full_version_for_prerelease() {
        let spec = VersionSpec::new(VersionSpecKind::Tilde, "~1.2", "1.2").with_prefix("~");
        assert_eq!(spec.format_updated("2.0.0-rc.1"), "~2.0.0-rc.1");
        assert_eq!(spec.format_updated("2.0.0+build.5"), "~2.0.0+build.5");
    }

    /// 元の指定が数値のみのセグメント列でない (Ruby のドット区切りプレリリース等)
    /// 場合も従来どおり完全版を使う。
    #[test]
    fn test_format_updated_tilde_keeps_full_version_for_non_numeric_current() {
        let spec = VersionSpec::new(VersionSpecKind::Tilde, "~> 1.0.0.pre", "1.0.0.pre")
            .with_prefix("~> ");
        assert_eq!(spec.format_updated("1.2.0"), "~> 1.2.0");
    }

    /// Swift の `.upToNextMinor` は常に 3 セグメントなので挙動が変わらない。
    #[test]
    fn test_format_updated_tilde_swift_three_segment_unchanged() {
        let spec = VersionSpec::new(VersionSpecKind::Tilde, "1.0.0", "1.0.0")
            .with_prefix(".upToNextMinor(from: \"")
            .with_suffix("\")");
        assert_eq!(
            spec.format_updated("2.1.0"),
            ".upToNextMinor(from: \"2.1.0\")"
        );
    }

    #[test]
    fn test_format_updated_greater_or_equal() {
        let spec =
            VersionSpec::new(VersionSpecKind::GreaterOrEqual, ">=1.2.3", "1.2.3").with_prefix(">=");
        assert_eq!(spec.format_updated("2.0.0"), ">=2.0.0");
    }

    #[test]
    fn test_try_format_updated_rejects_strict_greater() {
        // `>最新候補` に書き換えると、その最新候補自身が制約を満たさない
        let spec = VersionSpec::new(VersionSpecKind::Greater, ">1.2.3", "1.2.3").with_prefix(">");
        assert!(spec.try_format_updated("2.0.0").is_none());
    }

    #[test]
    fn test_try_format_updated_rejects_upper_bound_only_constraints() {
        // 上限だけの制約を書き換えると許容範囲を広げるため安全ではない
        let less = VersionSpec::new(VersionSpecKind::Less, "<2.0.0", "2.0.0").with_prefix("<");
        let less_or_equal =
            VersionSpec::new(VersionSpecKind::LessOrEqual, "<=2.0.0", "2.0.0").with_prefix("<=");

        assert!(less.try_format_updated("3.0.0").is_none());
        assert!(less_or_equal.try_format_updated("3.0.0").is_none());
    }

    #[test]
    fn test_format_updated_wildcard_major() {
        let spec = VersionSpec::new(VersionSpecKind::Wildcard, "1.*", "1");
        assert_eq!(spec.format_updated("2.3.4"), "2.*");
    }

    #[test]
    fn test_format_updated_wildcard_minor_x() {
        let spec = VersionSpec::new(VersionSpecKind::Wildcard, "1.2.x", "1.2");
        assert_eq!(spec.format_updated("2.3.4"), "2.3.x");
    }

    #[test]
    fn test_format_updated_wildcard_multiple_positions() {
        let spec = VersionSpec::new(VersionSpecKind::Wildcard, "1.x.x", "1");
        assert_eq!(spec.format_updated("2.3.4"), "2.x.x");
    }

    #[test]
    fn test_format_updated_wildcard_gradle_prefix() {
        let spec = VersionSpec::new(VersionSpecKind::Wildcard, "1.+", "1");
        assert_eq!(spec.format_updated("2.3.4"), "2.+");
    }

    #[test]
    fn test_format_updated_wildcard_preserves_v_prefix() {
        let spec = VersionSpec::new(VersionSpecKind::Wildcard, "v1.*", "1");
        assert_eq!(spec.format_updated("2.3.4"), "v2.*");
    }

    #[test]
    fn test_format_updated_floating_wildcard_stays_same() {
        let spec = VersionSpec::new(VersionSpecKind::Wildcard, "*", "");
        assert_eq!(spec.format_updated("2.3.4"), "*");
    }

    #[test]
    fn test_format_updated_floating_multi_segment_wildcard_stays_same() {
        let spec = VersionSpec::new(VersionSpecKind::Wildcard, "x.x", "");
        assert_eq!(spec.format_updated("2.3.4"), "x.x");
    }

    #[test]
    fn test_try_format_updated_range_replaces_lower_bound() {
        let spec = VersionSpec::new(VersionSpecKind::Range, ">=1.0,<2.0", "1.0");
        assert_eq!(
            spec.try_format_updated("1.9.3").as_deref(),
            Some(">=1.9.3,<2.0")
        );
    }

    #[test]
    fn test_try_format_updated_range_replaces_bare_lower_bound() {
        let spec = VersionSpec::new(VersionSpecKind::Range, "1.2 <2.0.0", "1.2.0");
        assert_eq!(
            spec.try_format_updated("1.9.3").as_deref(),
            Some("1.9 <2.0.0")
        );
    }

    #[test]
    fn test_try_format_updated_range_preserves_spacing() {
        let spec = VersionSpec::new(VersionSpecKind::Range, ">= 1.0, < 2.0", "1.0");
        assert_eq!(
            spec.try_format_updated("1.9.3").as_deref(),
            Some(">= 1.9.3, < 2.0")
        );
    }

    #[test]
    fn test_try_format_updated_range_updates_inclusive_lower_bound_when_ordered_later() {
        let spec = VersionSpec::new(VersionSpecKind::Range, "<=2.0,>=1.0", "1.0");
        assert_eq!(
            spec.try_format_updated("1.9.3").as_deref(),
            Some("<=2.0,>=1.9.3")
        );
    }

    #[test]
    fn test_try_format_updated_range_rejects_exclusive_lower_bound() {
        // `>最新候補` に書き換えると、その最新候補自身が制約を満たさない
        let spec = VersionSpec::new(VersionSpecKind::Range, ">1.0,<2.0", "1.0");
        assert!(spec.try_format_updated("1.9.3").is_none());
    }

    #[test]
    fn test_try_format_updated_range_hyphen_updates_left_side() {
        let spec = VersionSpec::new(VersionSpecKind::Range, "1.0 - 2.0", "1.0");
        assert_eq!(
            spec.try_format_updated("1.9.3").as_deref(),
            Some("1.9.3 - 2.0")
        );
    }

    #[test]
    fn test_try_format_updated_range_maven_updates_lower_bound() {
        let spec = VersionSpec::new(VersionSpecKind::Range, "[1.0,2.0)", "1.0");
        assert_eq!(
            spec.try_format_updated("1.9.3").as_deref(),
            Some("[1.9.3,2.0)")
        );
    }

    #[test]
    fn test_try_format_updated_range_maven_open_upper_updates_lower_bound() {
        let spec = VersionSpec::new(VersionSpecKind::Range, "[1.0,)", "1.0");
        assert_eq!(
            spec.try_format_updated("1.9.3").as_deref(),
            Some("[1.9.3,)")
        );
    }

    #[test]
    fn test_try_format_updated_range_partial_version_preserves_shape() {
        let spec = VersionSpec::new(VersionSpecKind::Range, "1.2", "1.2.0");
        assert_eq!(spec.try_format_updated("2.3.4").as_deref(), Some("2.3"));
    }

    #[test]
    fn test_try_format_updated_range_equal_partial_preserves_shape() {
        let spec = VersionSpec::new(VersionSpecKind::Range, "=1.2", "1.2.0");
        assert_eq!(spec.try_format_updated("2.3.4").as_deref(), Some("=2.3"));
    }

    #[test]
    fn test_try_format_updated_range_python_prefix_wildcard() {
        let spec = VersionSpec::new(VersionSpecKind::Range, "==1.2.*", "1.2");
        assert_eq!(spec.try_format_updated("2.3.4").as_deref(), Some("==2.3.*"));
    }

    #[test]
    fn test_try_format_updated_range_rejects_not_equal() {
        let spec = VersionSpec::new(VersionSpecKind::Range, "!=1.2.3", "1.2.3");
        assert!(spec.try_format_updated("2.0.0").is_none());
    }

    #[test]
    fn test_try_format_updated_range_rejects_or_constraint() {
        let spec = VersionSpec::new(VersionSpecKind::Range, "^1 || ^2", "1");
        assert!(spec.try_format_updated("2.0.0").is_none());
    }

    #[test]
    fn test_try_format_updated_range_rejects_single_pipe_or_constraint() {
        let spec = VersionSpec::new(VersionSpecKind::Range, "^1 | ^2", "1");
        assert!(spec.try_format_updated("2.0.0").is_none());
    }

    /// 回帰テスト: 比較演算子やカンマを含む単一パイプ OR も拒否する。
    ///
    /// 以前は `||` しか見ておらず、`^1 | ^2` が弾かれていたのは
    /// `format_partial_version_like` の数値セグメント検査に引っかかる副作用に
    /// すぎなかった。`<` / `>` / `,` が入ると穴が開き、片方の分岐の下限だけを
    /// 進めて OR の意味を変える (`>=1.0 <2.0 | >=3.0` の 1.x 分岐が消える) か、
    /// 別分岐の上限で候補を不当に絞っていた。
    #[test]
    fn test_try_format_updated_range_rejects_single_pipe_with_comparators() {
        for raw in [
            ">=1.0 <2.0 | >=3.0",
            ">=1.0 <2.0 | >=3.0 <4.0",
            ">=1.0,<2.0|>=3.0",
            "5.5.*|>=6.0",
        ] {
            let spec = VersionSpec::new(VersionSpecKind::Range, raw, "1.0");
            assert!(
                spec.try_format_updated("3.5.0").is_none(),
                "単一パイプ OR は書き換え不可であるべき: {raw}"
            );
        }
    }

    /// 回帰テスト: comparator set に埋め込まれた tilde の下限がプレリリースや
    /// ビルドメタデータを含む場合、セグメント数を保てなくても完全版へ
    /// フォールバックする。
    ///
    /// 単体 Tilde (`~1.2.3-rc.1`) は完全版フォールバックが効くのに、上限が付いて
    /// Range 扱いになった途端に書き換え不能になる非対称があった。
    #[test]
    fn test_format_range_like_embedded_tilde_with_prerelease_falls_back() {
        let spec = VersionSpec::new(VersionSpecKind::Range, "~1.2.3-rc.1, <2.0", "1.2.3-rc.1");
        assert_eq!(
            spec.try_format_updated("1.9.0-rc.2"),
            Some("~1.9.0-rc.2, <2.0".to_string())
        );

        let spec = VersionSpec::new(VersionSpecKind::Range, "~1.2.3+build, <2.0", "1.2.3+build");
        assert_eq!(
            spec.try_format_updated("1.9.0"),
            Some("~1.9.0, <2.0".to_string())
        );

        // 数値だけの下限は従来どおりセグメント数を保つ
        let spec = VersionSpec::new(VersionSpecKind::Range, "~1.2, <2.0", "1.2");
        assert_eq!(
            spec.try_format_updated("1.9.3"),
            Some("~1.9, <2.0".to_string())
        );
    }

    #[test]
    fn test_try_format_updated_range_maven_lower_open_returns_none() {
        // Maven 下限なし `(,2.0]` は安全に書き換えられないため None を返す
        let spec = VersionSpec::new(VersionSpecKind::Range, "(,2.0]", "0.0");
        assert!(spec.try_format_updated("1.5.0").is_none());
    }

    #[test]
    fn test_try_format_updated_range_maven_lower_open_exclusive_returns_none() {
        // Maven 下限なし `(,2.0)` も同様に None を返す
        let spec = VersionSpec::new(VersionSpecKind::Range, "(,2.0)", "0.0");
        assert!(spec.try_format_updated("1.5.0").is_none());
    }

    #[test]
    fn test_try_format_updated_range_arbitrary_equality_returns_none() {
        // `===` 付きレンジは安全に書き換えられない
        let spec = VersionSpec::new(VersionSpecKind::Range, "===v1.2-custom", "1.2");
        assert!(spec.try_format_updated("2.0.0").is_none());
    }

    #[test]
    fn test_display_trait() {
        let spec = VersionSpec::new(VersionSpecKind::Caret, "^1.2.3", "1.2.3");
        assert_eq!(format!("{}", spec), "^1.2.3");
    }

    #[test]
    fn test_version_spec_equality() {
        let spec1 = VersionSpec::new(VersionSpecKind::Caret, "^1.2.3", "1.2.3");
        let spec2 = VersionSpec::new(VersionSpecKind::Caret, "^1.2.3", "1.2.3");
        assert_eq!(spec1, spec2);
    }

    #[test]
    fn test_version_spec_clone() {
        let spec = VersionSpec::new(VersionSpecKind::Caret, "^1.2.3", "1.2.3");
        let cloned = spec.clone();
        assert_eq!(spec, cloned);
    }

    #[test]
    fn test_format_wildcard_like_v_prefix_upper() {
        // 大文字 V プレフィックス付きワイルドカードの更新
        let spec = VersionSpec::new(VersionSpecKind::Wildcard, "V1.*", "1");
        assert_eq!(spec.format_updated("2.3.4"), "V2.*");
    }

    #[test]
    fn test_format_range_like_maven_alt_brackets() {
        // Maven の反転ブラケット記法 ]...[ は下限排他なので安全に書き換えられない
        let spec = VersionSpec::new(VersionSpecKind::Range, "]1.0,2.0[", "1.0");
        assert!(spec.try_format_updated("1.5.0").is_none());
    }

    #[test]
    fn test_format_range_like_swift_half_open() {
        // Swift の半開区間 ..< は下限のみ更新し、上限と演算子を保持する
        let spec = VersionSpec::new(VersionSpecKind::Range, "1.0.0..<2.0.0", "1.0.0");
        assert_eq!(
            spec.try_format_updated("1.5.0").as_deref(),
            Some("1.5.0..<2.0.0")
        );
    }

    #[test]
    fn test_format_range_like_swift_closed() {
        // Swift の閉区間 ... は下限のみ更新し、上限と演算子を保持する
        let spec = VersionSpec::new(VersionSpecKind::Range, "1.0.0...2.0.0", "1.0.0");
        assert_eq!(
            spec.try_format_updated("1.5.0").as_deref(),
            Some("1.5.0...2.0.0")
        );
    }

    #[test]
    fn test_format_range_like_comma_not_equal_rejected() {
        // カンマ区切りの不等号制約は安全に書き換えられないため None を返す
        let spec = VersionSpec::new(VersionSpecKind::Range, ",!=1.2.3", "1.2.3");
        assert!(spec.try_format_updated("2.0.0").is_none());
    }

    #[test]
    fn test_format_range_like_spaced_not_equal_rejected() {
        // PEP 440 / Composer は `, !=` や空白区切りの `!=` を許容するが、
        // 除外候補を選ばない保証がないため自動更新では拒否する
        let comma_spaced =
            VersionSpec::new(VersionSpecKind::Range, ">= 1.0, != 1.5.0, < 2.0", "1.0");
        let space_separated = VersionSpec::new(VersionSpecKind::Range, ">=1.0 !=1.5.0 <2.0", "1.0");

        assert!(comma_spaced.try_format_updated("1.9.0").is_none());
        assert!(space_separated.try_format_updated("1.9.0").is_none());
    }

    #[test]
    fn test_format_range_like_shell_not_equal_rejected() {
        // Composer は not-equal を `<>` でも綴れる。`!=` と同様に、除外制約を含む
        // レンジは安全に書き換えられないため None を返す (下限だけ進めると除外
        // バージョンを選んで充足不能な制約を書き戻す恐れがある)。
        let comma = VersionSpec::new(VersionSpecKind::Range, ">=1.0,<>1.5.0,<2.0", "1.0");
        let spaced = VersionSpec::new(VersionSpecKind::Range, ">=1.0 <>1.5.0 <2.0", "1.0");

        assert!(comma.try_format_updated("1.9.0").is_none());
        assert!(spaced.try_format_updated("1.9.0").is_none());
    }

    #[test]
    fn test_try_format_updated_any_empty_prefix_suffix() {
        // Any 種別で prefix/suffix が空の場合、新バージョンをそのまま返す
        let spec = VersionSpec {
            kind: VersionSpecKind::Any,
            raw: String::new(),
            version: String::new(),
            prefix: None,
            suffix: None,
            rejected_versions: Vec::new(),
        };
        assert_eq!(spec.try_format_updated("1.2.3").as_deref(), Some("1.2.3"));
    }

    #[test]
    fn test_format_range_like_v_prefix_in_range() {
        // レンジ内の v プレフィックスが保持されて下限のみ更新される
        let spec = VersionSpec::new(VersionSpecKind::Range, ">=v1.0,<v2.0", "1.0");
        assert_eq!(
            spec.try_format_updated("1.5.0").as_deref(),
            Some(">=v1.5.0,<v2.0")
        );
    }

    #[test]
    fn test_serde_version_spec_kind() {
        let kind = VersionSpecKind::GreaterOrEqual;
        let json = serde_json::to_string(&kind).unwrap();
        assert_eq!(json, "\"greater_or_equal\"");

        let parsed: VersionSpecKind = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, kind);
    }

    #[test]
    fn test_serde_version_spec() {
        let spec = VersionSpec::new(VersionSpecKind::Caret, "^1.2.3", "1.2.3").with_prefix("^");
        let json = serde_json::to_string(&spec).unwrap();
        let parsed: VersionSpec = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, spec);
    }

    #[test]
    fn test_serde_version_spec_rejected_versions() {
        let spec = VersionSpec::new(VersionSpecKind::Range, "[1.0,2.0[", "1.5")
            .with_rejected_versions(["1.6", "1.7"]);
        let json = serde_json::to_string(&spec).unwrap();
        let parsed: VersionSpec = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.rejected_versions, vec!["1.6", "1.7"]);
    }

    // --- Swift レンジ演算子の追加テスト ---

    #[test]
    fn test_format_range_like_swift_half_open_two_segment() {
        // 2セグメントバージョンの半開区間
        let spec = VersionSpec::new(VersionSpecKind::Range, "1.0..<2.0", "1.0");
        assert_eq!(spec.try_format_updated("1.5").as_deref(), Some("1.5..<2.0"));
    }

    #[test]
    fn test_format_range_like_swift_closed_different_major() {
        // メジャーバージョンが異なる閉区間
        let spec = VersionSpec::new(VersionSpecKind::Range, "2.0.0...3.0.0", "2.0.0");
        assert_eq!(
            spec.try_format_updated("2.5.0").as_deref(),
            Some("2.5.0...3.0.0")
        );
    }

    #[test]
    fn test_format_range_like_ruby_compound_comma() {
        // Ruby スタイルのカンマ区切り複合制約
        let spec = VersionSpec::new(VersionSpecKind::Range, ">= 1.0, < 2.0", "1.0");
        assert_eq!(
            spec.try_format_updated("1.8.0").as_deref(),
            Some(">= 1.8.0, < 2.0")
        );
    }

    #[test]
    fn test_format_range_like_maven_closed_brackets() {
        // Maven 閉区間 [A,B] の下限のみ更新
        let spec = VersionSpec::new(VersionSpecKind::Range, "[1.0,2.0]", "1.0");
        assert_eq!(
            spec.try_format_updated("1.5.0").as_deref(),
            Some("[1.5.0,2.0]")
        );
    }

    #[test]
    fn test_format_range_like_hyphen_range_preserves_spacing() {
        // ハイフンレンジのスペーシングが保持される
        let spec = VersionSpec::new(VersionSpecKind::Range, "1.0.0 - 3.0.0", "1.0.0");
        assert_eq!(
            spec.try_format_updated("2.0.0").as_deref(),
            Some("2.0.0 - 3.0.0")
        );
    }

    #[test]
    fn test_format_wildcard_like_gradle_two_segment() {
        // Gradle の 2セグメント + ワイルドカード
        let spec = VersionSpec::new(VersionSpecKind::Wildcard, "5.3.+", "5.3");
        assert_eq!(spec.format_updated("6.1.0"), "6.1.+");
    }

    #[test]
    fn test_format_wildcard_like_caret_prefix() {
        // npm の caret + x-range は `^` を保持して形を保って更新する
        let spec = VersionSpec::new(VersionSpecKind::Wildcard, "^1.x", "1");
        assert_eq!(spec.format_updated("2.3.4"), "^2.x");
    }

    #[test]
    fn test_format_wildcard_like_tilde_prefix_minor() {
        // npm の tilde + x-range は `~` を保持する
        let spec = VersionSpec::new(VersionSpecKind::Wildcard, "~1.2.x", "1.2");
        assert_eq!(spec.format_updated("2.3.4"), "~2.3.x");
    }

    #[test]
    fn test_format_wildcard_like_no_operator_unchanged() {
        // 演算子なしの既存ワイルドカードは従来どおりの挙動 (op_prefix が空)
        let spec = VersionSpec::new(VersionSpecKind::Wildcard, "1.x", "1");
        assert_eq!(spec.format_updated("2.3.4"), "2.x");
    }

    #[test]
    fn test_format_range_like_pep440_compatible_release() {
        // PEP 440 の `~=` はセグメント数を保って下限を進める。
        // セグメント数を変えると上限の意味が変わる (`~=1.2` の <2.0 が `~=1.9.0` だと <1.10.0)。
        let three = VersionSpec::new(VersionSpecKind::Range, "~=1.2.3", "1.2.3");
        assert_eq!(three.format_updated("1.2.9"), "~=1.2.9");
        let two = VersionSpec::new(VersionSpecKind::Range, "~=1.2", "1.2");
        assert_eq!(two.format_updated("1.9.5"), "~=1.9");
    }

    #[test]
    fn test_format_range_like_pep440_compatible_release_compound() {
        // `~=1.2, <1.5` のような複合制約は ~= 分岐に横取りされず、下限側のみ進める (回帰防止)
        let spec = VersionSpec::new(VersionSpecKind::Range, "~=1.2, <1.5", "1.2");
        assert_eq!(spec.format_updated("1.4"), "~=1.4, <1.5");
    }
}
