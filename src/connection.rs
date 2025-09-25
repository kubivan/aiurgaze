use std::time::Duration;
use sc2_proto::sc2api::{LocalMap, PlayerSetup, PlayerType, Request, Response};
use tokio::time::sleep;

use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

use std::{
    error::Error,
    net::{TcpListener, TcpStream},
};
use tungstenite::{connect, stream::MaybeTlsStream, WebSocket};

use protobuf::{Message, RepeatedField};
use tungstenite::Message::Binary;

use futures::{SinkExt, StreamExt};

pub use sc2_proto::sc2api::Request as SC2Request;
pub use sc2_proto::sc2api::Response as SC2Response;

pub(crate) type WS = WebSocket<MaybeTlsStream<TcpStream>>;
pub type SC2Result<T> = Result<T, Box<dyn Error>>;


pub async fn connect_until_success(url: &str) -> WS
{
    // Exponential-ish backoff with cap
    let mut delay = Duration::from_millis(200);
    let max_delay = Duration::from_secs(3);

    loop {
        match connect(url) {
            Ok((ws, _resp)) => return ws,
            Err(e) => {
                println!("connect_until_success: error {:?}", e);
                sleep(delay).await;
                delay = (delay * 2).min(max_delay);
            }
        }
    }
}
pub struct Connection {
    ws: WS
}

impl Connection {
    pub async fn connect(url: &str) -> Connection
    {
        Connection{ ws: connect_until_success(url).await }
    }

    pub async fn send(&mut self, req : SC2Request) -> SC2Response
    {
        let msg = req.write_to_bytes().unwrap();
        self.ws.write_message(Binary(bytes::Bytes::from(msg)));
        let msg = self.ws.read_message().unwrap();

        let mut res = Response::new();
        res.merge_from_bytes(msg.into_data().iter().as_slice());

        println!("GOT RESPONSE: {:?}", res);
        res
    }

}