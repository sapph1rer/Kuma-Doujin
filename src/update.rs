//! Self-update from GitHub Releases (modelled on waku-app).
//!
//! The newest Release of [`REPO`] is checked; its Windows zip (kumadoujin.exe, kuma-browser.exe, …) is
//! downloaded, checked against the `.sha256` published beside it, unpacked, and copied over the program's
//! folder. Windows lets a running .exe be renamed but not overwritten, so each old file becomes
//! `<name>.old-<n>` first (removed at the next start); then the new exe is started and this one closes.
//! `scripts/make-release.ps1` builds the zip + checksum and publishes the Release.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const REPO: &str = "sapph1rer/Kuma-Doujin";
const EXE: &str = "kumadoujin.exe";

#[derive(Clone, Debug)]
pub struct Release {
    pub tag: String,
    pub version: (u32, u32, u32),
    pub notes: String,
    pub page: String,
    pub asset_url: Option<String>,
    pub asset_name: String,
    pub asset_size: u64,
    pub sha_url: Option<String>,
}

pub fn parse_version(s: &str) -> Option<(u32, u32, u32)> {
    let s = s.trim().trim_start_matches(['v', 'V']);
    let core = s.split(['-', '+']).next()?;
    let mut it = core.split('.').map(|x| x.parse::<u32>().ok());
    let a = it.next()??;
    let b = it.next().flatten().unwrap_or(0);
    let c = it.next().flatten().unwrap_or(0);
    Some((a, b, c))
}

pub fn is_newer(release: (u32, u32, u32)) -> bool {
    parse_version(VERSION).is_some_and(|me| release > me)
}

/// Run from cargo's target folder? Those copies are updated with git, not replaced.
pub fn dev_build() -> bool {
    std::env::current_exe()
        .map(|p| p.components().any(|c| c.as_os_str() == "target"))
        .unwrap_or(false)
}

pub fn parse_release(v: &serde_json::Value) -> Result<Release> {
    let tag = v["tag_name"].as_str().ok_or_else(|| anyhow!("no releases yet"))?.to_string();
    let version = parse_version(&tag).ok_or_else(|| anyhow!("release tag {tag} is not a version"))?;
    let assets: Vec<&serde_json::Value> = v["assets"].as_array().map(|a| a.iter().collect()).unwrap_or_default();
    let name_of = |a: &serde_json::Value| a["name"].as_str().unwrap_or("").to_string();
    let zip = assets
        .iter()
        .filter(|a| name_of(a).to_lowercase().ends_with(".zip"))
        .max_by_key(|a| {
            let n = name_of(a).to_lowercase();
            (n.contains("windows") || n.contains("win")) as u8 * 2 + (n.contains("x64") || n.contains("x86_64")) as u8
        });
    let (asset_url, asset_name, asset_size) = match zip {
        Some(a) => (
            a["browser_download_url"].as_str().map(String::from),
            name_of(a),
            a["size"].as_u64().unwrap_or(0),
        ),
        None => (None, String::new(), 0),
    };
    let sha_url = assets
        .iter()
        .find(|a| !asset_name.is_empty() && name_of(a).eq_ignore_ascii_case(&format!("{asset_name}.sha256")))
        .and_then(|a| a["browser_download_url"].as_str().map(String::from));
    Ok(Release {
        tag,
        version,
        notes: v["body"].as_str().unwrap_or("").trim().to_string(),
        page: v["html_url"].as_str().unwrap_or("").to_string(),
        asset_url,
        asset_name,
        asset_size,
        sha_url,
    })
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(300))
        .user_agent(concat!("KumaDoujin/", env!("CARGO_PKG_VERSION")))
        .build()
        .expect("http client")
}

pub async fn latest_release() -> Result<Release> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let v: serde_json::Value = client()
        .get(url)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .send()
        .await?
        .json()
        .await
        .context("GitHub")?;
    if let Some(m) = v["message"].as_str()
        && v.get("tag_name").is_none()
    {
        bail!("GitHub: {m}");
    }
    parse_release(&v)
}

fn sha256_hex(path: &Path) -> Result<String> {
    use sha2::Digest;
    let mut f = std::fs::File::open(path)?;
    let mut h = sha2::Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// Unpack `zip` into `dir`; a single top folder inside the zip is skipped. Returns the files (relative).
pub fn unpack(zip: &Path, dir: &Path) -> Result<Vec<PathBuf>> {
    let mut a = zip::ZipArchive::new(std::fs::File::open(zip)?).context("the download is not a zip")?;
    let names: Vec<PathBuf> = (0..a.len())
        .filter_map(|i| a.by_index(i).ok().and_then(|f| f.enclosed_name()))
        .collect();
    let top: Option<PathBuf> = {
        let firsts: std::collections::HashSet<_> = names
            .iter()
            .filter_map(|p| p.components().next().map(|c| c.as_os_str().to_owned()))
            .collect();
        (firsts.len() == 1 && names.iter().all(|p| p.components().count() > 1))
            .then(|| PathBuf::from(firsts.into_iter().next().expect("one")))
    };
    let _ = std::fs::remove_dir_all(dir);
    std::fs::create_dir_all(dir)?;
    let mut out = Vec::new();
    for i in 0..a.len() {
        let mut f = a.by_index(i)?;
        // enclosed_name rejects absolute paths and `..`, so nothing lands outside `dir`
        let Some(p) = f.enclosed_name() else { continue };
        let rel = match &top {
            Some(t) => p.strip_prefix(t).map(Path::to_path_buf).unwrap_or(p),
            None => p,
        };
        if rel.as_os_str().is_empty() {
            continue;
        }
        let dest = dir.join(&rel);
        if f.is_dir() {
            std::fs::create_dir_all(&dest)?;
            continue;
        }
        if let Some(d) = dest.parent() {
            std::fs::create_dir_all(d)?;
        }
        let mut w = std::fs::File::create(&dest)?;
        std::io::copy(&mut f, &mut w)?;
        out.push(rel);
    }
    Ok(out)
}

/// Copy the unpacked files over `target`, moving files in use aside (`.old-N`) first.
pub fn swap_in(src: &Path, files: &[PathBuf], target: &Path) -> Result<()> {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    for rel in files {
        let (from, to) = (src.join(rel), target.join(rel));
        if let Some(d) = to.parent() {
            std::fs::create_dir_all(d)?;
        }
        if to.exists() {
            let name = to.file_name().unwrap_or_default().to_string_lossy().to_string();
            let aside = to.with_file_name(format!("{name}.old-{stamp}"));
            std::fs::rename(&to, &aside).with_context(|| format!("move aside {}", to.display()))?;
        }
        std::fs::copy(&from, &to).with_context(|| format!("copy {}", to.display()))?;
    }
    Ok(())
}

/// Files left by an earlier update (`*.old-N`) are removed at start. Right after an update the
/// previous process may still be closing (its exe stays locked), so this retries in the background.
pub fn clean_old() {
    let Some(dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)) else {
        return;
    };
    let _ = std::thread::Builder::new().name("clean-old".into()).spawn(move || {
        for _ in 0..120 {
            if remove_old(&dir) == 0 {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
    });
}

/// Deletes `*.old-N` files in `dir`; returns how many are still there (in use).
fn remove_old(dir: &Path) -> usize {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().contains(".old-"))
        .filter(|e| std::fs::remove_file(e.path()).is_err())
        .count()
}

/// Download, verify and put the release in place. Returns the new program to start.
pub async fn install(r: &Release, work: &Path, progress: &Mutex<(u64, u64)>) -> Result<PathBuf> {
    let url = r.asset_url.clone().ok_or_else(|| anyhow!("the release {} has no Windows zip", r.tag))?;
    let sha_url = r
        .sha_url
        .clone()
        .ok_or_else(|| anyhow!("the release has no {}.sha256 — not installed (unverified)", r.asset_name))?;
    let exe = std::env::current_exe()?;
    let target = exe.parent().ok_or_else(|| anyhow!("program folder"))?.to_path_buf();
    std::fs::create_dir_all(work)?;
    let zip = work.join(&r.asset_name);
    let c = client();
    let mut resp = c.get(&url).send().await?.error_for_status()?;
    let total = resp.content_length().unwrap_or(r.asset_size);
    *progress.lock().expect("progress") = (0, total);
    let mut bytes = Vec::with_capacity(total as usize);
    while let Some(chunk) = resp.chunk().await? {
        bytes.extend_from_slice(&chunk);
        *progress.lock().expect("progress") = (bytes.len() as u64, total);
    }
    if total > 0 && bytes.len() as u64 != total {
        bail!("the download stopped early ({} of {total} bytes)", bytes.len());
    }
    std::fs::write(&zip, &bytes)?;
    let want = c.get(&sha_url).send().await?.error_for_status()?.text().await?;
    let want = want.split_whitespace().next().unwrap_or("").to_lowercase();
    if want.len() != 64 || want != sha256_hex(&zip)? {
        let _ = std::fs::remove_file(&zip);
        bail!("the download does not match its checksum — not installed");
    }
    let dir = work.join(&r.tag);
    let files = unpack(&zip, &dir)?;
    if !files.iter().any(|p| p.file_name().is_some_and(|n| n.eq_ignore_ascii_case(EXE))) {
        bail!("the zip has no {EXE}");
    }
    swap_in(&dir, &files, &target)?;
    let _ = std::fs::remove_file(&zip);
    Ok(target.join(EXE))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn versions() {
        assert_eq!(parse_version("v1.2.3"), Some((1, 2, 3)));
        assert_eq!(parse_version("0.4"), Some((0, 4, 0)));
        assert_eq!(parse_version("v2.0.0-beta.1"), Some((2, 0, 0)));
        assert_eq!(parse_version("latest"), None);
    }

    #[test]
    fn release_json() {
        let v: serde_json::Value = serde_json::from_str(
            r#"{"tag_name":"v0.2.0","body":"notes","html_url":"https://github.com/o/r/releases/tag/v0.2.0","assets":[
            {"name":"KumaDoujin-v0.2.0-windows-x64.zip","browser_download_url":"https://x/win.zip","size":1234},
            {"name":"KumaDoujin-v0.2.0-windows-x64.zip.sha256","browser_download_url":"https://x/win.zip.sha256","size":64}]}"#,
        )
        .unwrap();
        let r = parse_release(&v).unwrap();
        assert_eq!(r.version, (0, 2, 0));
        assert_eq!(r.sha_url.as_deref(), Some("https://x/win.zip.sha256"));
        assert!(parse_release(&serde_json::json!({"message": "Not Found"})).is_err());
    }

    /// zip with a top folder → unpacked flat → swapped over a folder whose files are "in use"
    #[test]
    fn unpack_and_swap() {
        let base = std::env::temp_dir().join(format!("kuma-upd-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("app")).unwrap();
        std::fs::write(base.join("app/kumadoujin.exe"), b"old").unwrap();
        let zp = base.join("u.zip");
        {
            let mut z = zip::ZipWriter::new(std::fs::File::create(&zp).unwrap());
            let o = zip::write::SimpleFileOptions::default();
            z.start_file("KumaDoujin/kumadoujin.exe", o).unwrap();
            z.write_all(b"new").unwrap();
            z.start_file("KumaDoujin/kuma-browser.exe", o).unwrap();
            z.write_all(b"helper").unwrap();
            z.finish().unwrap();
        }
        let files = unpack(&zp, &base.join("x")).unwrap();
        assert!(files.contains(&PathBuf::from("kumadoujin.exe")));
        swap_in(&base.join("x"), &files, &base.join("app")).unwrap();
        assert_eq!(std::fs::read(base.join("app/kumadoujin.exe")).unwrap(), b"new");
        assert_eq!(std::fs::read(base.join("app/kuma-browser.exe")).unwrap(), b"helper");
        let old: Vec<_> = std::fs::read_dir(base.join("app"))
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().contains(".old-"))
            .collect();
        assert_eq!(old.len(), 1);
        assert_eq!(remove_old(&base.join("app")), 0); // not in use any more: cleaned up
        assert!(base.join("app/kumadoujin.exe").exists());
        let _ = std::fs::remove_dir_all(&base);
    }
}
