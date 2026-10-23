//! テスト用の HTTP サーバ。決めた順の要求に、決めた応答を 1 つずつ返す
use std::io::{Read, Write};

/// `responses` の (要求の先頭行の前方一致, ステータス行の残りとヘッダ, 本文) を順に返す
/// サーバを立て、その起点の URL と、全要求を受け終えるスレッドを返す。想定と違う要求が
/// 来たらスレッドが panic するので、テストの最後に `join` して確かめる。
pub(crate) fn serve_responses(
    responses: Vec<(&'static str, &'static str, &'static str)>,
) -> (String, std::thread::JoinHandle<()>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let handle = std::thread::spawn(move || {
        for (request_start, status_and_headers, body) in responses {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 4096];
            let read = stream.read(&mut request).unwrap();
            assert!(
                String::from_utf8_lossy(&request[..read]).starts_with(request_start),
                "unexpected request: {}",
                String::from_utf8_lossy(&request[..read])
            );
            let response = format!(
                "HTTP/1.1 {status_and_headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).unwrap();
        }
    });
    (format!("http://{address}"), handle)
}
