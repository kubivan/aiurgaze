use crate::frame::{Frame, FrameSource};
use crate::proxy_channel::{
    bridge, PlayerId, ProxyDataChannel, ProxyPublisher, ProxyReadySignal, TaggedResponse,
    UpstreamWs,
};
use crate::sources::protocol::ResponseFrameAssembler;
use futures_util::StreamExt;
use protobuf::Message;
use sc2_proto::sc2api::{Request, Response};
use tokio_tungstenite::tungstenite;

type UpstreamWriter = futures_util::stream::SplitSink<UpstreamWs, tungstenite::Message>;
type UpstreamReader = futures_util::stream::SplitStream<UpstreamWs>;

#[derive(Default)]
pub(crate) struct FrameAssembler(ResponseFrameAssembler);

impl FrameAssembler {
    pub(crate) fn push(&mut self, response: TaggedResponse) -> Option<Frame> {
        self.0
            .push(FrameSource::Replay, response.player_id, response.response)
    }
}

pub(crate) fn spawn_frames(
    runtime: &bevy_tokio_tasks::TokioTasksRuntime,
    responses: tokio::sync::mpsc::Receiver<TaggedResponse>,
) {
    let mut assembler = FrameAssembler::default();
    super::spawn_frame_stream(runtime, responses, move |response| {
        assembler.push(response)
    });
}

pub(crate) async fn run(
    channel: ProxyDataChannel,
    ready_signal: ProxyReadySignal,
    start_replay_request: Request,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (player_id, upstream_url, publisher) = channel.into_replay_parts();
    let upstream = bridge::connect_upstream(&upstream_url).await?;
    let (mut writer, mut reader) = upstream.split();
    ready_signal.signal_ready();

    println!("[{player_id}] Starting replay");
    start_replay(
        player_id,
        &publisher,
        &mut writer,
        &mut reader,
        start_replay_request,
    )
    .await?;
    publish_game_info(player_id, &publisher, &mut writer, &mut reader).await?;
    replay_observations(player_id, &publisher, &mut writer, &mut reader).await
}

async fn start_replay(
    player_id: PlayerId,
    publisher: &ProxyPublisher,
    writer: &mut UpstreamWriter,
    reader: &mut UpstreamReader,
    request: Request,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let response = send_request(writer, reader, request, "start_replay").await?;
    if !response.has_start_replay() {
        return Err("SC2 returned an unexpected response to start_replay".into());
    }

    let start_replay = response.get_start_replay();
    if start_replay.has_error() {
        let error = format!(
            "StartReplay failed: {:?} - {}",
            start_replay.get_error(),
            start_replay.get_error_details()
        );
        publisher.publish(player_id, response).await;
        return Err(error.into());
    }
    publisher.publish(player_id, response).await;
    Ok(())
}

async fn publish_game_info(
    player_id: PlayerId,
    publisher: &ProxyPublisher,
    writer: &mut UpstreamWriter,
    reader: &mut UpstreamReader,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut request = Request::new();
    request.mut_game_info();
    let response = send_request(writer, reader, request, "game_info").await?;
    if !response.has_game_info() {
        return Err("SC2 returned an unexpected response to replay game_info".into());
    }
    publisher.publish(player_id, response).await;
    Ok(())
}

async fn replay_observations(
    player_id: PlayerId,
    publisher: &ProxyPublisher,
    writer: &mut UpstreamWriter,
    reader: &mut UpstreamReader,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    loop {
        let mut request = Request::new();
        request.mut_observation().set_disable_fog(true);
        let response = send_request(writer, reader, request, "observation").await?;
        if !response.has_observation() {
            return Err("SC2 returned an unexpected response to replay observation".into());
        }
        let ended = response
            .get_observation()
            .player_result
            .iter()
            .any(|result| result.has_result());
        publisher.publish(player_id, response).await;

        if ended {
            publish_replay_export(player_id, publisher, writer, reader).await;
            return Ok(());
        }

        let mut request = Request::new();
        request.mut_step().set_count(1);
        let response = send_request(writer, reader, request, "step").await?;
        if !response.has_step() {
            return Err("SC2 returned an unexpected response to replay step".into());
        }
        publisher.publish(player_id, response).await;
        tokio::time::sleep(std::time::Duration::from_millis(45)).await;
    }
}

async fn publish_replay_export(
    player_id: PlayerId,
    publisher: &ProxyPublisher,
    writer: &mut UpstreamWriter,
    reader: &mut UpstreamReader,
) {
    match bridge::save_replay(writer, reader).await {
        Ok(response) => publisher.publish(player_id, response).await,
        Err(error) => {
            eprintln!("[{player_id}] Could not prepare replay export: {error}");
            let mut response = Response::new();
            response.mut_save_replay();
            publisher.publish(player_id, response).await;
        }
    }
}

async fn send_request(
    writer: &mut UpstreamWriter,
    reader: &mut UpstreamReader,
    request: Request,
    name: &str,
) -> Result<Response, Box<dyn std::error::Error + Send + Sync>> {
    let request = request
        .write_to_bytes()
        .map_err(|error| format!("Protobuf encode {name}: {error}"))?;
    let response_bytes = bridge::roundtrip(writer, reader, request).await?;
    bridge::try_parse_response(&response_bytes)
        .ok_or_else(|| format!("Failed to parse replay {name} response").into())
}
