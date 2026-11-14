//! kuma-browser: hidden WebView2 window driven over stdin/stdout (one JSON object per line).
//!
//! Requests:  {"id":1,"op":"fetch","url":"...","referer":"..."}  -> page HTML once scripts and any
//!            browser check have finished;  {"op":"cookies","url":"..."};  {"op":"show"|"hide"|"quit"|"ping"}
//! Replies:   {"id":1,"ok":true,"status":200,"url":"...","html":"..."} | {"id":1,"ok":false,"error":"..."}
//!
//! Some sources answer plain HTTP clients with 403 but serve a real browser; this helper is that browser.
//! The window stays hidden unless a check needs a human click, then it is shown until the check passes.
#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(not(windows))]
fn main() {
    eprintln!("kuma-browser needs Windows (WebView2)");
    std::process::exit(2);
}

#[cfg(windows)]
fn main() {
    if let Err(e) = win::run() {
        println!("{}", serde_json::json!({"id": 0, "ok": false, "fatal": true, "error": e.to_string()}));
        std::process::exit(1);
    }
}

#[cfg(windows)]
mod win {
    use serde_json::{Value, json};
    use std::io::{BufRead, Write};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};
    use tao::dpi::LogicalSize;
    use tao::event::{Event, WindowEvent};
    use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy};
    use tao::window::WindowBuilder;
    use wry::http::HeaderMap;
    use wry::{WebContext, WebViewBuilder};

    #[derive(Debug)]
    enum Ev {
        Cmd(Value),
        StdinClosed,
        Poll(u64, String),
        Html(u64, String),
        Ipc(String),
    }

    struct Job {
        id: Value,
        token: u64,
        url: String,
        started: Instant,
        deadline: Instant,
        shown: bool,
        tried_inpage: bool,
        html_pending: bool,
        /// CSS selector that must match before the page counts as loaded (client-rendered pages)
        wait: Option<String>,
        /// when the wait selector first matched; the page then gets a moment to finish rendering
        wait_met_at: Option<Instant>,
        /// page open when the job began; seeing it again means the navigation has not replaced it yet
        start_href: String,
    }

    fn reply(v: Value) {
        let mut out = std::io::stdout().lock();
        let _ = writeln!(out, "{v}");
        let _ = out.flush();
    }

    fn origin(u: &str) -> String {
        let (scheme, rest) = u.split_once("://").unwrap_or(("https", u));
        format!("{scheme}://{}", rest.split(['/', '?', '#']).next().unwrap_or(""))
    }

    fn strip(u: &str) -> String {
        u.split('#').next().unwrap_or(u).trim_end_matches('/').to_string()
    }

    /// Replies [readyState, title, href, challenged, waitedForSelectorPresent].
    fn poll_js(wait: Option<&str>) -> String {
        let sel = serde_json::to_string(wait.unwrap_or("")).unwrap_or_else(|_| "\"\"".into());
        POLL_JS.replace("__SEL__", &sel)
    }

    const POLL_JS: &str = r#"(function(){var t=document.title||'';var b=document.body?document.body.innerText.slice(0,400):'';
var ch=/just a moment|attention required|checking your browser|verify you are human|security check|sorry, you have been blocked/i.test(t+' '+b)
 || !!document.querySelector('#challenge-form,#cf-challenge-running,.cf-turnstile,#turnstile-wrapper,[name=cf-turnstile-response]');
var sel=__SEL__;var w=!sel||!!document.querySelector(sel);
return JSON.stringify([document.readyState,t,location.href,ch,w]);})()"#;

    pub fn run() -> Result<(), Box<dyn std::error::Error>> {
        let event_loop = EventLoopBuilder::<Ev>::with_user_event().build();
        let proxy: EventLoopProxy<Ev> = event_loop.create_proxy();

        {
            let p = proxy.clone();
            std::thread::spawn(move || {
                for line in std::io::stdin().lock().lines() {
                    let Ok(line) = line else { break };
                    if line.trim().is_empty() {
                        continue;
                    }
                    match serde_json::from_str::<Value>(&line) {
                        Ok(v) => {
                            let _ = p.send_event(Ev::Cmd(v));
                        }
                        Err(e) => reply(json!({"id": 0, "ok": false, "error": format!("bad request: {e}")})),
                    }
                }
                let _ = p.send_event(Ev::StdinClosed);
            });
        }

        let window = WindowBuilder::new()
            .with_title("KumaDoujin - browser check")
            .with_visible(false)
            .with_inner_size(LogicalSize::new(1000.0, 720.0))
            .build(&event_loop)?;

        let data_dir = std::env::var_os("KUMA_BROWSER_DATA").map(std::path::PathBuf::from).unwrap_or_else(|| {
            std::env::var_os("LOCALAPPDATA")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(std::env::temp_dir)
                .join("KumaDoujin")
                .join("browser")
        });
        let _ = std::fs::create_dir_all(&data_dir);
        let mut ctx = WebContext::new(Some(data_dir));

        // only the job's own site (and the challenge host) may navigate the top page
        let nav_host: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let ipc_proxy = proxy.clone();
        let nh = nav_host.clone();
        let webview = WebViewBuilder::new_with_web_context(&mut ctx)
            .with_ipc_handler(move |req| {
                let _ = ipc_proxy.send_event(Ev::Ipc(req.body().clone()));
            })
            .with_new_window_req_handler(|_url, _features| wry::NewWindowResponse::Deny)
            .with_navigation_handler(move |url: String| match nh.lock().ok().and_then(|g| g.clone()) {
                None => true,
                Some(o) => url.starts_with("about:") || origin(&url) == o || url.contains("challenges.cloudflare.com"),
            })
            .with_url("about:blank")
            .build(&window)?;

        let ua: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
        {
            let ua = ua.clone();
            let _ = webview.evaluate_script_with_callback("navigator.userAgent", move |s| {
                if let Ok(mut g) = ua.lock() {
                    *g = serde_json::from_str::<String>(&s).unwrap_or_default();
                }
            });
        }

        let mut job: Option<Job> = None;
        let mut queue: std::collections::VecDeque<Value> = Default::default();
        let mut token: u64 = 0;
        let mut current_origin = String::new();
        let mut current_href = String::new();
        let mut challenged_origin: Option<String> = None;
        reply(json!({"id": 0, "ok": true, "ready": true}));

        event_loop.run(move |event, _, control_flow| {
            let _keep_context_alive = &ctx;
            *control_flow = ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(400));
            let mut finish: Option<Value> = None;
            match event {
                Event::WindowEvent { event: WindowEvent::CloseRequested, .. } => {
                    window.set_visible(false);
                    if let Some(j) = &job {
                        finish = Some(json!({"id": j.id, "ok": false, "error": "browser window closed"}));
                    }
                }
                Event::UserEvent(Ev::StdinClosed) => *control_flow = ControlFlow::Exit,
                Event::UserEvent(Ev::Cmd(v)) => match v.get("op").and_then(|x| x.as_str()).unwrap_or("") {
                    "quit" => *control_flow = ControlFlow::Exit,
                    "show" => window.set_visible(true),
                    "hide" => window.set_visible(false),
                    "ping" => reply(json!({"id": v.get("id"), "ok": true})),
                    "cookies" => {
                        let u = v.get("url").and_then(|x| x.as_str()).unwrap_or("");
                        let c = webview
                            .cookies_for_url(u)
                            .map(|v| v.iter().map(|c| format!("{}={}", c.name(), c.value())).collect::<Vec<_>>().join("; "))
                            .unwrap_or_default();
                        let ua = ua.lock().map(|g| g.clone()).unwrap_or_default();
                        reply(json!({"id": v.get("id"), "ok": true, "cookies": c, "ua": ua}));
                    }
                    _ => queue.push_back(v),
                },
                Event::UserEvent(Ev::Ipc(body)) => {
                    if let (Some(j), Ok(m)) = (job.as_mut(), serde_json::from_str::<Value>(&body))
                        && m.get("k").and_then(|x| x.as_str()) == Some("fetch")
                        && m.get("t").and_then(|x| x.as_u64()) == Some(j.token)
                    {
                        let status = m.get("status").and_then(|x| x.as_u64()).unwrap_or(0);
                        let body = m.get("body").and_then(|x| x.as_str()).unwrap_or("");
                        let challenged = (status == 403 || status == 503) && body.contains("challenge");
                        if m.get("err").is_none() && status > 0 && !challenged {
                            finish = Some(json!({"id": j.id, "ok": true, "status": status, "url": j.url, "html": body}));
                        } else {
                            navigate(&webview, &nav_host, &j.url, None);
                        }
                    }
                }
                Event::UserEvent(Ev::Poll(t, s)) => {
                    if let Some(j) = job.as_mut().filter(|j| j.token == t) {
                        let v: Value = serde_json::from_str(&s)
                            .ok()
                            .and_then(|x: Value| x.as_str().and_then(|s| serde_json::from_str(s).ok()))
                            .unwrap_or(Value::Null);
                        let ready = v.get(0).and_then(|x| x.as_str()) == Some("complete");
                        let href = v.get(2).and_then(|x| x.as_str()).unwrap_or("").to_string();
                        let ch = v.get(3).and_then(|x| x.as_bool()).unwrap_or(false);
                        let matched = v.get(4).and_then(|x| x.as_bool()).unwrap_or(true);
                        if matched && j.wait.is_some() {
                            j.wait_met_at.get_or_insert_with(Instant::now);
                        }
                        let waited = matched
                            && (j.wait.is_none()
                                || j.wait_met_at.is_some_and(|t| t.elapsed() > Duration::from_millis(2500)));
                        if !href.is_empty() && href != "about:blank" {
                            current_origin = origin(&href);
                            current_href = href.clone();
                        }
                        let stale = strip(&href) == strip(&j.start_href)
                            && strip(&href) != strip(&j.url)
                            && j.started.elapsed() < Duration::from_secs(20);
                        if stale {
                            return;
                        }
                        if ch {
                            challenged_origin = Some(origin(&j.url));
                            // a check that does not pass on its own needs the user
                            if !j.shown && j.started.elapsed() > Duration::from_secs(8) {
                                j.shown = true;
                                window.set_visible(true);
                                window.set_focus();
                            }
                        } else if ready && waited && href != "about:blank" && !j.html_pending {
                            j.html_pending = true;
                            let p = proxy.clone();
                            let tk = j.token;
                            let _ = webview.evaluate_script_with_callback(
                                "JSON.stringify(['__wkhtml__', location.href, document.documentElement.outerHTML])",
                                move |h| {
                                    let _ = p.send_event(Ev::Html(tk, h));
                                },
                            );
                        }
                    }
                }
                Event::UserEvent(Ev::Html(t, h)) => {
                    if let Some(j) = job.as_mut().filter(|j| j.token == t) {
                        let inner: String = serde_json::from_str(&h).unwrap_or_default();
                        let arr: Value = serde_json::from_str(&inner).unwrap_or(Value::Null);
                        let ok = arr.get(0).and_then(|x| x.as_str()) == Some("__wkhtml__");
                        let html = arr.get(2).and_then(|x| x.as_str()).unwrap_or("").to_string();
                        let got = arr.get(1).and_then(|x| x.as_str()).unwrap_or("");
                        let old_page = strip(got) == strip(&j.start_href)
                            && strip(got) != strip(&j.url)
                            && j.started.elapsed() < Duration::from_secs(20);
                        if !ok || html.len() < 64 || old_page {
                            j.html_pending = false;
                            return;
                        }
                        challenged_origin = challenged_origin.take().filter(|o| *o != origin(&j.url));
                        finish = Some(json!({"id": j.id, "ok": true, "status": 200, "url": j.url, "html": html}));
                    }
                }
                _ => {}
            }

            if job.is_none()
                && finish.is_none()
                && let Some(v) = queue.pop_front()
            {
                token += 1;
                let url = v.get("url").and_then(|x| x.as_str()).unwrap_or("").to_string();
                let referer = v.get("referer").and_then(|x| x.as_str()).map(str::to_string);
                let secs = v.get("timeout").and_then(|x| x.as_u64()).unwrap_or(60);
                let mut j = Job {
                    id: v.get("id").cloned().unwrap_or(Value::Null),
                    token,
                    url: url.clone(),
                    started: Instant::now(),
                    deadline: Instant::now() + Duration::from_secs(secs),
                    shown: false,
                    tried_inpage: false,
                    html_pending: false,
                    wait_met_at: None,
                    wait: v.get("wait").and_then(|x| x.as_str()).map(str::to_string),
                    start_href: current_href.clone(),
                };
                // same site already open and past its check: fetch from inside the page (fast, keeps cookies)
                let same = j.wait.is_none() && origin(&url) == current_origin && challenged_origin.as_deref() != Some(current_origin.as_str());
                if same {
                    j.tried_inpage = true;
                    let js = format!(
                        "fetch({u},{{credentials:'include'}}).then(function(r){{return r.text().then(function(t){{window.ipc.postMessage(JSON.stringify({{k:'fetch',t:{tk},status:r.status,body:t}}))}})}}).catch(function(e){{window.ipc.postMessage(JSON.stringify({{k:'fetch',t:{tk},err:String(e)}}))}})",
                        u = serde_json::to_string(&url).unwrap_or_default(),
                        tk = token
                    );
                    let _ = webview.evaluate_script(&js);
                } else {
                    navigate(&webview, &nav_host, &url, referer.as_deref());
                }
                job = Some(j);
            }

            if finish.is_none()
                && let Some(j) = job.as_mut()
            {
                let now = Instant::now();
                if now > j.deadline {
                    finish = Some(json!({"id": j.id, "ok": false, "error": "timed out waiting for the page (browser check not passed?)"}));
                } else if !j.html_pending && !(j.tried_inpage && now.duration_since(j.started) < Duration::from_secs(4)) {
                    let p = proxy.clone();
                    let tk = j.token;
                    let _ = webview.evaluate_script_with_callback(&poll_js(j.wait.as_deref()), move |s| {
                        let _ = p.send_event(Ev::Poll(tk, s));
                    });
                }
            }

            if let Some(r) = finish {
                if job.as_ref().is_some_and(|j| j.shown) {
                    window.set_visible(false);
                }
                reply(r);
                job = None;
                if let Ok(mut g) = nav_host.lock() {
                    *g = None;
                }
                *control_flow = ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(10));
            }
        });
    }

    fn navigate(webview: &wry::WebView, nav_host: &Arc<Mutex<Option<String>>>, url: &str, referer: Option<&str>) {
        if let Ok(mut g) = nav_host.lock() {
            *g = Some(origin(url));
        }
        let mut h = HeaderMap::new();
        if let Some(r) = referer
            && let Ok(v) = r.parse()
        {
            h.insert("referer", v);
        }
        let _ = webview.load_url_with_headers(url, h);
    }
}
