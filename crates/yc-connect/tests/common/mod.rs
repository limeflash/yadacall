//! A tiny HTTP/1.1 server standing in for Google's endpoints.
#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// A parsed request.
#[derive(Debug, Clone)]
pub struct Req {
    pub method: String,
    pub path: String,
    pub query: HashMap<String, String>,
    pub headers: HashMap<String, String>,
    pub body: String,
}

impl Req {
    /// Form-encoded body as a map.
    pub fn form(&self) -> HashMap<String, String> {
        url::form_urlencoded::parse(self.body.as_bytes())
            .into_owned()
            .collect()
    }

    pub fn bearer(&self) -> Option<&str> {
        self.headers
            .get("authorization")
            .and_then(|v| v.strip_prefix("Bearer "))
    }
}

pub struct Resp {
    pub status: u16,
    pub content_type: &'static str,
    pub body: String,
}

impl Resp {
    pub fn json(status: u16, body: serde_json::Value) -> Self {
        Self {
            status,
            content_type: "application/json",
            body: body.to_string(),
        }
    }
    pub fn text(status: u16, content_type: &'static str, body: &str) -> Self {
        Self {
            status,
            content_type,
            body: body.to_string(),
        }
    }
}

pub struct Mock {
    pub base: String,
    pub requests: Arc<Mutex<Vec<Req>>>,
}

impl Mock {
    pub async fn start<F>(handler: F) -> Mock
    where
        F: Fn(&Req) -> Resp + Send + Sync + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let log = requests.clone();
        let handler = Arc::new(handler);
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                let (handler, log) = (handler.clone(), log.clone());
                tokio::spawn(async move {
                    let Some(req) = read_request(&mut stream).await else {
                        return;
                    };
                    log.lock().unwrap().push(req.clone());
                    let resp = handler(&req);
                    let reason = if resp.status < 300 { "OK" } else { "Error" };
                    let out = format!(
                        "HTTP/1.1 {} {reason}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        resp.status,
                        resp.content_type,
                        resp.body.len(),
                        resp.body
                    );
                    let _ = stream.write_all(out.as_bytes()).await;
                    let _ = stream.shutdown().await;
                });
            }
        });
        Mock { base, requests }
    }

    /// Requests whose path equals `path`.
    pub fn hits(&self, path: &str) -> Vec<Req> {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.path == path)
            .cloned()
            .collect()
    }
}

async fn read_request(stream: &mut tokio::net::TcpStream) -> Option<Req> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break pos;
        }
        let n = stream.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
    let mut lines = head.lines();
    let mut first = lines.next()?.split(' ');
    let method = first.next()?.to_string();
    let target = first.next()?.to_string();
    let headers: HashMap<String, String> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
        .collect();
    let len: usize = headers
        .get("content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let mut body = buf[head_end + 4..].to_vec();
    while body.len() < len {
        let n = stream.read(&mut chunk).await.ok()?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    let parsed = url::Url::parse(&format!("http://x{target}")).ok()?;
    Some(Req {
        method,
        path: parsed.path().to_string(),
        query: parsed.query_pairs().into_owned().collect(),
        headers,
        body: String::from_utf8_lossy(&body).into_owned(),
    })
}
