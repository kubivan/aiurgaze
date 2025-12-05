use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use protobuf::Message;
use sc2_proto::sc2api::{Request, Response};
use tokio_tungstenite::connect_async;
use tokio::runtime::Runtime;
use tokio::time::sleep;
use std::time::Duration;
use tokio_tungstenite::tungstenite::Error as WsError;
use tokio_tungstenite::WebSocketStream;
use std::net::TcpStream;

pub fn send_create_game_request(request: Request, ws_url: &str, max_retries: u32, retry_delay: u64) -> Result<(), String> {
    // Run async code in a blocking context
    let rt = Runtime::new().map_err(|e| format!("Tokio runtime error: {}", e))?;
    rt.block_on(async move {
        let mut attempt = 0;
        let mut ws_stream_opt = None;
        while attempt < max_retries {
            match connect_async(ws_url).await {
                Ok((ws_stream, _)) => {
                    ws_stream_opt = Some(ws_stream);
                    break;
                }
                Err(e) => {
                    println!("Failed to connect to {}: {}", ws_url, e);
                    attempt += 1;
                    if attempt < max_retries {
                        sleep(Duration::from_secs(retry_delay)).await;
                    } else {
                        return Err(format!("WebSocket connect error after {} attempts: {}", max_retries, e));
                    }
                }
            }
        }
        let mut ws_stream = ws_stream_opt.ok_or_else(|| "WebSocket connect failed".to_string())?;
        let bytes = request.write_to_bytes().map_err(|e| format!("Protobuf serialization error: {}", e))?;
        ws_stream.send(tungstenite::Message::Binary(Bytes::from(bytes))).await.map_err(|e| format!("WebSocket send error: {}", e))?;
        // Wait for a response
        if let Some(msg) = ws_stream.next().await {
            match msg {
                Ok(tungstenite::Message::Binary(resp_bytes)) => {
                    let sc2_resp = Response::parse_from_bytes(resp_bytes.iter().as_slice())
                        .map_err(|e| format!("Failed to parse SC2 response: {}", e))?;
                    println!("got response {:?}", sc2_resp);

                    // Check for top-level errors
                    let errors = sc2_resp.get_error();
                    if !errors.is_empty() {
                        return Err(format!("SC2 error: {}", errors.join(", ")));
                    }

                    // Check for create_game specific errors
                    if sc2_resp.has_create_game() {
                        let create_game_resp = sc2_resp.get_create_game();
                        if create_game_resp.has_error() {
                            let error_details = if create_game_resp.has_error_details() {
                                create_game_resp.get_error_details()
                            } else {
                                "No details"
                            };
                            return Err(format!("CreateGame failed: {:?} - {}", 
                                create_game_resp.get_error(), error_details));
                        }
                    }

                    Ok(())
                }
                Ok(_) => Err("Unexpected non-binary response".to_string()),
                Err(e) => Err(format!("WebSocket receive error: {}", e)),
            }
        } else {
            Err("No response from server".to_string())
        }
    })
}
