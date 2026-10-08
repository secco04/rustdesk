// M1 (plans/soft-frolicking-thimble.md): kcp-sys's build.rs uses bindgen in a way that doesn't
// reliably pick up the NDK cross-compile clang flags (--target/--sysroot) in this build â€” flaky
// across otherwise-identical runs. It's RustDesk's optional low-latency UDP transport; plain
// TCP/relay connect works without it, and it was already out of scope for M1-M4 in the plan. This
// keeps the same public API (KcpStream::connect/accept, both returning `ResultType<(Self, Stream)>`)
// so client.rs and rendezvous_mediator.rs need zero changes â€” only this file's internals differ.
// Restore the real kcp-sys dependency + this file's original body once KCP transport is needed.

use hbb_common::{bytes::BytesMut, tokio::net::UdpSocket, ResultType, Stream};
use std::sync::Arc;

pub struct KcpStream;

impl KcpStream {
    /// Upstream (1.5.0) API: how long the peer has been silent. Never connected here, so None.
    pub fn peer_silent_for(&self) -> Option<std::time::Duration> {
        None
    }

    pub async fn accept(
        _udp_socket: Arc<UdpSocket>,
        _timeout: std::time::Duration,
        _init_packet: Option<BytesMut>,
    ) -> ResultType<(Self, Stream)> {
        Err(hbb_common::anyhow::anyhow!(
            "KCP transport deferred (see src/kcp_stream.rs)"
        ))
    }

    pub async fn connect(
        _udp_socket: Arc<UdpSocket>,
        _timeout: std::time::Duration,
    ) -> ResultType<(Self, Stream)> {
        Err(hbb_common::anyhow::anyhow!(
            "KCP transport deferred (see src/kcp_stream.rs)"
        ))
    }
}
