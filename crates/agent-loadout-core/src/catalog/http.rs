//! HTTP for catalogs (#26): `GET` only, behind [`Http`] so tests use a
//! local stub or recorded fixtures. [`UreqHttp`] is `ureq` with rustls and
//! the platform's certificate verifier (no OpenSSL, SPEC 4), gzip, a
//! connect and an overall timeout, a body cap, and proxies from the
//! injected [`Env`] (never the process environment): `HTTPS_PROXY` (then
//! `ALL_PROXY`) for `https://`, `HTTP_PROXY` (then `ALL_PROXY`) for a
//! loopback `http://` registry. A SOCKS proxy or one that does not parse
//! is an error, never a direct connection. Every hop of an `https://`
//! request is `https://`: a redirect to plain http is refused before it
//! is sent; a loopback `http://` registry is not redirected at all.

use std::time::Duration;

use ureq::tls::{RootCerts, TlsConfig};

use crate::env::Env;
use crate::error::{CoreError, ErrorCode};

/// A response: the status and the body (gzip already decoded).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    pub body: Vec<u8>,
}

/// The one request catalogs make. Errors are `catalog_unreachable`.
pub trait Http: Send + Sync {
    fn get(&self, url: &str) -> Result<HttpResponse, CoreError>;
}

/// Per-request limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HttpLimits {
    pub connect: Duration,
    /// The whole request, body included.
    pub total: Duration,
    pub max_bytes: u64,
}

impl Default for HttpLimits {
    fn default() -> Self {
        Self {
            connect: Duration::from_secs(10),
            total: Duration::from_secs(20),
            max_bytes: 8 * 1024 * 1024,
        }
    }
}

/// [`Http`] over `ureq`.
pub struct UreqHttp {
    /// `https://` requests: https on every hop.
    https: ureq::Agent,
    /// Loopback `http://` registries: no redirects.
    loopback: ureq::Agent,
    /// Why the proxy settings cannot be used, per scheme: every request of
    /// that scheme fails with it (never a direct connection).
    https_problem: Option<String>,
    loopback_problem: Option<String>,
    max_bytes: u64,
}

impl std::fmt::Debug for UreqHttp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UreqHttp").finish_non_exhaustive()
    }
}

/// The first of `names` set (non-empty) in `env`, with its name.
fn var(env: &Env, names: &[&'static str]) -> Option<(&'static str, String)> {
    names.iter().find_map(|name| {
        env.var(name)
            .map(|value| value.to_string_lossy().trim().to_owned())
            .filter(|value| !value.is_empty())
            .map(|value| (*name, value))
    })
}

/// The proxy the first set of `names` gives, with the `NO_PROXY` hosts;
/// `Err` (the reason) for a SOCKS proxy (ureq is built without SOCKS) or
/// a value that does not parse.
fn proxy(env: &Env, names: &[&'static str]) -> Result<Option<ureq::Proxy>, String> {
    let Some((name, url)) = var(env, names) else {
        return Ok(None);
    };
    let unusable = || format!("the proxy in {name} is not a usable proxy address");
    let parsed = ureq::Proxy::new(&url).map_err(|_| unusable())?;
    if !matches!(
        parsed.protocol(),
        ureq::ProxyProtocol::Http | ureq::ProxyProtocol::Https
    ) {
        return Err(format!(
            "the proxy in {name} is a SOCKS proxy, which Loadout does not support; set it to an \
             HTTP proxy (http://host:port)"
        ));
    }
    let mut builder = ureq::Proxy::builder(parsed.protocol())
        .host(parsed.host())
        .port(parsed.port());
    if let Some(user) = parsed.username() {
        builder = builder.username(user);
    }
    if let Some(password) = parsed.password() {
        builder = builder.password(password);
    }
    if let Some((_, list)) = var(env, &["NO_PROXY", "no_proxy"]) {
        for expr in list.split(',').map(str::trim).filter(|e| !e.is_empty()) {
            builder = builder.no_proxy(expr);
        }
    }
    builder.build().map(Some).map_err(|_| unusable())
}

impl UreqHttp {
    pub fn new(env: &Env, limits: HttpLimits) -> Self {
        let agent = |proxy: Option<ureq::Proxy>, https_only: bool, redirects: u32| {
            ureq::Agent::config_builder()
                .http_status_as_error(false)
                .timeout_connect(Some(limits.connect))
                .timeout_global(Some(limits.total))
                .https_only(https_only)
                .max_redirects(redirects)
                .user_agent(format!("Loadout/{}", env!("CARGO_PKG_VERSION")))
                .proxy(proxy)
                .tls_config(
                    TlsConfig::builder()
                        .root_certs(RootCerts::PlatformVerifier)
                        .build(),
                )
                .build()
                .new_agent()
        };
        let https = proxy(
            env,
            &["HTTPS_PROXY", "https_proxy", "ALL_PROXY", "all_proxy"],
        );
        let loopback = proxy(env, &["HTTP_PROXY", "http_proxy", "ALL_PROXY", "all_proxy"]);
        let (https_proxy, https_problem) = match https {
            Ok(proxy) => (proxy, None),
            Err(why) => (None, Some(why)),
        };
        let (loopback_proxy, loopback_problem) = match loopback {
            Ok(proxy) => (proxy, None),
            Err(why) => (None, Some(why)),
        };
        Self {
            https: agent(https_proxy, true, 3),
            loopback: agent(loopback_proxy, false, 0),
            https_problem,
            loopback_problem,
            max_bytes: limits.max_bytes,
        }
    }

    /// The agent for `url`: the https-only one unless it is `http://`
    /// (registry URLs allow that for loopback hosts only).
    fn agent_for(&self, url: &str) -> &ureq::Agent {
        if url.starts_with("http://") {
            &self.loopback
        } else {
            &self.https
        }
    }

    fn problem_for(&self, url: &str) -> Option<&str> {
        if url.starts_with("http://") {
            self.loopback_problem.as_deref()
        } else {
            self.https_problem.as_deref()
        }
    }
}

/// `catalog_unreachable` about `url`.
pub(crate) fn unreachable(url: &str, why: impl std::fmt::Display) -> CoreError {
    CoreError::new(
        ErrorCode::CatalogUnreachable,
        format!("{} could not be reached: {why}", host_of(url)),
    )
}

/// The host (with a port) of `url`, for messages.
pub(crate) fn host_of(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|parsed| {
            let host = parsed.host_str()?.to_owned();
            Some(match parsed.port() {
                Some(port) => format!("{host}:{port}"),
                None => host,
            })
        })
        .unwrap_or_else(|| "the catalog".to_owned())
}

impl Http for UreqHttp {
    fn get(&self, url: &str) -> Result<HttpResponse, CoreError> {
        if let Some(why) = self.problem_for(url) {
            return Err(unreachable(url, why));
        }
        let response = self
            .agent_for(url)
            .get(url)
            .header("Accept", "application/json")
            .call()
            .map_err(|err| match err {
                ureq::Error::Timeout(_) => unreachable(url, "it did not answer in time"),
                ureq::Error::HostNotFound => unreachable(url, "the host was not found"),
                ureq::Error::ConnectionFailed => unreachable(url, "the connection failed"),
                ureq::Error::TooManyRedirects => unreachable(url, "too many redirects"),
                ureq::Error::RequireHttpsOnly(_) => {
                    unreachable(url, "it redirected to an address without encryption")
                }
                other => unreachable(url, other),
            })?;
        let status = response.status().as_u16();
        let body = response
            .into_body()
            .with_config()
            .limit(self.max_bytes)
            .read_to_vec()
            .map_err(|err| match err {
                ureq::Error::BodyExceedsLimit(_) => unreachable(url, "the answer is too large"),
                ureq::Error::Timeout(_) => unreachable(url, "it did not answer in time"),
                other => unreachable(url, other),
            })?;
        Ok(HttpResponse { status, body })
    }
}

#[cfg(test)]
mod tests {
    //! The transport itself against local stubs: proxies, redirects, the
    //! body cap after decompression and the overall timeout. No network.

    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use super::*;
    use crate::env::Os;

    /// A one-route HTTP stub: every request gets `answer(first line)`, written
    /// in `chunks` with `pause` between them; requests are counted.
    struct Stub {
        url: String,
        hits: Arc<AtomicUsize>,
        lines: Arc<Mutex<Vec<String>>>,
    }

    impl Stub {
        fn start(
            answer: impl Fn(&str) -> Vec<Vec<u8>> + Send + Sync + 'static,
            pause: Duration,
        ) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}", listener.local_addr().unwrap());
            let hits = Arc::new(AtomicUsize::new(0));
            let lines = Arc::new(Mutex::new(Vec::new()));
            let answer = Arc::new(answer);
            let (thread_hits, thread_lines) = (hits.clone(), lines.clone());
            std::thread::spawn(move || {
                for stream in listener.incoming() {
                    let Ok(mut stream) = stream else { continue };
                    let (hits, lines, answer) =
                        (thread_hits.clone(), thread_lines.clone(), answer.clone());
                    std::thread::spawn(move || {
                        let mut request = Vec::new();
                        let mut buf = [0u8; 1024];
                        while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                            match stream.read(&mut buf) {
                                Ok(0) | Err(_) => return,
                                Ok(n) => request.extend_from_slice(&buf[..n]),
                            }
                        }
                        let text = String::from_utf8_lossy(&request);
                        let line = text.lines().next().unwrap_or_default().to_owned();
                        hits.fetch_add(1, Ordering::SeqCst);
                        lines.lock().unwrap().push(line.clone());
                        for chunk in answer(&line) {
                            if stream.write_all(&chunk).is_err() {
                                return;
                            }
                            let _ = stream.flush();
                            std::thread::sleep(pause);
                        }
                    });
                }
            });
            Self { url, hits, lines }
        }

        fn json(body: &'static str) -> Self {
            Self::start(
                move |_| {
                    vec![format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    )
                    .into_bytes()]
                },
                Duration::ZERO,
            )
        }

        fn hits(&self) -> usize {
            self.hits.load(Ordering::SeqCst)
        }

        fn host_port(&self) -> &str {
            self.url.trim_start_matches("http://")
        }
    }

    fn env(vars: &[(&str, &str)]) -> Env {
        let mut builder = Env::builder(Os::Linux, "/home/me");
        for (name, value) in vars {
            builder = builder.var(name, *value);
        }
        builder.build()
    }

    fn limits() -> HttpLimits {
        HttpLimits {
            connect: Duration::from_secs(2),
            total: Duration::from_secs(2),
            max_bytes: 1024,
        }
    }

    /// The request, run so a panic is a failure of its own.
    fn get(http: &UreqHttp, url: &str) -> Result<HttpResponse, CoreError> {
        let url = url.to_owned();
        std::thread::scope(|scope| {
            scope
                .spawn(|| http.get(&url))
                .join()
                .unwrap_or_else(|_| panic!("the request panicked"))
        })
    }

    #[test]
    fn a_socks_proxy_is_refused_without_connecting_or_going_direct() {
        let target = Stub::json("{}");
        let proxy = Stub::json("{}");
        let loopback = format!("{}/v0.1/servers", target.url);
        for (name, url) in [
            ("ALL_PROXY", loopback.as_str()),
            ("ALL_PROXY", "https://registry.example.invalid/v0.1/servers"),
            (
                "HTTPS_PROXY",
                "https://registry.example.invalid/v0.1/servers",
            ),
            ("HTTP_PROXY", loopback.as_str()),
        ] {
            let value = format!("socks5://{}", proxy.host_port());
            let http = UreqHttp::new(&env(&[(name, value.as_str())]), limits());
            let err = get(&http, url).unwrap_err();
            assert_eq!(err.code(), ErrorCode::CatalogUnreachable, "{name}");
            assert!(err.message().contains("SOCKS"), "{name}: {err}");
        }
        assert_eq!(target.hits(), 0, "never direct");
        assert_eq!(proxy.hits(), 0, "never through the SOCKS proxy");
    }

    #[test]
    fn a_proxy_that_does_not_parse_is_an_error_not_a_direct_connection() {
        let target = Stub::json("{}");
        let http = UreqHttp::new(&env(&[("HTTP_PROXY", "http://[not a host")]), limits());
        let err = get(&http, &format!("{}/v0.1/servers", target.url)).unwrap_err();
        assert_eq!(err.code(), ErrorCode::CatalogUnreachable);
        assert!(err.message().contains("proxy"), "{err}");
        assert_eq!(target.hits(), 0);
    }

    #[test]
    fn https_proxy_wins_over_all_proxy_for_https() {
        // The HTTPS proxy is a stub: it sees the CONNECT and answers 502.
        let proxy = Stub::start(
            |_| vec![b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\n\r\n".to_vec()],
            Duration::ZERO,
        );
        let https = format!("http://{}", proxy.host_port());
        let http = UreqHttp::new(
            &env(&[
                ("ALL_PROXY", "socks5://127.0.0.1:9"),
                ("https_proxy", https.as_str()),
            ]),
            limits(),
        );
        let err = get(&http, "https://registry.example.invalid/v0.1/servers").unwrap_err();
        assert_eq!(err.code(), ErrorCode::CatalogUnreachable);
        assert_eq!(proxy.hits(), 1, "{err}");
        assert!(
            proxy.lines.lock().unwrap()[0].starts_with("CONNECT registry.example.invalid:443"),
            "{:?}",
            proxy.lines.lock().unwrap()
        );
    }

    #[test]
    fn no_proxy_hosts_go_direct() {
        let target = Stub::json("{}");
        let http = UreqHttp::new(
            &env(&[
                ("HTTP_PROXY", "http://127.0.0.1:9"),
                ("NO_PROXY", "127.0.0.1"),
            ]),
            limits(),
        );
        assert_eq!(
            get(&http, &format!("{}/x", target.url)).unwrap().status,
            200
        );
        assert_eq!(target.hits(), 1);
    }

    #[test]
    fn an_https_request_never_sends_a_plain_http_hop() {
        // What a redirect from https to http would ask for: refused before
        // anything is sent, on every hop.
        let target = Stub::json("{}");
        let http = UreqHttp::new(&env(&[]), limits());
        let err = http
            .agent_for("https://registry.example")
            .get(&format!("{}/latest/meta-data", target.url))
            .call()
            .unwrap_err();
        assert!(matches!(err, ureq::Error::RequireHttpsOnly(_)), "{err}");
        assert_eq!(target.hits(), 0);
    }

    #[test]
    fn a_loopback_registry_does_not_follow_redirects() {
        let elsewhere = Stub::json("{}");
        let location = format!(
            "{}/v0.1/servers",
            elsewhere.url.replace("127.0.0.1", "localhost")
        );
        let registry = Stub::start(
            move |_| {
                vec![format!(
                    "HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                )
                .into_bytes()]
            },
            Duration::ZERO,
        );
        let http = UreqHttp::new(&env(&[]), limits());
        let result = get(&http, &format!("{}/v0.1/servers", registry.url));
        assert!(
            result.as_ref().is_err() || result.as_ref().unwrap().status == 302,
            "{result:?}"
        );
        assert_eq!(elsewhere.hits(), 0);
    }

    #[test]
    fn a_gzip_body_over_the_cap_is_refused_after_decompression() {
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
        gz.write_all(&vec![b' '; 64 * 1024]).unwrap();
        let body = gz.finish().unwrap();
        assert!(body.len() < 1024, "the compressed body is under the cap");
        let stub = Stub::start(
            move |_| {
                let mut answer = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Encoding: gzip\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .into_bytes();
                answer.extend_from_slice(&body);
                vec![answer]
            },
            Duration::ZERO,
        );
        let http = UreqHttp::new(&env(&[]), limits());
        let err = get(&http, &format!("{}/x", stub.url)).unwrap_err();
        assert!(err.message().contains("too large"), "{err}");
    }

    #[test]
    fn a_slow_body_stops_at_the_total_timeout() {
        let stub = Stub::start(
            |_| {
                let mut chunks = vec![
                    b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 100\r\n\r\n"
                        .to_vec(),
                ];
                chunks.extend((0..100).map(|_| b" ".to_vec()));
                chunks
            },
            Duration::from_millis(100),
        );
        let http = UreqHttp::new(&env(&[]), limits());
        let started = Instant::now();
        let err = get(&http, &format!("{}/x", stub.url)).unwrap_err();
        assert_eq!(err.code(), ErrorCode::CatalogUnreachable);
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );
    }
}
