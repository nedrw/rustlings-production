//! An HTTP client small enough to read in one sitting: the fixture's way of talking to a service
//! and of looking at what came back.
//!
//! It exists because a test that cannot *send* a header cannot test authentication, and a test that
//! cannot *read* one cannot test a correlation id, an `Allow` on a 405, or a `Retry-After` on a 429.
//! Nothing here parses cookies or JSON payloads beyond what a test needs: what a test asserts has to
//! stay visible in the test.

#![allow(dead_code)] // a spec uses the part of this harness it needs, not all of it

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

/// How long any one request may take before the fixture gives up on it.
pub const DEADLINE: Duration = Duration::from_secs(20);

/// A connection target (`127.0.0.1:port`) plus the deadline every request on it gets.
#[derive(Debug, Clone, Copy)]
pub struct Client {
    address: SocketAddr,
}

/// One request, built by the helpers below.
#[derive(Debug, Clone)]
pub struct Request {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

/// One answer, as the fixture saw it.
#[derive(Debug, Clone)]
pub struct Response {
    /// Status code from the status line.
    pub status: u16,
    /// Headers as they arrived, names in the case the server sent them.
    pub headers: Vec<(String, String)>,
    /// Body, as text.
    pub body: String,
}

impl Request {
    /// `GET path`.
    #[must_use]
    pub fn get(path: &str) -> Self {
        Self::new("GET", path)
    }

    /// `POST path` with a body.
    #[must_use]
    pub fn post(path: &str, body: impl Into<Vec<u8>>) -> Self {
        let mut request = Self::new("POST", path);
        request.body = body.into();
        request
    }

    /// One method and path, with no headers and no body yet.
    #[must_use]
    pub fn new(method: &str, path: &str) -> Self {
        Self {
            method: method.to_owned(),
            path: path.to_owned(),
            headers: Vec::new(),
            body: Vec::new(),
        }
    }

    /// Add a header.
    #[must_use]
    pub fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_owned(), value.to_owned()));
        self
    }

    /// Add a JSON body and the matching `content-type`.
    #[must_use]
    pub fn json(self, value: &serde_json::Value) -> Self {
        let body = value.to_string();
        self.header("content-type", "application/json").body(body)
    }

    /// Replace the body.
    #[must_use]
    pub fn body(mut self, body: impl Into<Vec<u8>>) -> Self {
        self.body = body.into();
        self
    }

    /// `Authorization: Bearer <key>`, the one header the service treats as identity.
    #[must_use]
    pub fn bearer(self, key: &str) -> Self {
        self.header("authorization", &format!("Bearer {key}"))
    }

    /// The request as bytes on a socket.
    fn to_bytes(&self, address: SocketAddr) -> Vec<u8> {
        let mut head = format!(
            "{} {} HTTP/1.1\r\nhost: {address}\r\n",
            self.method, self.path
        );
        for (name, value) in &self.headers {
            head.push_str(&format!("{name}: {value}\r\n"));
        }
        head.push_str(&format!(
            "content-length: {}\r\nconnection: close\r\n\r\n",
            self.body.len()
        ));
        let mut bytes = head.into_bytes();
        bytes.extend_from_slice(&self.body);
        bytes
    }
}

impl Client {
    /// A client for `address`.
    #[must_use]
    pub fn new(address: SocketAddr) -> Self {
        Self { address }
    }

    /// Send one request and read the whole answer.
    ///
    /// Panics when the connection, the write or the read fails: inside a test, "the service is not
    /// there" is a failure with a sentence, and the sentence names the address.
    pub fn send(&self, request: Request) -> Response {
        let mut stream = TcpStream::connect(self.address).unwrap_or_else(|error| {
            panic!(
                "the service at {} is not accepting connections: {error}",
                self.address
            )
        });
        stream
            .set_read_timeout(Some(DEADLINE))
            .expect("a read deadline can be set");
        stream
            .write_all(&request.to_bytes(self.address))
            .unwrap_or_else(|error| panic!("the request could not be sent: {error}"));
        let mut raw = Vec::new();
        stream
            .read_to_end(&mut raw)
            .unwrap_or_else(|error| panic!("the answer could not be read: {error}"));
        parse(&String::from_utf8_lossy(&raw))
    }

    /// `GET path`.
    pub fn get(&self, path: &str) -> Response {
        self.send(Request::get(path))
    }

    /// `GET path` with a bearer token.
    pub fn get_with_key(&self, path: &str, key: &str) -> Response {
        self.send(Request::get(path).bearer(key))
    }

    /// `POST path` with a JSON body and a bearer token.
    pub fn post_json(&self, path: &str, key: &str, body: &serde_json::Value) -> Response {
        self.send(Request::post(path, Vec::new()).bearer(key).json(body))
    }
}

impl Response {
    /// A header by name, case-insensitively.
    #[must_use]
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(header, _)| header.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    /// The body parsed as JSON.
    ///
    /// Panics with the body in the message when it is not JSON: a test that expected JSON and got
    /// HTML needs to see the HTML.
    #[must_use]
    pub fn json(&self) -> serde_json::Value {
        serde_json::from_str(&self.body)
            .unwrap_or_else(|error| panic!("the body is not JSON ({error}): {}", self.body))
    }

    /// A field of the JSON body, by name, panicking with the body when it is missing.
    #[must_use]
    pub fn field(&self, name: &str) -> serde_json::Value {
        let json = self.json();
        json.get(name)
            .cloned()
            .unwrap_or_else(|| panic!("the body has no `{name}`: {json}"))
    }
}

/// Split an HTTP answer into status, headers and body.
fn parse(raw: &str) -> Response {
    let (head, body) = raw
        .split_once("\r\n\r\n")
        .unwrap_or_else(|| panic!("an HTTP answer has a head and a body: `{raw}`"));
    let mut lines = head.lines();
    let status = lines
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse().ok())
        .unwrap_or_else(|| panic!("the status line has a code: `{head}`"));
    let headers = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_owned(), value.trim().to_owned()))
        .collect();
    Response {
        status,
        headers,
        body: body.to_owned(),
    }
}
