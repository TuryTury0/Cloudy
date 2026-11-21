//! The user's proxy: a local VPN client such as v2rayN, usually at 127.0.0.1:10808 (SOCKS5). Some services only
//! answer through it and others only without it, so each model, the voice and the web each choose.

use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// "127.0.0.1:10808" becomes "socks5h://127.0.0.1:10808" (names are resolved by the proxy, so blocked DNS does not
/// matter); a full address (http://…, socks5://…) is kept; empty means no proxy.
pub fn proxy_url(address: &str) -> Option<String> {
    let address = address.trim().trim_end_matches('/');
    if address.is_empty() {
        return None;
    }
    Some(if address.contains("://") { address.to_string() } else { format!("socks5h://{address}") })
}

/// An HTTP client that sends everything through the proxy.
pub fn client_through(proxy: &str, builder: reqwest::ClientBuilder) -> Result<reqwest::Client, String> {
    let proxy = reqwest::Proxy::all(proxy).map_err(|e| format!("Bad proxy address \"{proxy}\": {e}"))?;
    builder.proxy(proxy).build().map_err(|e| e.to_string())
}

/// A TCP connection to host:port through the proxy (SOCKS5 or HTTP CONNECT), for the voice connection.
pub async fn connect_through(proxy: &str, host: &str, port: u16) -> Result<TcpStream, String> {
    let (scheme, address) = proxy.split_once("://").unwrap_or(("socks5h", proxy));
    let address = address.trim_end_matches('/');
    let connecting = async {
        match scheme {
            "socks5" | "socks5h" | "socks" => tokio_socks::tcp::Socks5Stream::connect(address, (host, port))
                .await
                .map(|s| s.into_inner())
                .map_err(|e| format!("Proxy {address}: {e}")),
            "http" => {
                let mut stream = TcpStream::connect(address).await.map_err(|e| format!("Proxy {address}: {e}"))?;
                let request = format!("CONNECT {host}:{port} HTTP/1.1\r\nHost: {host}:{port}\r\n\r\n");
                stream.write_all(request.as_bytes()).await.map_err(|e| format!("Proxy {address}: {e}"))?;
                let mut head = Vec::new();
                let mut byte = [0u8; 1];
                while !head.ends_with(b"\r\n\r\n") {
                    if stream.read(&mut byte).await.map_err(|e| format!("Proxy {address}: {e}"))? == 0 || head.len() > 8192 {
                        return Err(format!("Proxy {address} closed the connection."));
                    }
                    head.push(byte[0]);
                }
                let head = String::from_utf8_lossy(&head);
                match head.split_whitespace().nth(1) {
                    Some("200") => Ok(stream),
                    _ => Err(format!("Proxy {address} refused: {}", head.lines().next().unwrap_or_default())),
                }
            }
            other => Err(format!("Unknown proxy type \"{other}\"; use socks5://host:port or http://host:port.")),
        }
    };
    tokio::time::timeout(CONNECT_TIMEOUT, connecting).await.map_err(|_| format!("Proxy {address} did not answer (is the VPN app running?)."))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bare_address_means_socks5() {
        assert_eq!(proxy_url(" 127.0.0.1:10808 ").as_deref(), Some("socks5h://127.0.0.1:10808"));
        assert_eq!(proxy_url("http://127.0.0.1:10809/").as_deref(), Some("http://127.0.0.1:10809"));
        assert_eq!(proxy_url("  "), None);
    }
}
