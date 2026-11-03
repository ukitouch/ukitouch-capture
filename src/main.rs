mod protocol;

use serde::Deserialize;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, BufReader};

use capture::stream::{StreamOptions, StreamState};
use capture::Started;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MonitorId {
    id: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StartStream {
    #[serde(default)]
    options: Option<StreamOptions>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConfigureStream {
    id: u64,
    options: StreamOptions,
}

#[tokio::main]
async fn main() {
    let out = protocol::stdout();
    protocol::register(&out).await;

    let state = StreamState::default();
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let Ok(msg) = serde_json::from_str::<protocol::Incoming>(&line) else {
            continue;
        };
        match msg {
            protocol::Incoming::Call {
                id,
                method,
                payload,
            } => {
                let result = dispatch(&state, &method, payload).await;
                protocol::reply(&out, id, result).await;
            }
        }
    }
}

/// Route one call to the matching capture operation.
async fn dispatch(state: &StreamState, method: &str, payload: Value) -> Result<Value, String> {
    match method {
        "start_screen_stream" => {
            started(state.start(parse::<StartStream>(payload)?.options).await?)
        }
        "stream_configure" => {
            let args = parse::<ConfigureStream>(payload)?;
            state.configure(args.id, args.options)?;
            Ok(Value::Null)
        }
        "stop_screen_stream" => {
            state.stop(parse::<MonitorId>(payload)?.id)?;
            Ok(Value::Null)
        }
        other => Err(format!("unknown method: {other}")),
    }
}

fn parse<T: serde::de::DeserializeOwned>(payload: Value) -> Result<T, String> {
    serde_json::from_value(payload).map_err(|e| e.to_string())
}

fn started(started: Started) -> Result<Value, String> {
    serde_json::to_value(started).map_err(|e| e.to_string())
}
