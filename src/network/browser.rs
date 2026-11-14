//! Client for the `kuma-browser` helper (hidden WebView2 window, see src/bin/kuma-browser.rs).
//!
//! Started on first use and kept running; requests are serialised. Used only as a fallback for sources
//! whose host answers plain HTTP clients with 403 but serves a real browser.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::Mutex;
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};

struct Proc {
    child: Child,
    stdin: ChildStdin,
    rx: Receiver<Value>,
    next: u64,
}

pub struct BrowserProcess {
    exe: PathBuf,
    p: Mutex<Option<Proc>>,
    /// set when the helper could not start (no WebView2 runtime), so we stop retrying
    broken: Mutex<Option<String>>,
}

impl BrowserProcess {
    /// The helper next to the running executable, if present (Windows only).
    pub fn beside_current_exe() -> Option<Self> {
        let exe = std::env::var_os("KUMA_BROWSER_EXE")
            .map(PathBuf::from)
            .or_else(|| Some(std::env::current_exe().ok()?.parent()?.join("kuma-browser.exe")))?;
        (cfg!(windows) && exe.exists()).then(|| Self {
            exe,
            p: Mutex::new(None),
            broken: Mutex::new(None),
        })
    }

    fn start(&self) -> Result<Proc> {
        let mut cmd = Command::new(&self.exe);
        cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        let mut child = cmd.spawn().with_context(|| format!("starting {}", self.exe.display()))?;
        let stdin = child.stdin.take().ok_or_else(|| anyhow!("no stdin"))?;
        let stdout = child.stdout.take().ok_or_else(|| anyhow!("no stdout"))?;
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if let Ok(v) = serde_json::from_str::<Value>(&line)
                    && tx.send(v).is_err()
                {
                    break;
                }
            }
        });
        match rx.recv_timeout(Duration::from_secs(20)) {
            Ok(v) if v.get("ready").and_then(Value::as_bool) == Some(true) => {}
            Ok(v) => bail!("browser helper failed: {}", v.get("error").and_then(Value::as_str).unwrap_or("?")),
            Err(_) => {
                let _ = child.kill();
                bail!("browser helper did not start (is the WebView2 runtime installed?)")
            }
        }
        Ok(Proc { child, stdin, rx, next: 1 })
    }

    fn call(&self, mut req: Value, timeout: Duration) -> Result<Value> {
        if let Some(e) = self.broken.lock().ok().and_then(|g| g.clone()) {
            bail!("browser helper unavailable: {e}");
        }
        let mut g = self.p.lock().map_err(|_| anyhow!("browser lock poisoned"))?;
        let alive = g.as_mut().is_some_and(|p| p.child.try_wait().ok().flatten().is_none());
        if !alive {
            match self.start() {
                Ok(p) => *g = Some(p),
                Err(e) => {
                    if let Ok(mut b) = self.broken.lock() {
                        *b = Some(format!("{e:#}"));
                    }
                    return Err(e);
                }
            }
        }
        let p = g.as_mut().expect("started above");
        let id = p.next;
        p.next += 1;
        req["id"] = json!(id);
        writeln!(p.stdin, "{req}").context("writing to browser helper")?;
        p.stdin.flush()?;
        loop {
            match p.rx.recv_timeout(timeout) {
                Ok(v) if v.get("id").and_then(Value::as_u64) == Some(id) => {
                    if v.get("ok").and_then(Value::as_bool) == Some(true) {
                        return Ok(v);
                    }
                    bail!("{}", v.get("error").and_then(Value::as_str).unwrap_or("browser error"));
                }
                Ok(_) => continue, // late answer to an earlier, timed-out request
                Err(_) => {
                    let _ = p.child.kill();
                    *g = None;
                    bail!("browser helper timed out");
                }
            }
        }
    }

    /// Page HTML after scripts and any browser check have run. Blocking.
    pub fn fetch(&self, url: &str, wait: Option<&str>) -> Result<String> {
        // a selector that never shows up (e.g. an empty result list) should not hold the reader for a minute
        let timeout = if wait.is_some() { 20 } else { 60 };
        let v = self.call(
            json!({"op": "fetch", "url": url, "wait": wait, "timeout": timeout}),
            Duration::from_secs(timeout + 35),
        )?;
        let status = v.get("status").and_then(Value::as_u64).unwrap_or(200);
        if status >= 400 {
            bail!("GET {url} (browser) -> HTTP {status}");
        }
        Ok(v.get("html").and_then(Value::as_str).unwrap_or_default().to_string())
    }

    /// (Cookie header value, User-Agent) the browser holds for `url`. Blocking.
    pub fn cookies(&self, url: &str) -> Result<(String, String)> {
        let v = self.call(json!({"op": "cookies", "url": url}), Duration::from_secs(10))?;
        let get = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
        Ok((get("cookies"), get("ua")))
    }
}

impl Drop for BrowserProcess {
    fn drop(&mut self) {
        if let Ok(mut g) = self.p.lock()
            && let Some(p) = g.as_mut()
        {
            let _ = writeln!(p.stdin, "{}", json!({"op": "quit"}));
        }
    }
}
