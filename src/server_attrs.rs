//! Request attributes shared by the server layers.

use crate::core::REDACT_PII;
use http::{Request, Version, header, uri::Authority};
use serde_json::Value;
use std::{
    net::{IpAddr, SocketAddr},
    sync::atomic::Ordering,
};

pub(crate) fn header_text<'r, B>(request: &'r Request<B>, name: &str) -> Option<&'r str> {
    request
        .headers()
        .get(name)
        .and_then(|value| value.to_str().ok())
}

/// `client.address` is the TCP peer, or the first `X-Forwarded-For` address when
/// `trust_forwarded_for` says a proxy sets that header. `redact_pii` drops both.
fn client_attributes<B>(
    request: &Request<B>,
    peer: Option<SocketAddr>,
    trust_forwarded_for: bool,
) -> Vec<(String, Value)> {
    if REDACT_PII.load(Ordering::Relaxed) {
        return Vec::new();
    }
    let forwarded = trust_forwarded_for
        .then(|| header_text(request, "x-forwarded-for"))
        .flatten()
        .and_then(|value| value.split(',').next())
        .and_then(|first| first.trim().parse::<IpAddr>().ok())
        .map(|ip| ip.to_string());
    let mut attributes = Vec::new();
    if let Some(client) = forwarded.or_else(|| peer.map(|peer| peer.ip().to_string())) {
        attributes.push(("client.address".to_owned(), Value::from(client)));
    }
    if let Some(peer) = peer {
        let address = peer.ip().to_string();
        attributes.push(("network.peer.address".to_owned(), Value::from(address)));
        attributes.push(("network.peer.port".to_owned(), Value::from(peer.port())));
    }
    attributes
}

/// Attributes every server span gets, HTTP or gRPC.
pub(crate) fn connection_attributes<B>(
    request: &Request<B>,
    peer: Option<SocketAddr>,
    trust_forwarded_for: bool,
) -> Vec<(String, Value)> {
    let mut attributes = client_attributes(request, peer, trust_forwarded_for);
    // HTTP/2 carries the authority in the URI, HTTP/1.1 in `Host`.
    let authority = request.uri().authority().cloned().or_else(|| {
        header_text(request, header::HOST.as_str()).and_then(|host| host.parse::<Authority>().ok())
    });
    if let Some(authority) = authority {
        // IPv6 hosts arrive bracketed; the attribute wants the bare address.
        let host = authority
            .host()
            .trim_start_matches('[')
            .trim_end_matches(']');
        attributes.push(("server.address".to_owned(), Value::from(host)));
        if let Some(port) = authority.port_u16() {
            attributes.push(("server.port".to_owned(), Value::from(port)));
        }
    }
    if let Some(agent) = header_text(request, header::USER_AGENT.as_str()) {
        attributes.push(("user_agent.original".to_owned(), Value::from(agent)));
    }
    let version = match request.version() {
        Version::HTTP_10 => Some("1.0"),
        Version::HTTP_11 => Some("1.1"),
        Version::HTTP_2 => Some("2"),
        Version::HTTP_3 => Some("3"),
        _ => None,
    };
    if let Some(version) = version {
        attributes.push(("network.protocol.version".to_owned(), Value::from(version)));
    }
    attributes
}
