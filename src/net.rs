//! Length-prefixed messages to the host's `pipe_bind` bridge.
//!
//! Each message is: a kind byte, then the payload.
//! The host reads the kind byte to pick text or binary on the channel.

use std::io;

use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Notify;

/// The host sends this payload as a data channel message.
const BINARY: u8 = 0;

/// The host sends this payload as a data channel text message.
const TEXT: u8 = 1;

/// One established connection to the host.
pub struct Pipe {
    stream: TcpStream,
}

/// Bind a loopback address and report it for the host to connect to.
pub async fn listen() -> io::Result<(TcpListener, String)> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
    let addr = listener.local_addr()?.to_string();
    Ok((listener, addr))
}

/// Wait for the host to connect or stop.
pub async fn accept(listener: TcpListener, stop: &Notify) -> Option<Pipe> {
    let stream = tokio::select! {
        accepted = listener.accept() => accepted.ok()?.0,
        _ = stop.notified() => return None,
    };
    Some(Pipe::new(stream))
}

impl Pipe {
    pub fn new(stream: TcpStream) -> Self {
        // Frames are small and latency matters more than packet count.
        let _ = stream.set_nodelay(true);
        Self { stream }
    }

    /// Send a JSON message.
    pub async fn text(&mut self, msg: &str) -> io::Result<()> {
        self.send(TEXT, msg.as_bytes()).await
    }

    /// Send raw bytes.
    pub async fn binary(&mut self, data: &[u8]) -> io::Result<()> {
        self.send(BINARY, data).await
    }

    async fn send(&mut self, kind: u8, payload: &[u8]) -> io::Result<()> {
        self.stream
            .write_all(&(payload.len() as u32).to_le_bytes())
            .await?;
        self.stream.write_all(&[kind]).await?;
        self.stream.write_all(payload).await?;
        self.stream.flush().await
    }
}
