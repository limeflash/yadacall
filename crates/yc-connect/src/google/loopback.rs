//! The loopback redirect receiver of the installed-app OAuth flow
//! (`http://127.0.0.1:<random port>/callback`).

use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::error::{ConnectError, ConnectResult};

const SUCCESS_PAGE: &str = "<!doctype html><html lang=\"ru\"><head><meta charset=\"utf-8\"><title>YadaCall</title>\
<style>body{font-family:-apple-system,Segoe UI,sans-serif;display:grid;place-items:center;height:100vh;margin:0;background:#f6f6f7;color:#1b1b1f}\
main{text-align:center}h1{font-size:22px;font-weight:600}p{color:#666}</style></head>\
<body><main><h1>Готово, можно вернуться в YadaCall</h1><p>Эту вкладку можно закрыть.</p></main></body></html>";

const FAILURE_PAGE: &str = "<!doctype html><html lang=\"ru\"><head><meta charset=\"utf-8\"><title>YadaCall</title>\
<style>body{font-family:-apple-system,Segoe UI,sans-serif;display:grid;place-items:center;height:100vh;margin:0;background:#f6f6f7;color:#1b1b1f}\
main{text-align:center}h1{font-size:22px;font-weight:600}p{color:#666}</style></head>\
<body><main><h1>Вход не выполнен</h1><p>Вернитесь в YadaCall и попробуйте ещё раз.</p></main></body></html>";

/// A bound loopback listener waiting for Google's redirect.
#[derive(Debug)]
pub struct CallbackServer {
    listener: TcpListener,
    port: u16,
}

impl CallbackServer {
    /// Binds `127.0.0.1` on a random free port.
    pub async fn bind() -> ConnectResult<Self> {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
        let port = listener.local_addr()?.port();
        Ok(Self { listener, port })
    }

    /// The `redirect_uri` to send to Google.
    pub fn redirect_uri(&self) -> String {
        format!("http://127.0.0.1:{}/callback", self.port)
    }

    /// Waits for the redirect carrying `expected_state` and returns the authorization code.
    /// Requests with a wrong state or another path are answered and ignored.
    pub async fn wait_for_code(
        self,
        expected_state: &str,
        cancel: &CancellationToken,
        timeout: Duration,
    ) -> ConnectResult<String> {
        let (tx, mut rx) = mpsc::channel::<ConnectResult<String>>(4);
        let deadline = tokio::time::sleep(timeout);
        tokio::pin!(deadline);
        loop {
            tokio::select! {
                () = cancel.cancelled() => return Err(ConnectError::Cancelled),
                () = &mut deadline => return Err(ConnectError::Timeout),
                accepted = self.listener.accept() => {
                    let Ok((stream, _)) = accepted else { continue };
                    let state = expected_state.to_string();
                    let tx = tx.clone();
                    tokio::spawn(async move {
                        if let Some(result) = handle(stream, &state).await {
                            let _ = tx.send(result).await;
                        }
                    });
                }
                Some(result) = rx.recv() => return result,
            }
        }
    }
}

/// Answers one connection; `Some` when it carried a decisive callback.
async fn handle(mut stream: TcpStream, expected_state: &str) -> Option<ConnectResult<String>> {
    let head = tokio::time::timeout(Duration::from_secs(5), read_head(&mut stream))
        .await
        .ok()??;
    let target = head
        .lines()
        .next()?
        .strip_prefix("GET ")?
        .split(' ')
        .next()?;
    let url = Url::parse(&format!("http://127.0.0.1{target}")).ok()?;
    if url.path() != "/callback" {
        respond(&mut stream, 404, "text/plain; charset=utf-8", "not found").await;
        return None;
    }
    let param = |name: &str| {
        url.query_pairs()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.into_owned())
    };
    if param("state").as_deref() != Some(expected_state) {
        respond(&mut stream, 400, "text/html; charset=utf-8", FAILURE_PAGE).await;
        return None;
    }
    if let Some(error) = param("error") {
        respond(&mut stream, 200, "text/html; charset=utf-8", FAILURE_PAGE).await;
        return Some(Err(ConnectError::Denied(error)));
    }
    match param("code") {
        Some(code) if !code.is_empty() => {
            respond(&mut stream, 200, "text/html; charset=utf-8", SUCCESS_PAGE).await;
            Some(Ok(code))
        }
        _ => {
            respond(&mut stream, 400, "text/html; charset=utf-8", FAILURE_PAGE).await;
            None
        }
    }
}

async fn read_head(stream: &mut TcpStream) -> Option<String> {
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    while buf.len() < 16 * 1024 {
        let n = stream.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
    }
    String::from_utf8(buf).ok()
}

async fn respond(stream: &mut TcpStream, status: u16, content_type: &str, body: &str) {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        _ => "Not Found",
    };
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.shutdown().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn get(uri: &str) -> String {
        reqwest::get(uri).await.unwrap().text().await.unwrap()
    }

    #[tokio::test]
    async fn returns_code_and_ignores_wrong_state() {
        let server = CallbackServer::bind().await.unwrap();
        let base = server.redirect_uri();
        let cancel = CancellationToken::new();
        let waiter = tokio::spawn(async move {
            server
                .wait_for_code("good", &cancel, Duration::from_secs(5))
                .await
        });
        assert!(
            get(&format!("{base}?code=evil&state=bad"))
                .await
                .contains("Вход не выполнен")
        );
        assert_eq!(
            get(&base.replace("/callback", "/favicon.ico")).await,
            "not found"
        );
        assert!(
            get(&format!("{base}?code=abc%2F1&state=good"))
                .await
                .contains("Готово")
        );
        assert_eq!(waiter.await.unwrap().unwrap(), "abc/1");
    }

    #[tokio::test]
    async fn denied_timeout_and_cancel() {
        let server = CallbackServer::bind().await.unwrap();
        let base = server.redirect_uri();
        let cancel = CancellationToken::new();
        let c2 = cancel.clone();
        let waiter =
            tokio::spawn(
                async move { server.wait_for_code("s", &c2, Duration::from_secs(5)).await },
            );
        get(&format!("{base}?error=access_denied&state=s")).await;
        assert!(matches!(
            waiter.await.unwrap(),
            Err(ConnectError::Denied(e)) if e == "access_denied"
        ));

        let server = CallbackServer::bind().await.unwrap();
        let r = server
            .wait_for_code("s", &CancellationToken::new(), Duration::from_millis(50))
            .await;
        assert!(matches!(r, Err(ConnectError::Timeout)));

        let server = CallbackServer::bind().await.unwrap();
        cancel.cancel();
        let r = server
            .wait_for_code("s", &cancel, Duration::from_secs(5))
            .await;
        assert!(matches!(r, Err(ConnectError::Cancelled)));
    }
}
