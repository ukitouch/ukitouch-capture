use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncWriteExt, Stdout};
use tokio::sync::Mutex;

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Incoming {
    /// One method call from the host.
    Call {
        id: u64,
        method: String,
        #[serde(default)]
        payload: Value,
    },
}

#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Outgoing {
    /// The first message, register this extension with its information.
    Register {
        capabilities: Vec<String>,
        version: String,
    },
    /// Response to a single call.
    Reply {
        id: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        result: Option<Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
}

pub type Out = Arc<Mutex<Stdout>>;

/// Wrap stdout for the request handlers.
pub fn stdout() -> Out {
    Arc::new(Mutex::new(tokio::io::stdout()))
}

pub async fn register(out: &Out) {
    let capabilities = [
        "start_screen_stream",
        "stream_configure",
        "stop_screen_stream",
    ]
    .map(String::from)
    .to_vec();
    send(
        out,
        &Outgoing::Register {
            capabilities,
            version: env!("CARGO_PKG_VERSION").into(),
        },
    )
    .await;
}

pub async fn reply(out: &Out, id: u64, result: Result<Value, String>) {
    let (result, error) = match result {
        Ok(value) => (Some(value), None),
        Err(error) => (None, Some(error)),
    };
    send(out, &Outgoing::Reply { id, result, error }).await;
}

async fn send(out: &Out, msg: &Outgoing) {
    let Ok(mut line) = serde_json::to_string(msg) else {
        return;
    };
    line.push('\n');
    let mut stdout = out.lock().await;
    let _ = stdout.write_all(line.as_bytes()).await;
    let _ = stdout.flush().await;
}
