//! gRPC proxy: accepts bot RPCs, forwards to SC2, and ships compact snapshots to the renderer.


#[derive(Clone)]
struct ProxySvc {
    /// Client → SC2
    sc2: Sc2ApiClient<tonic::transport::Channel>,
    /// Pipe to Bevy renderer
    tx: Sender<ProxyToRenderMsg>,
}


pub async fn run_proxy(
    bot_listen_addr: &str,
    sc2_addr: &str,
    tx_state: Sender<ProxyToRenderMsg>,
) -> Result<(), ProxyError> {
    // 1) Connect to SC2
    let mut sc2 = Sc2ApiClient::connect(sc2_addr.to_string()).await?;


    // 2) Send CreateGameRequest at startup (fill in map/rules as you prefer)
    let create = CreateGameRequest {
        // TODO: map settings (ladder map, local map, realtime=false, etc.)
        ..Default::default()
    };
    let _ = sc2.create_game(Request { request: Some(sc2::s2clientprotocol::sc2api::request::Request::CreateGame(create)) }).await?;


    // 3) Start gRPC server for the bot → we proxy to SC2
    let svc = ProxySvc { sc2, tx: tx_state.clone() };


    // Spawn server; note: we move `svc`, but we’ll need to recreate client handles per-call.
    let addr = bot_listen_addr.parse().unwrap();


    Server::builder()
        .add_service(Sc2ApiServer::new(svc))
        .serve(addr)
        .await?;


    Ok(())
}


#[tonic::async_trait]
impl Sc2Api for ProxySvc {
    async fn ping(&self, request: Request<sc2::s2clientprotocol::sc2api::PingRequest>) -> Result<Response<sc2::s2clientprotocol::sc2api::PingResponse>, Status> {
        // Simple transparent forward (optional)
        let mut sc2 = self.sc2.clone();
        sc2.ping(request).await
    }


    async fn join_game(&self, req: Request<JoinGameRequest>) -> Result<Response<Sc2Response>, Status> {
        // Capture bot’s JoinGameRequest, augment if needed (ports, options)
        let mut join = req.into_inner();
        // TODO: override options, feature layers, raw enabled, etc.


        let mut sc2 = self.sc2.clone();
        let resp = sc2.join_game(Request{ request: Some(sc2::s2clientprotocol::sc2api::request::Request::JoinGame(join))}).await?;
        Ok(resp)
    }


    async fn step(&self, req: Request<StepRequest>) -> Result<Response<Sc2Response>, Status> {
        // 1) Pull observation BEFORE forwarding bot’s step (or after, depending on your flow)
        let mut sc2 = self.sc2.clone();
        let obs_resp = sc2.observation(Request{ request: Some(sc2::s2clientprotocol::sc2api::request::Request::Observation(ObservationRequest{..Default::default()}))}).await?;


        // 2) Convert observation → compact render snapshot
        if let Some(rsp) = obs_resp.get_ref().response.as_ref() {
            if let sc2::s2clientprotocol::sc2api::response::Response::Observation(o) = rsp {
                // Terrain / grids from `StartRaw` are usually available via `o.observation.as_ref().and_then(|x| x.raw_data.as_ref())` for units,
                // and from `o.observation.as_ref().and_then(|x| x.raw_data.as_ref().and_then(|r| r.map_state.as_ref()))` or via the separate `GameInfo`.
                // For the skeleton, we push only units with positions and request layers later via GameInfo.


                // Units (dummy icon position)
                if let Some(obs) = o.observation.as_ref() {
                    if let Some(raw) = obs.raw_data.as_ref() {
                        let units = raw.units.iter().map(|u| UnitMarker{
                            x: u.pos.as_ref().map(|p| p.x).unwrap_or_default(),
                            y: u.pos.as_ref().map(|p| p.y).unwrap_or_default(),
                            owner: u.owner as u32,
                        }).collect::<Vec<_>>();
                        let _ = self.tx.send(ProxyToRenderMsg::Units(units));
                    }
                }
            }
        }


        // 3) Forward bot’s StepRequest transparently
        let resp = self.sc2.clone().step(req).await?;
        Ok(resp)
    }


    // Transparent forwarders for other calls (query, action, etc.)
    // Implement the minimal set your bot uses.
}
