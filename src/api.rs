use reqwest::{header, Client, Method, Response, Url};
use serde_json::{json, Value};
use std::{
    fmt,
    io::Write,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};

const API_VERSION: &str = "2022-11-28";
const MAX_JSON: usize = 8 * 1024 * 1024;
const MAX_ZIP: usize = 128 * 1024 * 1024;

pub fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key].as_str().unwrap_or_default()
}
pub fn number(value: &Value, key: &str) -> Option<u64> {
    value[key]
        .as_u64()
        .or_else(|| value[key].as_str()?.parse().ok())
        .filter(|v| *v > 0)
}

#[derive(Debug)]
pub enum Error {
    Message(&'static str),
    Network,
    Reply,
    Http(u16),
    Limit(Option<u64>),
    Graph,
}

impl Error {
    pub fn status(&self, code: u16) -> bool {
        matches!(self, Self::Http(status) if *status==code)
    }
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Message(message)=>f.write_str(message),
            Self::Network=>f.write_str("GitHub did not return a response. If this was a write, check GitHub before trying again."),
            Self::Reply=>f.write_str("GitHub returned an unreadable or oversized response. Check GitHub before repeating any write."),
            Self::Http(401)=>f.write_str("GitHub rejected the Personal Access Token. Update it in extension preferences."),
            Self::Http(403 | 404)=>f.write_str("GitHub could not grant access to this item. Check the token permissions, repository access and organization authorization."),
            Self::Http(409 | 422)=>f.write_str("GitHub could not apply the operation. Check the selected item, current state and required values."),
            Self::Http(500..=599)=>f.write_str("GitHub reports a service problem. Check GitHub before repeating any write."),
            Self::Http(status)=>write!(f,"GitHub returned HTTP {status}."),
            Self::Limit(Some(delay))=>write!(f,"GitHub is rate limiting this request. Try again in {delay} seconds."),
            Self::Limit(None)=>f.write_str("GitHub is rate limiting this request. Try again later."),
            Self::Graph=>f.write_str("GitHub reported a GraphQL error. Check the selected item and token permissions before trying again."),
        }
    }
}
impl From<Error> for String {
    fn from(error: Error) -> String {
        error.to_string()
    }
}

#[derive(Clone)]
pub struct Transport {
    http: Client,
    origin: Url,
}

impl Transport {
    pub fn new() -> Result<Self, String> {
        #[cfg(not(feature = "fixture"))]
        let origin = Url::parse("https://api.github.com/").expect("fixed GitHub origin");
        #[cfg(feature = "fixture")]
        let origin = {
            let raw = std::env::var("TEZBAR_GITHUB_FIXTURE_ORIGIN")
                .map_err(|_| "Fixture builds require a loopback origin.")?;
            let url = Url::parse(&raw).map_err(|_| "Invalid fixture origin.")?;
            if url.scheme() != "http"
                || !matches!(url.host_str(), Some("127.0.0.1" | "[::1]"))
                || url.port().is_none()
                || !url.username().is_empty()
                || url.password().is_some()
                || url.path() != "/"
                || url.query().is_some()
                || url.fragment().is_some()
            {
                return Err(
                    "Fixture origin must be HTTP on a literal loopback address and explicit port."
                        .into(),
                );
            }
            url
        };
        let mut builder = Client::builder()
            .user_agent("Tezbar GitHub extension")
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(15))
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never());
        if cfg!(feature = "fixture") {
            builder = builder.no_proxy();
        }
        let http = builder
            .build()
            .map_err(|_| "Could not initialize GitHub HTTPS.")?;
        Ok(Self { http, origin })
    }
    pub fn auth(&self, raw: &str) -> Result<Api, Error> {
        let token = raw.trim();
        #[cfg(feature = "fixture")]
        if !matches!(token, "gh_fixture_only" | "gh_fixture_only_rotated") {
            return Err(Error::Message(
                "Fixture builds accept only the synthetic GitHub token.",
            ));
        }
        if token.is_empty() || token.len() > 1024 || token.bytes().any(|b| b.is_ascii_whitespace())
        {
            return Err(Error::Message(
                "Set a valid Personal Access Token in extension preferences.",
            ));
        }
        let mut secret =
            header::HeaderValue::from_str(&format!("Bearer {token}")).map_err(|_| {
                Error::Message("The Personal Access Token contains invalid characters.")
            })?;
        secret.set_sensitive(true);
        Ok(Api {
            transport: self.clone(),
            secret,
            poll_interval: Arc::new(AtomicU64::new(0)),
        })
    }
}

#[derive(Clone)]
pub struct Api {
    transport: Transport,
    secret: header::HeaderValue,
    poll_interval: Arc<AtomicU64>,
}

fn headers(response: &Response, key: &str) -> Option<String> {
    response
        .headers()
        .get(key)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
}
fn status(response: &Response) -> Error {
    let code = response.status().as_u16();
    let delay = headers(response, "retry-after").and_then(|value| value.parse().ok());
    if code == 429
        || code == 403
            && (delay.is_some()
                || headers(response, "x-ratelimit-remaining").as_deref() == Some("0"))
    {
        let delay = delay.or_else(|| {
            headers(response, "x-ratelimit-reset")
                .and_then(|value| value.parse::<i64>().ok())
                .map(|reset| (reset - chrono::Utc::now().timestamp()).max(0) as u64)
        });
        return Error::Limit(delay);
    }
    Error::Http(code)
}
async fn bounded(mut response: Response, limit: usize) -> Result<Vec<u8>, Error> {
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        return Err(Error::Reply);
    }
    let mut out = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| Error::Network)? {
        if out.len() + chunk.len() > limit {
            return Err(Error::Reply);
        }
        out.extend_from_slice(&chunk);
    }
    Ok(out)
}
fn link_next(value: &str) -> bool {
    value.split(',').any(|entry| {
        entry
            .split(';')
            .skip(1)
            .any(|rel| rel.trim() == "rel=\"next\"")
    })
}

impl Api {
    pub fn same_credentials(&self, other: &Api) -> bool {
        self.secret == other.secret
    }
    pub fn poll_interval(&self) -> u64 {
        self.poll_interval.load(Ordering::Relaxed)
    }
    pub async fn mark_all_read(&self, stamp: &str) -> Result<u16, Error> {
        let body = json!({"last_read_at":stamp,"read":true});
        let response = self
            .send(
                Method::PUT,
                &["notifications"],
                &[],
                Some(&body),
                "application/vnd.github+json",
            )
            .await?;
        match response.status().as_u16() {
            code @ (202 | 205) => Ok(code),
            _ => Err(status(&response)),
        }
    }
    async fn send(
        &self,
        method: Method,
        path: &[&str],
        args: &[(&str, String)],
        body: Option<&Value>,
        accept: &str,
    ) -> Result<Response, Error> {
        if path.is_empty()
            || path.iter().any(|segment| {
                segment.is_empty() || matches!(*segment, "." | "..") || segment.contains('\0')
            })
        {
            return Err(Error::Message("GitHub resource path is invalid."));
        }
        let mut url = self.transport.origin.clone();
        url.path_segments_mut()
            .map_err(|_| Error::Reply)?
            .pop_if_empty()
            .extend(path);
        let mut request = self
            .transport
            .http
            .request(method, url)
            .query(args)
            .header(header::AUTHORIZATION, self.secret.clone())
            .header(header::ACCEPT, accept)
            .header("X-GitHub-Api-Version", API_VERSION);
        if let Some(body) = body {
            request = request.json(body);
        }
        request.send().await.map_err(|_| Error::Network)
    }
    async fn json(
        &self,
        method: Method,
        path: &[&str],
        args: &[(&str, String)],
        body: Option<&Value>,
    ) -> Result<(Value, bool), Error> {
        let method_is_notification_poll = method == Method::GET && path == ["notifications"];
        let response = self
            .send(method, path, args, body, "application/vnd.github+json")
            .await?;
        if method_is_notification_poll {
            if let Some(seconds) =
                headers(&response, "x-poll-interval").and_then(|s| s.parse().ok())
            {
                self.poll_interval.store(seconds, Ordering::Relaxed);
            }
        }
        if !response.status().is_success() {
            return Err(status(&response));
        }
        let next = headers(&response, "link").is_some_and(|value| link_next(&value));
        let bytes = bounded(response, MAX_JSON).await?;
        let value = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).map_err(|_| Error::Reply)?
        };
        Ok((value, next))
    }
    pub async fn get(&self, path: &[&str], args: &[(&str, String)]) -> Result<Value, Error> {
        Ok(self.json(Method::GET, path, args, None).await?.0)
    }
    pub async fn page(
        &self,
        path: &[&str],
        args: &[(&str, String)],
        page: u16,
        per_page: u16,
    ) -> Result<(Value, bool), Error> {
        let mut args = args.to_vec();
        args.extend([
            ("page", page.to_string()),
            ("per_page", per_page.to_string()),
        ]);
        self.json(Method::GET, path, &args, None).await
    }
    pub async fn mutate(
        &self,
        method: Method,
        path: &[&str],
        body: Option<Value>,
    ) -> Result<Value, Error> {
        Ok(self.json(method, path, &[], body.as_ref()).await?.0)
    }
    pub async fn graph(&self, query: &str, variables: Value) -> Result<Value, Error> {
        let body = json!({"query":query,"variables":variables});
        let (mut reply, _) = self
            .json(Method::POST, &["graphql"], &[], Some(&body))
            .await?;
        if reply["errors"]
            .as_array()
            .is_some_and(|errors| !errors.is_empty())
            || !reply["data"].is_object()
        {
            return Err(Error::Graph);
        }
        Ok(reply["data"].take())
    }
    pub async fn raw(&self, path: &[&str], args: &[(&str, String)]) -> Result<String, Error> {
        let response = self
            .send(
                Method::GET,
                path,
                args,
                None,
                "application/vnd.github.raw+json",
            )
            .await?;
        if !response.status().is_success() {
            return Err(status(&response));
        }
        String::from_utf8(bounded(response, 256 * 1024).await?).map_err(|_| Error::Reply)
    }

    pub async fn download(
        &self,
        owner: &str,
        repo: &str,
        reference: Option<&str>,
        directory: &Path,
    ) -> Result<PathBuf, String> {
        if !directory.is_absolute() || !directory.is_dir() {
            return Err("Choose an existing absolute download directory.".into());
        }
        let mut path = vec!["repos", owner, repo, "zipball"];
        path.extend(reference);
        let response = self
            .send(Method::GET, &path, &[], None, "application/vnd.github+json")
            .await?;
        let response = if response.status().is_redirection() {
            let location =
                headers(&response, "location").ok_or("GitHub did not supply a ZIP location.")?;
            let target = self
                .transport
                .origin
                .join(&location)
                .map_err(|_| "Invalid GitHub ZIP location.")?;
            let allowed = if cfg!(feature = "fixture") {
                target.origin() == self.transport.origin.origin()
            } else {
                target.scheme() == "https"
                    && target.host_str() == Some("codeload.github.com")
                    && target.port_or_known_default() == Some(443)
            };
            if !allowed || !target.username().is_empty() || target.password().is_some() {
                return Err("GitHub returned an unexpected ZIP location.".into());
            }
            self.transport
                .http
                .get(target)
                .timeout(Duration::from_secs(35))
                .send()
                .await
                .map_err(|_| Error::Network)?
        } else {
            response
        };
        if !response.status().is_success() {
            return Err(status(&response).into());
        }
        let mut response = response;
        if response
            .content_length()
            .is_some_and(|length| length > MAX_ZIP as u64)
        {
            return Err("Repository ZIP exceeds 128 MiB.".into());
        }
        let mut file = tempfile::NamedTempFile::new_in(directory)
            .map_err(|_| "Could not create a temporary ZIP in the download directory.")?;
        let mut size = 0;
        let mut prefix = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| Error::Network)? {
            size += chunk.len();
            if size > MAX_ZIP {
                return Err("Repository ZIP exceeds 128 MiB.".into());
            }
            if prefix.len() < 4 {
                prefix.extend_from_slice(&chunk[..chunk.len().min(4 - prefix.len())]);
            }
            file.write_all(&chunk)
                .map_err(|_| "Could not write the ZIP to the selected directory.")?;
        }
        if !matches!(prefix.as_slice(), b"PK\x03\x04" | b"PK\x05\x06") {
            return Err("GitHub did not return a ZIP file.".into());
        }
        file.as_file()
            .sync_all()
            .map_err(|_| "Could not finish writing the ZIP.")?;
        let suffix = reference.filter(|name| !name.is_empty()).map(|name| {
            name.chars()
                .take(48)
                .map(|c| {
                    if c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.') {
                        c
                    } else {
                        '-'
                    }
                })
                .collect::<String>()
        });
        let name = match suffix {
            Some(suffix) => format!("{repo}-{suffix}.zip"),
            None => format!("{repo}.zip"),
        };
        let final_path = directory.join(name);
        file.persist_noclobber(&final_path)
            .map_err(|_| "That ZIP already exists, or the destination could not be written.")?;
        Ok(final_path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn notification_header_reaches_poll_scheduler() {
        use std::io::{BufRead, BufReader};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let origin = Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(&stream);
            let mut line = String::new();
            loop {
                line.clear();
                assert!(reader.read_line(&mut line).unwrap() > 0);
                if line == "\r\n" {
                    break;
                }
            }
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nX-Poll-Interval: 7200\r\n\r\n[]",
                )
                .unwrap();
        });
        let api = Transport {
            http: Client::builder().no_proxy().build().unwrap(),
            origin,
        }
        .auth("gh_fixture_only")
        .unwrap();
        api.page(&["notifications"], &[], 1, 25).await.unwrap();
        server.join().unwrap();
        assert_eq!(
            crate::tray::notification_period(api.poll_interval()),
            Duration::from_secs(7200)
        );
        assert_eq!(
            crate::tray::notification_period(30),
            Duration::from_secs(300)
        );
    }
    #[test]
    fn detects_next_link_without_following_its_origin() {
        assert!(link_next(
            "<https://example.invalid/>; rel=\"next\", <https://api.github.com/>; rel=\"last\""
        ));
        assert!(!link_next("<https://api.github.com/>; rel=\"prev\""));
    }
}
