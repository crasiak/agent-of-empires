use super::*;
use std::{io::Write, path::PathBuf, process::Command};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
    Put,
}
impl Method {
    fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
            Self::Put => "PUT",
        }
    }
}
/// Privately constructed by the producer; never a model/user command or arbitrary URL.
#[derive(Debug)]
pub struct HttpRequest {
    method: Method,
    path: String,
    body: Option<Vec<u8>>,
}
impl HttpRequest {
    pub(super) fn new(method: Method, path: String, body: Option<Value>) -> Result<Self> {
        ensure!(
            path.len() <= 1024
                && path.starts_with('/')
                && !path.starts_with("//")
                && path.is_ascii()
                && !path
                    .bytes()
                    .any(|b| b <= 32 || b == 127 || b == b'#' || b == b'\\'),
            "Unsafe typed HTTP path"
        );
        let body = body.map(|b| serde_json::to_vec(&b)).transpose()?;
        ensure!(
            body.as_ref().is_none_or(|b| b.len() <= MAX_BODY),
            "HTTP request body exceeds bound"
        );
        Ok(Self { method, path, body })
    }
    pub fn method(&self) -> Method {
        self.method
    }
    pub fn path(&self) -> &str {
        &self.path
    }
    pub fn body(&self) -> Option<&[u8]> {
        self.body.as_deref()
    }
    pub fn api_version(&self) -> &'static str {
        API_VERSION
    }
}
#[derive(Debug)]
pub struct HttpResponse {
    pub status: u16,
    pub body: Vec<u8>,
    pub next_page: bool,
}
/// Implementations are host-owned authenticated capabilities, not model tools.
/// They must enforce a deadline, response/body limits and no automatic write retries.
pub trait Transport {
    fn send(&self, request: &HttpRequest) -> Result<HttpResponse>;
}

pub struct GhTransport {
    executable: PathBuf,
    timeout: Duration,
}
impl GhTransport {
    pub fn new() -> Result<Self> {
        Ok(Self {
            executable: which::which("gh").context("Host GitHub CLI is required")?,
            timeout: Duration::from_secs(20),
        })
    }
}
impl Transport for GhTransport {
    fn send(&self, request: &HttpRequest) -> Result<HttpResponse> {
        // A bounded regular input file avoids a writer thread or inherited stdin pipe.
        let mut input = tempfile::NamedTempFile::new()?;
        if let Some(body) = request.body() {
            input.write_all(body)?;
            input.flush()?;
        }
        let cwd = tempfile::tempdir()?;
        let mut command = Command::new(&self.executable);
        command
            .current_dir(cwd.path())
            .env("GH_PROMPT_DISABLED", "1")
            .env("GH_PAGER", "")
            .env("PAGER", "")
            .env_remove("GH_DEBUG")
            .env_remove("GH_FORCE_TTY")
            .args([
                "api",
                "--hostname",
                "github.com",
                "--method",
                request.method.as_str(),
                "--include",
                "--header",
                "Accept: application/vnd.github+json",
                "--header",
                &format!("X-GitHub-Api-Version: {API_VERSION}"),
            ]);
        if request.body.is_some() {
            command.arg("--input").arg(input.path());
        }
        command.arg(&request.path);
        let output = crate::process::run_with_bounded_stdout(
            &mut command,
            self.timeout,
            MAX_BODY + 32 * 1024,
        )?
        .context("GitHub outcome unknown: deadline/output bound exceeded; do not replay write")?;
        let response = decode_http(&output.stdout)?;
        ensure!(
            output.status.success() || response.status >= 400,
            "GitHub process failed without authoritative response"
        );
        Ok(response)
    }
}
pub(super) fn decode_http(bytes: &[u8]) -> Result<HttpResponse> {
    ensure!(bytes.len() <= MAX_BODY + 32 * 1024, "HTTP output too large");
    let boundary = bytes
        .windows(4)
        .position(|b| b == b"\r\n\r\n")
        .map(|p| (p, 4))
        .or_else(|| bytes.windows(2).position(|b| b == b"\n\n").map(|p| (p, 2)))
        .context("Missing complete HTTP headers")?;
    ensure!(boundary.0 <= 32 * 1024, "HTTP headers too large");
    let headers = std::str::from_utf8(&bytes[..boundary.0])?;
    let mut lines = headers.lines();
    let mut status = lines
        .next()
        .context("Missing HTTP status")?
        .split_whitespace();
    ensure!(
        matches!(status.next(), Some("HTTP/1.1" | "HTTP/2.0" | "HTTP/2")),
        "Unknown HTTP protocol"
    );
    let status: u16 = status.next().context("Missing HTTP status code")?.parse()?;
    ensure!((200..=599).contains(&status), "Unsupported HTTP status");
    let mut next_page = false;
    let mut length = None;
    for line in lines {
        let (name, value) = line.split_once(':').context("Malformed HTTP header")?;
        if name.eq_ignore_ascii_case("link") {
            next_page |= value.contains("rel=\"next\"");
        }
        if name.eq_ignore_ascii_case("content-length") {
            ensure!(length.is_none(), "Duplicate Content-Length");
            length = Some(value.trim().parse::<usize>()?);
        }
    }
    let body = bytes[boundary.0 + boundary.1..].to_vec();
    ensure!(
        body.len() <= MAX_BODY && length.is_none_or(|n| n == body.len()),
        "Truncated/oversized HTTP body"
    );
    Ok(HttpResponse {
        status,
        body,
        next_page,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn raw_http_rejects_truncation_and_conflicting_lengths() {
        for raw in [
            b"HTTP/2.0 200 OK\r\nContent-Length: 3\r\n\r\n{}".as_slice(),
            b"HTTP/2.0 200 OK\r\nContent-Length: 2\r\nContent-Length: 2\r\n\r\n{}",
            b"HTTP/2.0 200 OK\r\n",
            b"HTTP/2.0 302 Found\r\nLocation: https://evil.test\r\n\r\n",
        ] {
            if let Ok(response) = decode_http(raw) {
                assert_ne!(response.status, 200);
            }
        }
    }
    #[cfg(unix)]
    #[test]
    fn host_gh_producer_uses_fixed_http_argv_bounded_regular_input_and_no_shell() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        let executable = home.path().join("fake-gh");
        let args = home.path().join("args");
        let input = home.path().join("input");
        let script=format!("#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\nwhile [ \"$#\" -gt 0 ]; do\n if [ \"$1\" = --input ]; then shift; /bin/cat \"$1\" > '{}'; fi\n shift\ndone\nprintf 'HTTP/2.0 201 Created\\r\\nContent-Length: 2\\r\\n\\r\\n{{}}'\n",args.display(),input.display());
        std::fs::write(&executable, script).unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        let transport = GhTransport {
            executable,
            timeout: Duration::from_secs(5),
        };
        let request = HttpRequest::new(
            Method::Post,
            "/repos/acme/skills/pulls".into(),
            Some(json!({"title":"$(do-not-execute)","head":"workshop/fake"})),
        )
        .unwrap();
        assert_eq!(transport.send(&request).unwrap().status, 201);
        let argv = std::fs::read_to_string(args).unwrap();
        assert!(argv.starts_with("api\n--hostname\ngithub.com\n--method\nPOST\n--include\n"));
        assert!(argv.contains("X-GitHub-Api-Version: 2026-03-10\n"));
        assert!(argv.ends_with("/repos/acme/skills/pulls\n"));
        assert!(!argv.contains("do-not-execute"));
        assert_eq!(std::fs::read(input).unwrap(), request.body.unwrap());
    }
    #[cfg(unix)]
    #[test]
    fn gh_deadline_and_output_bound_return_unknown_without_pipe_hang() {
        use std::os::unix::fs::PermissionsExt;
        for script in [
            "#!/bin/sh\nexec /bin/sleep 60\n",
            "#!/bin/sh\nexec /usr/bin/yes oversized\n",
        ] {
            let home = tempfile::tempdir().unwrap();
            let executable = home.path().join("fake-gh");
            std::fs::write(&executable, script).unwrap();
            std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
            let transport = GhTransport {
                executable,
                timeout: Duration::from_millis(100),
            };
            let request = HttpRequest::new(Method::Get, "/user".into(), None).unwrap();
            let start = Instant::now();
            assert!(transport.send(&request).is_err());
            assert!(start.elapsed() < Duration::from_secs(5));
        }
    }
}
