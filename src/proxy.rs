// src/proxy.rs
use std::net::SocketAddr;
use tokio::sync::mpsc;
use tonic::{transport::Server, Request, Response, Status};

pub mod sc2api {
    tonic::include_proto!("sc2api_protocol"); // Generated from SC2 API .proto files
    //tonic::include_proto!("sc2proxy");
}

use sc2api::{
    request::Request as Sc2Request, 
    response
};
use crate::proxy::sc2api::sc2_proxy_server;

/// Messages we forward to the Bevy render system
#[derive(Debug, Clone)]
pub enum ProxyEvent {
    GameState(sc2api::Response),
    BotStep(Sc2Request),
}

/// Our proxy gRPC service
#[derive(Debug)]
pub struct ProxyService {
    tx: mpsc::UnboundedSender<ProxyEvent>,
}

#[tonic::async_trait]
impl sc2api::sc2_proxy_server::Sc2Proxy for ProxyService {
    async fn send_join_request(
        &self,
        request: tonic::Request<sc2api::Request>,
    ) -> std::result::Result<tonic::Response<sc2api::Response>, tonic::Status> {
        let req = request.into_inner();

        // TODO: forward join request to SC2
        // let response = tonic::Response::new(
        //     sc2api::response::Response::CreateGame(sc2api::ResponseCreateGame { error: Some(0), error_details: None })
        // );
        let response = tonic::Response::new(sc2api::Response {
            id: None,
            error: vec![],
            status: None,
            response: Some(sc2api::response::Response::CreateGame(
                sc2api::ResponseCreateGame { error: Some(0), error_details: None }
            )),
        });

        // let _ = self.tx.send(ProxyEvent::BotStep(req)).await;

        Ok(response)
    }

    async fn send_step_request(
        &self,
        request: tonic::Request<sc2api::Request>,
    ) -> std::result::Result<tonic::Response<sc2api::Response>, tonic::Status> {
        let req = request.into_inner();

        // TODO: forward step request to SC2
        // let response = tonic::Response::new(
        //     sc2api::response::Response::CreateGame(sc2api::ResponseCreateGame { error: Some(0), error_details: None })
        // );

        let response = tonic::Response::new(sc2api::Response {
            id: None,
            error: vec![],
            status: None,
            response: Some(sc2api::response::Response::CreateGame(
                sc2api::ResponseCreateGame { error: Some(0), error_details: None }
            )),
            // ... existing code ...
        });

        // let _ = self.tx.send(ProxyEvent::BotStep(req)).await;

        Ok(response)
    }
}

/// Launches the proxy gRPC server
pub async fn start_proxy(
    tx: mpsc::UnboundedSender<ProxyEvent>,
    port: u16
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let addr: SocketAddr = format!("127.0.0.1:{}", port).parse()?;
    let svc = ProxyService { tx };

    println!("Proxy server listening on {}", addr);

    Server::builder()
        .add_service(sc2api::sc2_proxy_server::Sc2ProxyServer::new(svc))
        .serve(addr)
        .await?;

    Ok(())
}
