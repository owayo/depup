//! 統合テストで共有する補助
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

/// 実行権限 `mode` を付けて `content` を書く。書いた直後のファイルを exec すると、並列に走る
/// ほかのテストの fork が、このプロセスの書き込み用の fd の写しを exec までの間持っていることが
/// あり、Linux は `ETXTBSY` (Text file busy, os error 26) で拒否する。子プロセスに書かせて、
/// このプロセスではファイルを書き込み用に開かない (ライブラリの `test_support::exec` と同じ)。
pub fn write_script(path: &Path, content: &str, mode: u32) {
    let mut writer = Command::new("sh")
        .args(["-c", "cat > \"$1\" && chmod \"$2\" \"$1\"", "sh"])
        .arg(path)
        .arg(format!("{mode:o}"))
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    // 標準入力は書き終えたら閉じる (閉じないと cat が終わらない)。書き込みに失敗しても、子を待ってから報告する
    let sent = {
        let mut stdin = writer.stdin.take().unwrap();
        stdin.write_all(content.as_bytes())
    };
    let status = writer.wait().unwrap();
    assert!(status.success(), "cannot write {}", path.display());
    sent.unwrap();
}
