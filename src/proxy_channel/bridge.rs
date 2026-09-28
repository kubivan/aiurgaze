use super::*;
use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use protobuf::Message;
use protobuf::RepeatedField;
use sc2_proto::debug::{DebugCommand, DebugGameState};
use sc2_proto::sc2api::Request_oneof_request;
use tokio::net::TcpListener;
use tokio_tungstenite::{accept_async, connect_async};

pub(super) fn try_parse_request(raw: &[u8]) -> Option<Request> {
    let mut request = Request::new();
    request.merge_from_bytes(raw).ok().map(|_| request)
}

pub(super) fn try_parse_response(raw: &[u8]) -> Option<Response> {
    let mut response = Response::new();
    response.merge_from_bytes(raw).ok().map(|_| response)
}

pub(super) fn is_join_game(request: &Request) -> bool {
    matches!(request.request, Some(Request_oneof_request::join_game(_)))
}

pub(super) fn make_game_info_request() -> Result<Vec<u8>, String> {
    let mut request = Request::new();
    request.mut_game_info();
    request
        .write_to_bytes()
        .map_err(|error| format!("Protobuf encode: {error}"))
}

fn make_disable_fog_observation_request() -> Result<Vec<u8>, String> {
    let mut request = Request::new();
    request.mut_observation().set_disable_fog(true);
    request
        .write_to_bytes()
        .map_err(|error| format!("Protobuf encode: {error}"))
}

fn make_reveal_map_debug_request() -> Result<Vec<u8>, String> {
    let mut request = Request::new();
    let mut command = DebugCommand::new();
    command.set_game_state(DebugGameState::show_map);
    request
        .mut_debug()
        .set_debug(RepeatedField::from_vec(vec![command]));
    request
        .write_to_bytes()
        .map_err(|error| format!("Protobuf encode: {error}"))
}

pub(super) async fn connect_upstream(url: &str) -> Result<UpstreamWs, tungstenite::Error> {
    let mut retries = 10u32;
    let delay = std::time::Duration::from_secs(2);
    println!("[connect_upstream] Connecting to {url}");
    loop {
        match connect_async(url).await {
            Ok((websocket, _)) => {
                println!("[connect_upstream] Connected");
                return Ok(websocket);
            }
            Err(error) => {
                if retries == 0 {
                    return Err(error);
                }
                println!("[connect_upstream] Retry ({retries} left): {error}");
                retries -= 1;
                tokio::time::sleep(delay).await;
            }
        }
    }
}

async fn accept_one_client(
    listener: &TcpListener,
    player_id: PlayerId,
) -> Result<WsStream, Box<dyn std::error::Error + Send + Sync>> {
    println!("[{player_id}] Waiting for client...");
    loop {
        let (stream, address) = listener.accept().await?;
        println!("[{player_id}] Client connected from {address}");
        match accept_async(stream).await {
            Ok(websocket) => return Ok(websocket),
            Err(error) => eprintln!("[{player_id}] Handshake failed (retrying): {error}"),
        }
    }
}

pub(super) async fn roundtrip(
    write: &mut futures_util::stream::SplitSink<UpstreamWs, tungstenite::Message>,
    read: &mut futures_util::stream::SplitStream<UpstreamWs>,
    data: Vec<u8>,
) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
    write
        .send(tungstenite::Message::Binary(Bytes::from(data)))
        .await?;
    let message = read.next().await.ok_or("SC2 closed connection")??;
    Ok(message.into_data().to_vec())
}

pub(super) struct BridgeOptions {
    pub(super) join_barrier: Option<JoinResponseBarrier>,
    pub(super) multiplayer_ports: Option<MultiplayerPorts>,
    pub(super) observer_sender: Option<broadcast::Sender<TaggedResponse>>,
    pub(super) debug_tx: Option<mpsc::Sender<crate::debug_draw::DebugDrawEvent>>,
    pub(super) chat_tx: Option<mpsc::Sender<crate::chat_overlay::ChatMessageEvent>>,
    pub(super) pause_state: Option<Arc<AtomicBool>>,
}

pub(super) fn advance_observer_game_loop(last_game_loop: &mut u32, current_game_loop: u32) -> bool {
    if current_game_loop <= *last_game_loop {
        return false;
    }
    *last_game_loop = current_game_loop;
    true
}

pub(super) async fn poll_observer(
    player_id: PlayerId,
    up_w: &mut futures_util::stream::SplitSink<UpstreamWs, tungstenite::Message>,
    up_r: &mut futures_util::stream::SplitStream<UpstreamWs>,
    publisher: &ProxyPublisher,
    observer_sender: &broadcast::Sender<TaggedResponse>,
    last_game_loop: &mut u32,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let request = make_disable_fog_observation_request()?;
    let response_bytes = roundtrip(up_w, up_r, request).await?;
    let Some(response) = try_parse_response(&response_bytes) else {
        static LOGGED_PARSE_FAIL: AtomicBool = AtomicBool::new(false);
        if !LOGGED_PARSE_FAIL.swap(true, Ordering::Relaxed) {
            eprintln!("[{player_id}] Observer: failed to parse disable_fog response");
        }
        return Ok(());
    };

    let Some(current_game_loop) = response
        .get_observation()
        .observation
        .as_ref()
        .map(|observation| observation.get_game_loop())
    else {
        return Ok(());
    };

    let is_first_observation = *last_game_loop == 0;
    if !advance_observer_game_loop(last_game_loop, current_game_loop) {
        return Ok(());
    }

    if is_first_observation {
        println!(
            "[{player_id}] Observer: first obs at game_loop={current_game_loop}, \
             receivers={}, response_type={}",
            observer_sender.receiver_count(),
            response
                .response
                .as_ref()
                .map(|kind| format!("{:?}", std::mem::discriminant(kind)))
                .unwrap_or_else(|| "None".to_string())
        );
    }
    publisher.publish_to(observer_sender, PlayerId::Player2, response);
    Ok(())
}

pub(super) async fn create_game(
    player_id: PlayerId,
    request: Request,
    up_w: &mut futures_util::stream::SplitSink<UpstreamWs, tungstenite::Message>,
    up_r: &mut futures_util::stream::SplitStream<UpstreamWs>,
    publisher: &ProxyPublisher,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    println!("[{player_id}] Sending CreateGame");
    let request = request
        .write_to_bytes()
        .map_err(|error| format!("Protobuf encode: {error}"))?;
    let response_bytes = roundtrip(up_w, up_r, request).await?;

    if let Some(response) = try_parse_response(&response_bytes) {
        if response.has_create_game() {
            let create_game = response.get_create_game();
            if create_game.has_error() {
                eprintln!(
                    "[{player_id}] CreateGame error: {:?} - {}",
                    create_game.get_error(),
                    create_game.get_error_details()
                );
            } else {
                println!("[{player_id}] CreateGame succeeded");
            }
        }
        publisher.publish(player_id, response);
    }

    Ok(())
}

pub(super) async fn accept_proxy_client(
    listen_addr: &str,
    player_id: PlayerId,
    ready_signal: &ProxyReadySignal,
) -> Result<WsStream, Box<dyn std::error::Error + Send + Sync>> {
    let listener = TcpListener::bind(listen_addr).await?;
    println!("[{player_id}] Listening on ws://{listen_addr}");
    ready_signal.signal_ready();
    accept_one_client(&listener, player_id).await
}

pub(super) async fn accept_and_bridge(
    player_id: PlayerId,
    client_ws: WsStream,
    up_w: &mut futures_util::stream::SplitSink<UpstreamWs, tungstenite::Message>,
    up_r: &mut futures_util::stream::SplitStream<UpstreamWs>,
    publisher: &ProxyPublisher,
    options: BridgeOptions,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let BridgeOptions {
        join_barrier,
        multiplayer_ports,
        observer_sender,
        debug_tx,
        chat_tx,
        pause_state,
    } = options;
    let (mut client_write, mut client_read) = client_ws.split();

    let join_message = client_read
        .next()
        .await
        .ok_or("Client disconnected before JoinGame")??;
    let raw = join_message.into_data().to_vec();
    let mut request = try_parse_request(&raw)
        .filter(is_join_game)
        .ok_or("Expected a valid JoinGame request")?;

    let raw = if let Some(ref ports) = multiplayer_ports {
        ports.inject_into(&mut request);
        request
            .write_to_bytes()
            .map_err(|error| format!("Protobuf re-encode JoinGame: {error}"))?
    } else {
        raw
    };

    println!("[{player_id}] Forwarding JoinGame");
    let response_bytes = roundtrip(up_w, up_r, raw).await?;

    if let Some(response) = try_parse_response(&response_bytes) {
        publisher.publish(player_id, response);
    }
    client_write
        .send(tungstenite::Message::Binary(Bytes::from(response_bytes)))
        .await?;

    if let Some(ref barrier) = join_barrier {
        barrier.mark_joined();
        barrier.wait_ready().await;
    }

    if let Some(ref observer_sender) = observer_sender {
        let request = make_game_info_request()?;
        let response_bytes = roundtrip(up_w, up_r, request).await?;
        if let Some(response) = try_parse_response(&response_bytes) {
            publisher.publish_to(observer_sender, PlayerId::Player2, response);
        }
        println!("[{player_id}] Observer: initial GameInfo published");
    }

    let mut observer_last_game_loop = 0;
    loop {
        while pause_state
            .as_ref()
            .is_some_and(|state| state.load(Ordering::SeqCst))
        {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }

        let Some(message) = client_read.next().await else {
            break;
        };
        let raw = message?.into_data().to_vec();

        if let Some(sender) = &debug_tx {
            if let Some(event) = crate::debug_draw::debug_draw_from_request(player_id, &raw) {
                let _ = sender.try_send(event);
            }
        }
        if let Some(sender) = &chat_tx {
            for event in crate::chat_overlay::chat_messages_from_request(player_id, &raw) {
                let _ = sender.try_send(event);
            }
        }

        let response_bytes = roundtrip(up_w, up_r, raw).await?;
        if let Some(response) = try_parse_response(&response_bytes) {
            publisher.publish(player_id, response);
        }
        client_write
            .send(tungstenite::Message::Binary(Bytes::from(response_bytes)))
            .await?;

        if let Some(observer_sender) = observer_sender.as_ref() {
            poll_observer(
                player_id,
                up_w,
                up_r,
                publisher,
                observer_sender,
                &mut observer_last_game_loop,
            )
            .await?;
        }
    }

    println!("[{player_id}] Proxy finished.");
    Ok(())
}
