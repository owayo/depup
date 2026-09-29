//! Cargo.lock の `--age` 差し戻しで使う部品。
//!
//! 1 件ずつの `cargo update --precise` では戻せない crate (互いを `=` で固定し合う一族)
//! を、利用者の Cargo.toml に触れずに workspace の写しでまとめて解き直す
//! ([`batch::resolve_together`])。そのための依存グラフ・semver 系列の判定・一時写しと、
//! install 後の lock と表示の突き合わせ ([`report::lock_mismatches`]) を置く。

pub mod batch;
pub mod graph;
pub mod report;
pub mod scratch;
pub mod series;
