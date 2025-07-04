// Taken from https://github.com/Gelbpunkt/tokio-websockets/
// Modified to use anyhow::Error for simplicity

use anyhow::{Context as _, Result, anyhow, bail};
use base64::{Engine, engine::general_purpose::STANDARD};
use bytes::{Buf, BytesMut};
use httparse::Request;
use std::str::FromStr;
use tokio_util::codec::Decoder;

// The Globally Unique Identifier (GUID) used in the WebSocket protocol (see [the RFC](https://datatracker.ietf.org/doc/html/rfc6455#section-1.3)).
const GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

fn digest(key: &[u8]) -> [u8; 20] {
    let mut ctx = ring::digest::Context::new(&ring::digest::SHA1_FOR_LEGACY_USE_ONLY);
    ctx.update(key);
    ctx.update(GUID.as_bytes());
    ctx.finish().as_ref().try_into().unwrap()
}

// A static HTTP/1.1 101 Switching Protocols response up until the
// `Sec-WebSocket-Accept` header value.
const SWITCHING_PROTOCOLS_BODY: &str = "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: ";

// Returns whether an ASCII byte slice is contained in another one, ignoring
// captalization.
fn contains_ignore_ascii_case(mut haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() {
        return true;
    }

    while haystack.len() >= needle.len() {
        if haystack[..needle.len()].eq_ignore_ascii_case(needle) {
            return true;
        }

        haystack = &haystack[1..];
    }

    false
}

// A client's opening handshake.
struct ClientRequest {
    // The SHA-1 digest of the `Sec-WebSocket-Key` header.
    ws_accept: [u8; 20],
}

impl ClientRequest {
    // Parses the client's opening handshake.
    //
    // # Errors
    //
    // This method fails when a header required for the WebSocket protocol is
    // missing in the handshake.
    fn parse<'a, F>(header: F) -> Result<Self>
    where
        F: Fn(&'static str) -> Option<&'a str> + 'a,
    {
        let find_header = |name| header(name).with_context(|| format!("missing header {name}"));

        let check_header = |name, expected, err: &str| {
            let actual = find_header(name)?;
            if actual.eq_ignore_ascii_case(expected) {
                Ok(())
            } else {
                Err(anyhow!("{}", err))
            }
        };

        let check_header_contains = |name, expected: &str, err: &str| {
            let actual = find_header(name)?;
            if contains_ignore_ascii_case(actual.as_bytes(), expected.as_bytes()) {
                Ok(())
            } else {
                Err(anyhow!("{}", err))
            }
        };

        check_header("Upgrade", "websocket", "upgrade not websocket")?;
        check_header_contains("Connection", "Upgrade", "connection not upgrade")?;
        check_header(
            "Sec-WebSocket-Version",
            "13",
            "unsupported websocket version",
        )?;

        let key = find_header("Sec-WebSocket-Key")?;
        let ws_accept = digest(key.as_bytes());
        Ok(Self { ws_accept })
    }

    // Returns the value that the client expects to see in the server's
    // `Sec-WebSocket-Accept` header.
    #[must_use]
    fn ws_accept(&self) -> String {
        STANDARD.encode(self.ws_accept)
    }
}

// A codec that implements a [`Decoder`] for HTTP/1.1 upgrade requests and
// yields the request and a HTTP/1.1 response to reply with.
//
// It does not implement an [`Encoder`].
//
// [`Encoder`]: tokio_util::codec::Encoder
pub(crate) struct Codec;

impl Decoder for Codec {
    type Error = anyhow::Error;
    type Item = (http::Request<()>, String);

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        let mut headers = [httparse::EMPTY_HEADER; 64];
        let mut request = Request::new(&mut headers);
        let status = request.parse(src).context("request parsing failed")?;

        if !status.is_complete() {
            return Ok(None);
        }

        let request_len = status.unwrap();

        let mut builder = http::request::Builder::new();
        if let Some(m) = request.method {
            let method =
                http::method::Method::from_bytes(m.as_bytes()).expect("httparse method is valid");
            builder = builder.method(method);
        }

        if let Some(uri) = request.path {
            builder = builder.uri(uri);
        }

        match request.version {
            Some(0) => builder = builder.version(http::Version::HTTP_10),
            Some(1) => builder = builder.version(http::Version::HTTP_11),
            _ => bail!("wrong HTTP version"),
        }

        let mut header_map = http::HeaderMap::with_capacity(request.headers.len());

        for header in request.headers {
            let name = http::HeaderName::from_str(header.name).context("invalid header name")?;
            let value =
                http::HeaderValue::from_bytes(header.value).context("invalid header value")?;

            header_map.insert(name, value);
        }

        // You have to build the request before you can assign headers: https://github.com/hyperium/http/issues/91
        let mut request = builder
            .body(())
            .expect("httparse sees the request as valid");
        *request.headers_mut() = header_map;

        let ws_accept =
            ClientRequest::parse(|name| request.headers().get(name).and_then(|h| h.to_str().ok()))?
                .ws_accept();

        src.advance(request_len);

        let mut resp = String::with_capacity(SWITCHING_PROTOCOLS_BODY.len() + ws_accept.len() + 4);

        resp.push_str(SWITCHING_PROTOCOLS_BODY);
        resp.push_str(&ws_accept);
        resp.push_str("\r\n\r\n");

        Ok(Some((request, resp)))
    }
}
