//! "Read on your phone": a small web server on the home network that serves the offline library
//! (downloaded chapters) to a phone browser. Every request needs the random key from the QR code,
//! so other devices on the Wi-Fi can't browse it; only files inside the downloads folder are served.

use std::io::Cursor;
use std::net::{IpAddr, UdpSocket};
use std::sync::Arc;

use reqwest::Url;
use serde::Serialize;
use tiny_http::{Header, Response, Server};

use crate::core::models::MangaKey;
use crate::downloads::Downloads;

pub const PORT: u16 = 8787;
const INDEX: &str = include_str!("phone.html");

pub struct PhoneServer {
    server: Arc<Server>,
    pub url: String,
}

impl Drop for PhoneServer {
    fn drop(&mut self) {
        self.server.unblock(); // ends the request loop thread
    }
}

/// This PC's address on the local network (no packet is sent: UDP `connect` only picks a route).
pub fn lan_ip() -> Option<IpAddr> {
    let s = UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("8.8.8.8:80").ok()?;
    Some(s.local_addr().ok()?.ip())
}

/// A fresh random access key (hex).
pub fn new_key() -> String {
    let mut b = [0u8; 12];
    let _ = getrandom::fill(&mut b);
    b.iter().map(|x| format!("{x:02x}")).collect()
}

pub fn start(downloads: Arc<Downloads>, key: String) -> anyhow::Result<PhoneServer> {
    let server = Arc::new(Server::http(("0.0.0.0", PORT)).map_err(|e| anyhow::anyhow!("{e}"))?);
    let ip = lan_ip().map_or("127.0.0.1".to_string(), |ip| ip.to_string());
    let url = format!("http://{ip}:{PORT}/?k={key}");
    let s = server.clone();
    std::thread::Builder::new().name("phone-server".into()).spawn(move || {
        for req in s.incoming_requests() {
            let resp = handle(&downloads, &key, req.url());
            if let Err(e) = req.respond(resp) {
                tracing::debug!(error = %e, "phone client went away");
            }
        }
    })?;
    Ok(PhoneServer { server, url })
}

#[derive(Serialize)]
struct Work {
    source: String,
    id: String,
    title: String,
    chapters: Vec<Ch>,
}

#[derive(Serialize)]
struct Ch {
    id: String,
    label: String,
    pages: usize,
}

type Resp = Response<Cursor<Vec<u8>>>;

fn header(k: &str, v: &str) -> Header {
    Header::from_bytes(k.as_bytes(), v.as_bytes()).expect("valid header")
}

fn text(status: u16, body: &str) -> Resp {
    Response::from_string(body).with_status_code(status).with_header(header("Content-Type", "text/plain; charset=utf-8"))
}

fn file(path: std::path::PathBuf) -> Resp {
    match std::fs::read(&path) {
        Ok(bytes) => {
            let mime = image::guess_format(&bytes).map_or("application/octet-stream", |f| f.to_mime_type());
            Response::from_data(bytes)
                .with_header(header("Content-Type", mime))
                .with_header(header("Cache-Control", "private, max-age=86400"))
        }
        Err(_) => text(404, "not found"),
    }
}

fn handle(dl: &Downloads, key: &str, raw: &str) -> Resp {
    let Ok(url) = Url::parse(&format!("http://phone{raw}")) else {
        return text(400, "bad request");
    };
    let q = |name: &str| url.query_pairs().find(|(k, _)| k == name).map(|(_, v)| v.into_owned());
    if q("k").as_deref() != Some(key) {
        return text(403, "Scan the QR code in KumaDoujin › Settings › Read on your phone.");
    }
    let work = || Some(MangaKey { source: q("s")?, id: q("m")? });
    match url.path() {
        "/" => Response::from_string(INDEX).with_header(header("Content-Type", "text/html; charset=utf-8")),
        "/api/library" => {
            let list: Vec<Work> = dl
                .list()
                .into_iter()
                .map(|e| Work {
                    source: e.manga.key.source.clone(),
                    id: e.manga.key.id.clone(),
                    title: e.manga.titles.display().to_string(),
                    chapters: e
                        .chapters
                        .iter()
                        .map(|c| Ch { id: c.chapter.id.clone(), label: c.chapter.label(), pages: c.pages })
                        .collect(),
                })
                .collect();
            Response::from_data(serde_json::to_vec(&list).unwrap_or_default())
                .with_header(header("Content-Type", "application/json"))
        }
        "/cover" => match work() {
            Some(k) => file(dl.cover_path(&k)),
            None => text(400, "bad request"),
        },
        "/page" => match (work(), q("c"), q("i").and_then(|i| i.parse::<usize>().ok())) {
            // paths are built from hashed folder names, so the values can't point outside the library
            (Some(k), Some(c), Some(i)) => file(dl.page_path(&k, &c, i)),
            _ => text(400, "bad request"),
        },
        _ => text(404, "not found"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn needs_the_key_and_serves_library_files() {
        let root = std::env::temp_dir().join(format!("kuma-phone-{}", std::process::id()));
        let dl = Downloads::new(root.clone());
        let key = MangaKey { source: "s".into(), id: "m".into() };
        std::fs::create_dir_all(dl.chapter_dir(&key, "c1")).unwrap();
        let png = {
            let mut b = Cursor::new(Vec::new());
            image::RgbaImage::new(1, 1).write_to(&mut b, image::ImageFormat::Png).unwrap();
            b.into_inner()
        };
        std::fs::write(dl.page_path(&key, "c1", 0), &png).unwrap();

        assert_eq!(handle(&dl, "abc", "/api/library").status_code().0, 403);
        assert_eq!(handle(&dl, "abc", "/api/library?k=nope").status_code().0, 403);
        assert_eq!(handle(&dl, "abc", "/?k=abc").status_code().0, 200);
        assert_eq!(handle(&dl, "abc", "/page?k=abc&s=s&m=m&c=c1&i=0").status_code().0, 200);
        assert_eq!(handle(&dl, "abc", "/page?k=abc&s=s&m=m&c=c1&i=1").status_code().0, 404);
        // a traversal attempt is just another (hashed, missing) folder name
        assert_eq!(handle(&dl, "abc", "/page?k=abc&s=..&m=..%2F..&c=..&i=0").status_code().0, 404);
        let _ = std::fs::remove_dir_all(root);
    }
}
