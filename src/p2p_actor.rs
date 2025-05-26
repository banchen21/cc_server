use actix::{Actor, Context};
use futures::StreamExt;
use libp2p::{
    Multiaddr, StreamProtocol, Swarm, SwarmBuilder,
    bytes::BufMut,
    identify, identity, kad, mdns, noise,
    swarm::{NetworkBehaviour, SwarmEvent},
    tcp, yamux,
};
use log::{error, info};
use serde::Serialize;
use std::{
    error::Error,
    num::NonZeroUsize,
    ops::Add,
    time::{Duration, Instant},
};
use tokio::sync::{mpsc, oneshot};
use yansi::Paint;

use crate::var_config::def_config::Config; // Added for actor communication

// 1. P2pStatus struct (remains the same)
#[derive(Debug, Clone, Default, Serialize)]
pub struct P2pStatus {
    pub peer_id: String,
    pub listeners: Vec<String>,
    pub num_known_peers: usize,
    pub k_buckets_info: String,
}

// No longer needed: type SharedP2pStatus = Arc<Mutex<P2pStatus>>;

// 2. Libp2p Network Behaviour (remains the same)
#[derive(NetworkBehaviour)]
#[behaviour(to_swarm = "MyBehaviourEvent")]
pub struct MyBehaviour {
    pub identify: identify::Behaviour,
    pub kademlia: kad::Behaviour<kad::store::MemoryStore>,
    pub mdns: mdns::tokio::Behaviour,
}

#[derive(Debug)]
#[allow(clippy::large_enum_variant)]
pub enum MyBehaviourEvent {
    Identify(identify::Event),
    Kademlia(kad::Event),
    Mdns(mdns::Event),
}

impl From<identify::Event> for MyBehaviourEvent {
    fn from(event: identify::Event) -> Self {
        MyBehaviourEvent::Identify(event)
    }
}

impl From<kad::Event> for MyBehaviourEvent {
    fn from(event: kad::Event) -> Self {
        MyBehaviourEvent::Kademlia(event)
    }
}

impl From<mdns::Event> for MyBehaviourEvent {
    fn from(event: mdns::Event) -> Self {
        MyBehaviourEvent::Mdns(event)
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct P2pRegisterUser(pub(crate) String);

// 3. Define Actor Messages
#[derive(Debug)]
pub enum P2pActorCommand {
    GetStatus(oneshot::Sender<P2pStatus>), // Sender for the response
    // 用户注册
    RegisterUser(oneshot::Receiver<P2pRegisterUser>),
}

// Type alias for the command sender channel
pub type P2pActorCommandSender = mpsc::Sender<P2pActorCommand>;

async fn handle_behaviour_event(
    event: SwarmEvent<MyBehaviourEvent>,
    status: &mut P2pStatus,
    swarm: &mut Swarm<MyBehaviour>,
) {
    match event {
        SwarmEvent::NewListenAddr { address, .. } => {
            info!("libp2p listening on: {}", address);
            status.listeners.push(address.to_string());
        }
        SwarmEvent::Behaviour(MyBehaviourEvent::Identify(event)) => {
            info!("[DEBUG IDENTIFY] Event: {:?}", event);
            if let identify::Event::Received { peer_id, info, .. } = event {
                info!(
                    "[DEBUG IDENTIFY] Received from peer: {peer_id} with agent: {}",
                    info.agent_version
                );
                for addr in info.listen_addrs {
                    info!("[DEBUG IDENTIFY] Adding address {addr} for {peer_id} to Kademlia.");
                    swarm.behaviour_mut().kademlia.add_address(&peer_id, addr);
                }
            }
        }
        SwarmEvent::Behaviour(MyBehaviourEvent::Kademlia(event)) => {
            match event {
                kad::Event::OutboundQueryProgressed { result, .. } => {
                    match result {
                        kad::QueryResult::Bootstrap(Ok(kad::BootstrapOk {
                            peer,
                            num_remaining,
                            ..
                        })) => {
                            info!(
                                "Kademlia bootstrap with {}: {} peers remaining.",
                                peer, num_remaining
                            );
                        }
                        kad::QueryResult::Bootstrap(Err(kad::BootstrapError::Timeout {
                            peer,
                            ..
                        })) => {
                            info!("Kademlia bootstrap timeout with peer: {}", peer);
                        }
                        kad::QueryResult::GetClosestPeers(Ok(ok)) => {
                            for peer in ok.peers {
                                info!("Kademlia found closer peer: {:?}", peer);
                            }
                        }
                        kad::QueryResult::GetClosestPeers(Err(err)) => {
                            info!("Kademlia GetClosestPeers error: {:?}", err);
                        }
                        _ => {
                            //info!("Kademlia other query result: {:?}", result)
                        }
                    }
                }
                kad::Event::RoutingUpdated {
                    peer,
                    is_new_peer,
                    addresses,
                    old_peer,
                    ..
                } => {
                    info!(
                        "[DEBUG KAD] RoutingUpdated: peer={}, is_new={}, addresses={:?}, old_peer={:?}",
                        peer, is_new_peer, addresses, old_peer
                    );
                    // Directly update actor's status
                    let mut known_peers_in_kbuckets = 0;
                    let mut k_buckets_str = String::new();
                    info!(
                        "[DEBUG KAD] Iterating k-buckets after RoutingUpdated for peer {}:",
                        peer
                    );
                    for bucket in swarm.behaviour_mut().kademlia.kbuckets() {
                        for entry in bucket.iter() {
                            known_peers_in_kbuckets += 1;
                            info!(
                                "[DEBUG KAD]   K-bucket entry: Peer: {}, Status: {:?}",
                                entry.node.key.preimage(),
                                entry.status
                            );
                            k_buckets_str.push_str(&format!(
                                "  - Peer: {}, Status: {:?}\n",
                                entry.node.key.preimage(),
                                entry.status
                            ));
                        }
                    }
                    status.num_known_peers = known_peers_in_kbuckets;
                    status.k_buckets_info = format!(
                        "K-Buckets ({} total entries):\n{}",
                        known_peers_in_kbuckets, k_buckets_str
                    );
                    info!(
                        "[DEBUG KAD] Actor status updated. num_known_peers: {}",
                        status.num_known_peers
                    );
                }
                kad::Event::InboundRequest { request } => {
                    info!("Kademlia inbound request: {:?}", request);
                }
                _ => {}
            }
        }
        SwarmEvent::Behaviour(MyBehaviourEvent::Mdns(mdns::Event::Discovered(list))) => {
            for (peer_id, multiaddr) in list {
                info!("[DEBUG MDNS] Discovered peer: {peer_id} at {multiaddr}");
                swarm
                    .behaviour_mut()
                    .kademlia
                    .add_address(&peer_id, multiaddr.clone());
                info!("[DEBUG MDNS] Attempted to add {peer_id} to Kademlia.");
                info!("[DEBUG MDNS] Actively dialing discovered peer: {multiaddr}");
                if let Err(e) = swarm.dial(multiaddr.clone()) {
                    info!("[DEBUG MDNS] Error dialing {multiaddr}: {e:?}");
                }
            }
        }
        SwarmEvent::Behaviour(MyBehaviourEvent::Mdns(mdns::Event::Expired(list))) => {
            for (peer_id, _multiaddr) in list {
                info!("mDNS discover peer has expired: {peer_id}");
            }
        }
        SwarmEvent::ConnectionEstablished {
            peer_id,
            endpoint,
            num_established,
            concurrent_dial_errors,
            ..
        } => {
            info!(
                "[DEBUG CONN] Connection established with: {} on {:?}, num_established: {}, concurrent_errors: {:?}",
                peer_id,
                endpoint.get_remote_address(),
                num_established,
                concurrent_dial_errors
            );
            swarm
                .behaviour_mut()
                .kademlia
                .add_address(&peer_id, endpoint.get_remote_address().clone());
            info!("[DEBUG CONN] Attempted to add connected peer {peer_id} to Kademlia.");
        }
        SwarmEvent::ConnectionClosed { peer_id, cause, .. } => {
            info!("Connection closed with: {} (Reason: {:?})", peer_id, cause);
        }
        SwarmEvent::IncomingConnection {
            local_addr,
            send_back_addr,
            ..
        } => {
            info!(
                "Incoming connection from {} to {}",
                send_back_addr, local_addr
            );
        }
        SwarmEvent::IncomingConnectionError {
            local_addr,
            send_back_addr,
            error,
            ..
        } => {
            info!(
                "Incoming connection error from {} to {}: {}",
                send_back_addr, local_addr, error
            );
        }
        SwarmEvent::OutgoingConnectionError { peer_id, error, .. } => {
            info!("Outgoing connection error to {:?}: {}", peer_id, error);
        }
        SwarmEvent::ExpiredListenAddr { address, .. } => {
            info!("Expired listen address: {}", address);
        }
        SwarmEvent::ListenerClosed {
            addresses, reason, ..
        } => {
            info!("Listener closed for {:?}, reason: {:?}", addresses, reason);
        }
        SwarmEvent::ListenerError { error, .. } => {
            info!("Listener error: {}", error);
        }
        _ => {}
    }
}

pub struct P2pActor {
    status: P2pStatus,
    command_receiver: mpsc::Receiver<P2pActorCommand>,
    key_pair: identity::Keypair,
    swarm: Swarm<MyBehaviour>,
    p2p_command_tx: mpsc::Sender<P2pActorCommand>,
}

impl Actor for P2pActor {
    type Context = Context<Self>;
}

impl P2pActor {
    pub async fn new(config: Config, addrs: Vec<Multiaddr>) -> tokio::task::JoinHandle<P2pActor> {
        let initial_p2p_status = P2pStatus::default();
        let (p2p_command_tx, p2p_command_rx) = mpsc::channel::<P2pActorCommand>(32); // Buffer size 32
        let local_key: identity::Keypair = identity::Keypair::generate_ed25519();

        let mut swarm: libp2p::Swarm<MyBehaviour> = Self::setswarm(local_key.clone()).await;

        // 尝试链接引导节点
        for boot_node in config.boot_nodes.iter() {
            let addr = boot_node.parse::<Multiaddr>();
            match addr {
                Ok(addr) => {
                    info!("Dialing boot node: {}", addr);
                    match swarm.dial(addr.clone()) {
                        Ok(_) => {
                            info!("Dialed boot node: {}", addr);
                        }
                        Err(err) => {
                            info!("Failed to dial boot node: {}", err);
                        }
                    };
                }
                Err(err) => {
                    info!("Failed to parse boot node address: {}", err);
                }
            }
        }

        match swarm.behaviour_mut().kademlia.bootstrap() {
            Ok(_) => info!("Kademlia bootstrap initiated."),
            Err(e) => info!("Kademlia bootstrap failed: {:?}", e),
        }

        let p2p_actor = Self {
            status: initial_p2p_status,
            command_receiver: p2p_command_rx,
            key_pair: local_key,
            swarm: swarm,
            p2p_command_tx,
        };
        tokio::spawn(p2p_actor.setswarm_start(addrs))
    }

    async fn setswarm(local_key: identity::Keypair) -> Swarm<MyBehaviour> {
        SwarmBuilder::with_existing_identity(local_key.clone())
            .with_tokio()
            .with_tcp(
                tcp::Config::default().nodelay(true),
                noise::Config::new,
                yamux::Config::default,
            )
            .unwrap()
            .with_behaviour(|key: &identity::Keypair| {
                let local_peer_id_from_builder = key.public().to_peer_id();
                info!(
                    "PeerId from SwarmBuilder's key: {:?}",
                    local_peer_id_from_builder
                );

                const IPFS_PROTO_NAME: StreamProtocol = StreamProtocol::new("/ipfs/kad/1.0.0");
                let mut kademlia_config = kad::Config::new(IPFS_PROTO_NAME);
                kademlia_config.set_query_timeout(Duration::from_secs(5 * 60));

                let store = kad::store::MemoryStore::new(local_peer_id_from_builder);
                let kademlia =
                    kad::Behaviour::with_config(local_peer_id_from_builder, store, kademlia_config);

                let identify_config =
                    identify::Config::new("p2p-web-app/0.1.0".to_string(), key.public())
                        .with_agent_version(format!("my-p2p-node/{}", env!("CARGO_PKG_VERSION")));
                let identify = identify::Behaviour::new(identify_config);

                let mdns = mdns::tokio::Behaviour::new(
                    mdns::Config::default(),
                    key.public().to_peer_id(),
                )?;
                Ok(MyBehaviour {
                    identify,
                    kademlia,
                    mdns,
                })
            })
            .unwrap()
            .build()
    }

    async fn setswarm_start(mut self, addrs: Vec<Multiaddr>) -> Self {
        for addr in addrs {
            if let Err(e) = self.swarm.listen_on(addr.clone()) {
                error!("监听地址失败: {}", e);
            }
        }
        tokio::select! {
            // Handle Swarm Events
            event = self.swarm.select_next_some() => {
                handle_behaviour_event(event, &mut self.status, &mut self.swarm).await;
            },
            Some(command) = self.command_receiver.recv() => {
                self.command_receiver(command).await;
            }
        }
        self
    }

    async fn command_receiver(&mut self, command: P2pActorCommand) {
        match command {
            P2pActorCommand::GetStatus(responder) => {
                info!("P2P Actor: Received GetStatus command");
                // Send a clone of the current status back to the requester
                if let Err(e) = responder.send(self.status.clone()) {
                    info!("P2P Actor: Failed to send status response: {:?}", e);
                }
            }
            P2pActorCommand::RegisterUser(response_rx) => {
                info!("P2P Actor: Received RegisterUser command");
                let mut pk_record_key = vec![];

                match response_rx.await {
                    Ok(user_info) => {
                        pk_record_key.put_slice("/pk/".as_bytes());
                        pk_record_key.put_slice(user_info.0.as_bytes());
                        let mut pk_record = kad::Record::new(
                            pk_record_key,
                            self.key_pair.public().encode_protobuf(),
                        );
                        pk_record.publisher = Some(*self.swarm.local_peer_id());
                        pk_record.expires = Some(Instant::now().add(Duration::from_secs(60)));
                        match self
                            .swarm
                            .behaviour_mut()
                            .kademlia
                            .put_record(pk_record, kad::Quorum::N(NonZeroUsize::new(3).unwrap()))
                        {
                            Ok(_) => {
                                println!("{}{}", "节点注册成功".green(), user_info.0);
                            }
                            Err(e) => {
                                println!("{}", "节点注册失败".red());
                                println!("{}", e);
                            }
                        }
                    }
                    Err(e) => {
                        error!("P2P Actor: Failed to receive user info: {}", e);
                    }
                };
            }
        }
    }
}
