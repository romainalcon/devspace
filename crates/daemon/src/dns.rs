//! DNS server for the local domains, on `127.0.0.1` (UDP).
//!
//! The answers are computed by [`devspace_core::web::dns::answer`]; this
//! module only moves datagrams.

use std::io;
use std::net::{Ipv4Addr, SocketAddr};

use devspace_core::config::WebConfig;
use tokio::net::UdpSocket;

/// Largest datagram read; queries for the local domains are much smaller.
const MAX_DATAGRAM_LEN: usize = 1500;

/// Binds the DNS socket on `127.0.0.1:port`.
pub(crate) async fn bind(port: u16) -> io::Result<UdpSocket> {
    UdpSocket::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, port))).await
}

/// Answers queries for the domain of `web` until the task is cancelled.
pub(crate) async fn serve(socket: UdpSocket, web: WebConfig) {
    let address = web.address.ip();
    let mut buffer = vec![0u8; MAX_DATAGRAM_LEN];
    loop {
        let (len, peer) = match socket.recv_from(&mut buffer).await {
            Ok(received) => received,
            Err(err) => {
                // E.g. an ICMP "port unreachable" reported on the socket
                // (Windows): not fatal for the server.
                tracing::debug!(error = %err, "DNS receive error");
                continue;
            }
        };
        let Some(response) = devspace_core::web::dns::answer(&buffer[..len], &web.tld, address)
        else {
            continue;
        };
        if let Err(err) = socket.send_to(&response, peer).await {
            tracing::debug!(error = %err, %peer, "failed to send a DNS response");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn answers_over_udp() {
        let socket = bind(0).await.unwrap();
        let addr = socket.local_addr().unwrap();
        let server = tokio::spawn(serve(socket, WebConfig::default()));

        let client = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let mut query = vec![0xab, 0xcd, 0x01, 0x00, 0, 1, 0, 0, 0, 0, 0, 0];
        for label in ["blog", "internal"] {
            query.push(u8::try_from(label.len()).unwrap());
            query.extend_from_slice(label.as_bytes());
        }
        query.extend_from_slice(&[0, 0, 1, 0, 1]);
        client.send_to(&query, addr).await.unwrap();
        let mut response = [0u8; 512];
        let (len, _) = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            client.recv_from(&mut response),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(&response[..2], &[0xab, 0xcd]);
        assert_eq!(&response[len - 4..len], &[127, 0, 0, 2]);
        server.abort();
    }
}
