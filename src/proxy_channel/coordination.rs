use protobuf::RepeatedField;
use sc2_proto::sc2api::{PortSet, Request, Request_oneof_request};
use tokio::sync::watch;

/// Shared signal indicating how many proxy listeners are ready.
/// Bots should wait for this before connecting.
#[derive(Debug, Clone, Default)]
pub struct ProxyReadySignal {
    ready_count: watch::Sender<u8>,
    expected_count: u8,
}

impl ProxyReadySignal {
    pub fn new(expected_count: u8) -> Self {
        let (ready_count, _) = watch::channel(0);
        Self {
            ready_count,
            expected_count,
        }
    }

    pub fn signal_ready(&self) {
        self.ready_count
            .send_modify(|count| *count = count.saturating_add(1));
        println!(
            "[ProxyReadySignal] Proxy ready ({}/{})",
            *self.ready_count.borrow(),
            self.expected_count
        );
    }

    pub fn is_ready(&self) -> bool {
        *self.ready_count.borrow() >= self.expected_count
    }

    pub fn has_count(&self, count: u8) -> bool {
        *self.ready_count.borrow() >= count
    }

    pub async fn wait_ready(&self) {
        self.wait_for_count(self.expected_count).await;
    }

    pub async fn wait_for_count(&self, count: u8) {
        let mut ready_count = self.ready_count.subscribe();
        loop {
            if *ready_count.borrow_and_update() >= count {
                return;
            }
            if ready_count.changed().await.is_err() {
                return;
            }
        }
    }
}

/// One-shot signal: host fires after CreateGame succeeds.
/// Guest awaits before sending JoinGame.
#[derive(Debug, Clone)]
pub struct CreateGameSignal {
    done: watch::Sender<bool>,
}

impl CreateGameSignal {
    pub fn new() -> Self {
        let (done, _) = watch::channel(false);
        Self { done }
    }

    pub fn signal(&self) {
        self.done.send_replace(true);
    }

    pub async fn wait(&self) {
        let mut done = self.done.subscribe();
        loop {
            if *done.borrow_and_update() {
                return;
            }
            if done.changed().await.is_err() {
                return;
            }
        }
    }
}

/// Barrier to synchronize JoinGame responses across proxies.
#[derive(Debug, Clone)]
pub struct JoinResponseBarrier {
    expected: u8,
    count: watch::Sender<u8>,
}

impl JoinResponseBarrier {
    pub fn new(expected: u8) -> Self {
        let (count, _) = watch::channel(0);
        Self { expected, count }
    }

    pub fn mark_joined(&self) {
        self.count
            .send_modify(|count| *count = count.saturating_add(1));
    }

    pub async fn wait_ready(&self) {
        let mut count = self.count.subscribe();
        loop {
            if *count.borrow_and_update() >= self.expected {
                return;
            }
            if count.changed().await.is_err() {
                return;
            }
        }
    }
}

/// Port configuration for SC2 multiplayer internal communication.
///
/// These are NOT the WebSocket proxy ports — they are TCP ports that SC2
/// opens internally for game synchronisation between participants.
/// Both players must send the same server_ports and client_ports in their
/// JoinGame requests.
#[derive(Debug, Clone)]
pub struct MultiplayerPorts {
    pub server_game_port: i32,
    pub server_base_port: i32,
    /// One (game_port, base_port) pair per participant.
    pub client_ports: Vec<(i32, i32)>,
}

impl MultiplayerPorts {
    /// Derive ports automatically from a base port.
    pub fn from_base(base: u16, num_players: u8) -> Self {
        let b = base as i32;
        let mut clients = Vec::new();
        for i in 0..num_players {
            let offset = 2 + (i as i32) * 2;
            clients.push((b + offset, b + offset + 1));
        }
        Self {
            server_game_port: b,
            server_base_port: b + 1,
            client_ports: clients,
        }
    }

    fn server_port_set(&self) -> PortSet {
        let mut port_set = PortSet::new();
        port_set.set_game_port(self.server_game_port);
        port_set.set_base_port(self.server_base_port);
        port_set
    }

    fn client_port_sets(&self) -> RepeatedField<PortSet> {
        let sets = self
            .client_ports
            .iter()
            .map(|&(game_port, base_port)| {
                let mut port_set = PortSet::new();
                port_set.set_game_port(game_port);
                port_set.set_base_port(base_port);
                port_set
            })
            .collect();
        RepeatedField::from_vec(sets)
    }

    pub(super) fn inject_into(&self, request: &mut Request) {
        if let Some(Request_oneof_request::join_game(ref mut join_game)) = request.request {
            join_game.set_server_ports(self.server_port_set());
            join_game.set_client_ports(self.client_port_sets());
            println!(
                "[MultiplayerPorts] Injected server=({},{}) clients={:?}",
                self.server_game_port, self.server_base_port, self.client_ports
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multiplayer_ports_inject_server_and_client_pairs() {
        let ports = MultiplayerPorts::from_base(5002, 2);
        let mut request = Request::new();
        request.mut_join_game();

        ports.inject_into(&mut request);

        let Some(Request_oneof_request::join_game(join_game)) = request.request.as_ref() else {
            panic!("JoinGame request should remain present");
        };
        assert_eq!(join_game.get_server_ports().get_game_port(), 5002);
        assert_eq!(join_game.get_server_ports().get_base_port(), 5003);
        assert_eq!(join_game.get_client_ports().len(), 2);
        assert_eq!(join_game.get_client_ports()[0].get_game_port(), 5004);
        assert_eq!(join_game.get_client_ports()[0].get_base_port(), 5005);
        assert_eq!(join_game.get_client_ports()[1].get_game_port(), 5006);
        assert_eq!(join_game.get_client_ports()[1].get_base_port(), 5007);
    }
}
