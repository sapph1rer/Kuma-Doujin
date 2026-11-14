use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use reqwest::{StatusCode, Url, header};
use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::sync::Semaphore;

use super::browser::BrowserProcess;
use super::rate_limiter::RateLimiter;
use super::retry::{RetryPolicy, with_retry};
use crate::core::errors::{SourceError, SourceResult};

pub const USER_AGENT: &str = concat!(
    "KumaDoujin/",
    env!("CARGO_PKG_VERSION"),
    " (desktop manga reader)"
);
const MAX_JSON_BYTES: usize = 8 * 1024 * 1024;
const MAX_CONCURRENT: usize = 6;

/// HTTP client for one source: https-only, host allow-list (also on redirects),
/// per-host rate limits, bounded concurrency, retries and response size caps.
pub struct HttpClient {
    inner: reqwest::Client,
    allowed_hosts: Arc<[String]>,
    limiter: RateLimiter,
    permits: Semaphore,
    retry: RetryPolicy,
    browser: Option<Arc<BrowserProcess>>,
}

/// SSRF guard: only `https` URLs whose host equals an allowed host or is a subdomain of it.
pub fn is_allowed(url: &Url, allowed_hosts: &[String]) -> bool {
    url.scheme() == "https"
        && url.port().is_none_or(|p| p == 443)
        && url.host_str().is_some_and(|h| {
            allowed_hosts
                .iter()
                .any(|a| h == a || h.strip_suffix(a.as_str()).is_some_and(|p| p.ends_with('.')))
        })
}

impl HttpClient {
    pub fn new(allowed_hosts: &[&str], rate_limits: &[(&str, Duration)]) -> anyhow::Result<Self> {
        let allowed: Arc<[String]> = allowed_hosts.iter().map(|h| h.to_string()).collect();
        let redirect_hosts = allowed.clone();
        let inner = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::custom(move |attempt| {
                if attempt.previous().len() >= 3 || !is_allowed(attempt.url(), &redirect_hosts) {
                    attempt.stop()
                } else {
                    attempt.follow()
                }
            }))
            .build()?;
        Ok(Self {
            inner,
            allowed_hosts: allowed,
            limiter: RateLimiter::new(rate_limits),
            permits: Semaphore::new(MAX_CONCURRENT),
            retry: RetryPolicy::default(),
            browser: None,
        })
    }

    /// Fallback for hosts that answer plain HTTP with 403 (browser checks): pages are loaded in the
    /// helper browser, and images are fetched with the cookies it earned.
    pub fn with_browser(mut self, browser: Option<Arc<BrowserProcess>>) -> Self {
        self.browser = browser;
        self
    }

    /// Replaces the retry policy (e.g. `max_attempts: 1` when the caller retries itself).
    pub fn with_retry(mut self, retry: RetryPolicy) -> Self {
        self.retry = retry;
        self
    }

    pub async fn get_json<T: DeserializeOwned>(&self, url: &Url) -> SourceResult<T> {
        let (body, _) = self.get(url, MAX_JSON_BYTES, None, None, None).await?;
        serde_json::from_slice(&body).map_err(|e| SourceError::InvalidResponse(e.to_string()))
    }

    /// Fetches an HTML/text page, rejecting non-`text/*` responses and bodies over `max_bytes`.
    /// On 403 the helper browser (if any) loads it instead.
    pub async fn get_text(&self, url: &str, max_bytes: usize) -> SourceResult<String> {
        let url = Url::parse(url).map_err(|e| SourceError::InvalidResponse(e.to_string()))?;
        match self.get(&url, max_bytes, Some("text/"), None, None).await {
            Ok((body, _)) if !(self.browser.is_some() && is_challenge(&body)) => {
                Ok(String::from_utf8_lossy(&body).into_owned())
            }
            // some hosts answer non-browser clients with a fake 404 instead of 403
            Ok(_) | Err(SourceError::AccessDenied | SourceError::NotFound) if self.browser.is_some() => {
                self.check(&url)?;
                let browser = self.browser.clone().expect("checked");
                let u = url.to_string();
                let html = tokio::task::spawn_blocking(move || browser.fetch(&u))
                    .await
                    .map_err(|e| SourceError::Network(e.to_string()))?
                    .map_err(|e| SourceError::Network(format!("{e:#}")))?;
                if html.len() > max_bytes {
                    return Err(SourceError::InvalidResponse("response too large".into()));
                }
                Ok(html)
            }
            r => r.map(|(b, _)| String::from_utf8_lossy(&b).into_owned()),
        }
    }

    /// Fetches an image, rejecting non-`image/*` responses and bodies over `max_bytes`.
    pub async fn get_image(&self, url: &str, max_bytes: usize) -> SourceResult<Vec<u8>> {
        Ok(self.get_image_with_headers(url, max_bytes).await?.0)
    }

    /// Like [`Self::get_image`], also returning the response headers.
    pub async fn get_image_with_headers(
        &self,
        url: &str,
        max_bytes: usize,
    ) -> SourceResult<(Vec<u8>, header::HeaderMap)> {
        let url = Url::parse(url).map_err(|e| SourceError::InvalidResponse(e.to_string()))?;
        match self.get(&url, max_bytes, Some("image/"), None, None).await {
            Err(SourceError::AccessDenied) if self.browser.is_some() => {
                let browser = self.browser.clone().expect("checked");
                let u = url.to_string();
                let auth = tokio::task::spawn_blocking(move || browser.cookies(&u))
                    .await
                    .map_err(|e| SourceError::Network(e.to_string()))?
                    .map_err(|e| SourceError::Network(format!("{e:#}")))?;
                self.get(&url, max_bytes, Some("image/"), None, Some(&auth)).await
            }
            r => r,
        }
    }

    /// POSTs a JSON body once (no retries); used for fire-and-forget telemetry the source requires.
    pub async fn post_json<T: Serialize>(&self, url: &str, body: &T) -> SourceResult<()> {
        let url = Url::parse(url).map_err(|e| SourceError::InvalidResponse(e.to_string()))?;
        self.check(&url)?;
        let resp = self
            .inner
            .post(url)
            .json(body)
            .send()
            .await
            .map_err(net_err)?;
        match resp.status() {
            s if s.is_success() => Ok(()),
            s => Err(status_error(s, None)),
        }
    }

    fn check(&self, url: &Url) -> SourceResult<()> {
        if is_allowed(url, &self.allowed_hosts) {
            Ok(())
        } else {
            Err(SourceError::InvalidResponse(format!(
                "blocked URL outside source hosts: {url}"
            )))
        }
    }

    /// POSTs `body` as JSON and decodes a JSON response (e.g. GraphQL), with the same
    /// allow-list, rate limit, retries and size cap as GETs.
    pub async fn post_json_for<B: Serialize, T: DeserializeOwned>(
        &self,
        url: &Url,
        body: &B,
    ) -> SourceResult<T> {
        let body =
            serde_json::to_value(body).map_err(|e| SourceError::InvalidResponse(e.to_string()))?;
        let (bytes, _) = self.get(url, MAX_JSON_BYTES, None, Some(&body), None).await?;
        serde_json::from_slice(&bytes).map_err(|e| SourceError::InvalidResponse(e.to_string()))
    }

    /// GET, or POST when `json` is given.
    async fn get(
        &self,
        url: &Url,
        max_bytes: usize,
        mime: Option<&str>,
        json: Option<&serde_json::Value>,
        auth: Option<&(String, String)>,
    ) -> SourceResult<(Vec<u8>, header::HeaderMap)> {
        self.check(url)?;
        let host = url.host_str().unwrap_or_default();
        with_retry(self.retry, || async {
            let _permit = self
                .permits
                .acquire()
                .await
                .expect("semaphore never closed");
            self.limiter.acquire(host).await;
            self.fetch_once(url, max_bytes, mime, json, auth).await
        })
        .await
    }

    async fn fetch_once(
        &self,
        url: &Url,
        max_bytes: usize,
        mime: Option<&str>,
        json: Option<&serde_json::Value>,
        auth: Option<&(String, String)>,
    ) -> SourceResult<(Vec<u8>, header::HeaderMap)> {
        let mut req = match json {
            Some(body) => self.inner.post(url.clone()).json(body),
            None => self.inner.get(url.clone()),
        };
        if let Some((cookie, ua)) = auth {
            req = req
                .header(header::COOKIE, cookie)
                .header(header::USER_AGENT, ua)
                .header(header::REFERER, format!("{}/", url.origin().ascii_serialization()));
        }
        let mut resp = req.send().await.map_err(net_err)?;
        let status = resp.status();
        if !status.is_success() {
            return Err(status_error(status, retry_after(resp.headers())));
        }
        if let Some(prefix) = mime {
            let ct = resp
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default();
            if !ct.starts_with(prefix) {
                return Err(SourceError::InvalidResponse(format!(
                    "unexpected content type {ct:?}"
                )));
            }
        }
        let headers = resp.headers().clone();
        if resp
            .content_length()
            .is_some_and(|n| n as usize > max_bytes)
        {
            return Err(SourceError::InvalidResponse("response too large".into()));
        }
        let mut body = Vec::new();
        while let Some(chunk) = resp.chunk().await.map_err(net_err)? {
            if body.len() + chunk.len() > max_bytes {
                return Err(SourceError::InvalidResponse("response too large".into()));
            }
            body.extend_from_slice(&chunk);
        }
        Ok((body, headers))
    }
}

/// A browser-check interstitial served with a 200 status.
fn is_challenge(body: &[u8]) -> bool {
    let head = String::from_utf8_lossy(&body[..body.len().min(4096)]).to_lowercase();
    head.contains("<title>just a moment") || head.contains("<title>attention required")
}

fn net_err(e: reqwest::Error) -> SourceError {
    SourceError::Network(e.to_string())
}

pub fn status_error(status: StatusCode, retry_after: Option<Duration>) -> SourceError {
    match status.as_u16() {
        429 => SourceError::RateLimited { retry_after },
        401 | 403 | 451 => SourceError::AccessDenied,
        404 | 410 => SourceError::NotFound,
        500..=599 | 408 => SourceError::Network(format!("server returned {status}")),
        _ => SourceError::InvalidResponse(format!("unexpected status {status}")),
    }
}

/// Reads standard `Retry-After` (seconds) or MangaDex's `X-RateLimit-Retry-After` (unix time).
fn retry_after(h: &header::HeaderMap) -> Option<Duration> {
    let num = |name: &str| h.get(name)?.to_str().ok()?.trim().parse::<u64>().ok();
    if let Some(secs) = num("retry-after") {
        return Some(Duration::from_secs(secs));
    }
    let until = num("x-ratelimit-retry-after")?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
    Some(Duration::from_secs(until.saturating_sub(now)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn allowed(url: &str) -> bool {
        is_allowed(&Url::parse(url).unwrap(), &["mangadex.org".to_string()])
    }

    #[test]
    fn url_allow_list() {
        assert!(allowed("https://mangadex.org/x"));
        assert!(allowed("https://api.mangadex.org/manga"));
        assert!(!allowed("http://api.mangadex.org/manga"));
        assert!(!allowed("https://evilmangadex.org/"));
        assert!(!allowed("https://mangadex.org.evil.com/"));
        assert!(!allowed("https://127.0.0.1/"));
        assert!(!allowed("https://api.mangadex.org:8443/"));
    }

    #[test]
    fn status_mapping() {
        assert!(matches!(
            status_error(StatusCode::FORBIDDEN, None),
            SourceError::AccessDenied
        ));
        assert!(matches!(
            status_error(StatusCode::NOT_FOUND, None),
            SourceError::NotFound
        ));
        assert!(status_error(StatusCode::TOO_MANY_REQUESTS, None).is_transient());
        assert!(status_error(StatusCode::BAD_GATEWAY, None).is_transient());
        assert!(!status_error(StatusCode::BAD_REQUEST, None).is_transient());
    }

    #[test]
    fn retry_after_parsing() {
        let mut h = header::HeaderMap::new();
        h.insert("retry-after", "7".parse().unwrap());
        assert_eq!(retry_after(&h), Some(Duration::from_secs(7)));
    }
}
