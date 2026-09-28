//! How the gateway accepts client connections.
//!
//! Every accepted socket sets `TCP_NODELAY`. A realtime client streams audio in 20 ms frames and
//! reads small events back; with Nagle on, each small write the gateway relays is held until the
//! client's next frame acknowledges the previous one, which adds a whole frame interval (~20 ms)
//! to every relayed frame (FRD-023 TC-PERF-01: p50 19.5 ms added, against a 5 ms budget). The
//! gateway's vendor sockets already set it; the client side did not.

use std::net::SocketAddr;

use axum::serve::{ListenerExt as _, TapIo};
use axum_server::accept::NoDelayAcceptor;
use axum_server::tls_rustls::{RustlsAcceptor, RustlsConfig};
use tokio::net::{TcpListener, TcpStream};

fn set_nodelay(tcp: &mut TcpStream) {
    if let Err(e) = tcp.set_nodelay(true) {
        tracing::warn!(error = %e, "could not set TCP_NODELAY on an accepted socket");
    }
}

/// The plain-TCP listener the gateway serves on (`axum::serve`), with `TCP_NODELAY` on accept.
pub fn nodelay_listener(listener: TcpListener) -> TapIo<TcpListener, fn(&mut TcpStream)> {
    listener.tap_io(set_nodelay as fn(&mut TcpStream))
}

/// The TLS server (`axum_server`), with `TCP_NODELAY` on accept.
pub fn tls_server(
    addr: SocketAddr,
    config: RustlsConfig,
) -> axum_server::Server<RustlsAcceptor<NoDelayAcceptor>> {
    axum_server::bind(addr).acceptor(RustlsAcceptor::new(config).acceptor(NoDelayAcceptor::new()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::serve::Listener as _;

    /// TC-PERF-01's cause, pinned: an accepted client socket has Nagle off.
    #[tokio::test]
    async fn accepted_sockets_set_tcp_nodelay() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let mut listener = nodelay_listener(listener);
        let _client = TcpStream::connect(addr).await.unwrap();
        let (accepted, _) = listener.accept().await;
        assert!(
            accepted.nodelay().unwrap(),
            "TCP_NODELAY must be set on accept"
        );
    }
}
