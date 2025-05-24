use actix_web::{App, HttpResponse, HttpServer, Responder, get, web};
use futures::StreamExt;
use libp2p::{
    Multiaddr, PeerId, SwarmBuilder, identify, identity, kad, mdns, noise, ping,
    swarm::{NetworkBehaviour, SwarmEvent},
    tcp, yamux,
};
use serde::Serialize;
use std::{
    error::Error,
    sync::{Arc, Mutex},
    time::Duration, // Keep for kademlia_config or ping_config
};
use var_config::def_config::inti_config;
use yansi::Paint;

mod var_config;

// ... (P2pStatus, SharedP2pStatus, MyBehaviour, MyBehaviourEvent, get_status - 保持不变) ...
// 1. 定义共享状态，用于在 libp2p 任务和 Actix-Web 处理器之间共享信息
#[derive(Debug, Clone, Default, Serialize)]
struct P2pStatus {
    peer_id: String,
    listeners: Vec<String>,
    num_known_peers: usize,
    k_buckets_info: String, // For more detailed Kademlia info
}

// 使用 Arc<Mutex<>> 来安全地共享和修改 P2pStatus
type SharedP2pStatus = Arc<Mutex<P2pStatus>>;

// 2. 定义 libp2p 网络行为
#[derive(NetworkBehaviour)]
#[behaviour(to_swarm = "MyBehaviourEvent")]
struct MyBehaviour {
    identify: identify::Behaviour,
    kademlia: kad::Behaviour<kad::store::MemoryStore>,
    mdns: mdns::tokio::Behaviour,
}

// Enum for events emitted by MyBehaviour
#[derive(Debug)]
#[allow(clippy::large_enum_variant)]
enum MyBehaviourEvent {
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

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    env_logger::init();

    let shared_status: Arc<Mutex<P2pStatus>> = Arc::new(Mutex::new(P2pStatus::default()));

    // --- libp2p 设置 ---
    let mut swarm = SwarmBuilder::with_new_identity()
        .with_tokio()
        .with_tcp(
            tcp::Config::default().nodelay(true),
            noise::Config::new, // 直接传递函数
            yamux::Config::default,
        )?
        .with_behaviour(|key: &identity::Keypair| {
            let local_peer_id_from_builder = key.public().to_peer_id();
            println!(
                "PeerId from SwarmBuilder's key: {:?}",
                local_peer_id_from_builder
            );

            // Create Kademlia DHT 行为
            let store = kad::store::MemoryStore::new(local_peer_id_from_builder);
            let kademlia_config = kad::Config::default();
            // kademlia_config.set_query_timeout(Duration::from_secs(5 * 60));
            let kademlia =
                kad::Behaviour::with_config(local_peer_id_from_builder, store, kademlia_config);

            // Create Identify 行为
            let identify_config = identify::Config::new(
                "p2p-web-app/0.1.0".to_string(),
                key.public(), // Use public key from the builder's keypair
            )
            .with_agent_version(format!("my-p2p-node/{}", env!("CARGO_PKG_VERSION")));
            let identify = identify::Behaviour::new(identify_config);

            // // 创建 Ping 行为
            // let ping_config = ping::Config::new().with_interval(Duration::from_secs(8)); // Example: set interval
            // let ping = ping::Behaviour::new(ping_config);

            // 创建 MDNS 行为
            let mdns =
                mdns::tokio::Behaviour::new(mdns::Config::default(), key.public().to_peer_id())?;
            Ok(MyBehaviour {
                identify,
                kademlia,
                mdns,
            })
        })?
        .build();

    // --- 获取 PeerId 和更新共享状态 ---
    let local_peer_id = *swarm.local_peer_id();
    println!("Local peer id (from swarm): {:?}", local_peer_id);
    {
        let mut status = shared_status.lock().unwrap();
        status.peer_id = local_peer_id.to_string();
    }

    // 监听地址
    let listen_addr_tcp = "/ip4/0.0.0.0/tcp/0".parse::<Multiaddr>()?;
    swarm.listen_on(listen_addr_tcp.clone())?;

    // 如果有命令行参数，尝试连接到指定的节点
    if let Some(addr_str) = std::env::args().nth(1) {
        match addr_str.parse::<Multiaddr>() {
            Ok(remote_addr) => {
                println!("Attempting to dial: {}", remote_addr);
                if let Err(e) = swarm.dial(remote_addr.clone()) {
                    eprintln!("Failed to dial {}: {:?}", remote_addr, e);
                } else {
                    if let Some(peer_id) = remote_addr.iter().find_map(|protocol| match protocol {
                        libp2p::multiaddr::Protocol::P2p(hash) => {
                            PeerId::from_multihash(hash.into()).ok()
                        }
                        _ => None,
                    }) {
                        println!("Adding dialed peer {} to Kademlia routing table", peer_id);
                        swarm
                            .behaviour_mut()
                            .kademlia
                            .add_address(&peer_id, remote_addr);
                    }
                }
            }
            Err(e) => eprintln!("Error parsing multiaddress '{}': {}", addr_str, e),
        }
    }

    match swarm.behaviour_mut().kademlia.bootstrap() {
        Ok(_) => println!("Kademlia bootstrap initiated."),
        Err(e) => eprintln!("Kademlia bootstrap failed: {:?}", e),
    }

    let p2p_task_status = shared_status.clone();

    // --- libp2p 事件循环 (基本不变) ---
    tokio::spawn(async move {
        loop {
            tokio::select! {
                event = swarm.select_next_some() => {
                    match event {
                        SwarmEvent::NewListenAddr { address, .. } => {
                            println!("libp2p listening on: {}", address);
                            let mut status = p2p_task_status.lock().unwrap();
                            status.listeners.push(address.to_string());
                        }
                        SwarmEvent::Behaviour(MyBehaviourEvent::Identify(event)) => {
                            println!("[DEBUG IDENTIFY] Event: {:?}", event); // Log the whole event
                            if let identify::Event::Received { peer_id, info } = event {
                                println!("[DEBUG IDENTIFY] Received from peer: {peer_id} with agent: {}", info.agent_version);
                                for addr in info.listen_addrs {
                                    println!("[DEBUG IDENTIFY] Adding address {addr} for {peer_id} to Kademlia.");
                                    swarm.behaviour_mut().kademlia.add_address(&peer_id, addr);
                                }
                            }
                        }
                        SwarmEvent::Behaviour(MyBehaviourEvent::Kademlia(event)) => {
                            match event {
                                kad::Event::OutboundQueryProgressed { result, .. } => {
                                     match result {
                                        kad::QueryResult::Bootstrap(Ok(kad::BootstrapOk { peer, num_remaining, .. })) => {
                                            println!("Kademlia bootstrap with {}: {} peers remaining.", peer, num_remaining);
                                        }
                                        kad::QueryResult::Bootstrap(Err(kad::BootstrapError::Timeout {peer, ..})) => {
                                            eprintln!("Kademlia bootstrap timeout with peer: {}", peer);
                                        }
                                        kad::QueryResult::GetClosestPeers(Ok(ok)) => {
                                            for peer in ok.peers {
                                                println!("Kademlia found closer peer: {}", peer);
                                            }
                                        }
                                        kad::QueryResult::GetClosestPeers(Err(err)) => {
                                            eprintln!("Kademlia GetClosestPeers error: {:?}", err);
                                        }
                                        _ => {
                                            //println!("Kademlia other query result: {:?}", result)
                                        }
                                     }
                                }
                                kad::Event::RoutingUpdated { peer, is_new_peer, addresses,old_peer, .. } => {
                                    println!(
                                        "[DEBUG KAD] RoutingUpdated: peer={}, is_new={}, addresses={:?}, old_peer={:?}",
                                        peer, is_new_peer, addresses, old_peer
                                    );
                                    let mut status_data = p2p_task_status.lock().unwrap();
                                    let mut known_peers_in_kbuckets = 0;
                                    let mut k_buckets_str = String::new();
                                    println!("[DEBUG KAD] Iterating k-buckets after RoutingUpdated for peer {}:", peer);
                                    for bucket in swarm.behaviour_mut().kademlia.kbuckets() {
                                        for entry in bucket.iter() {
                                            known_peers_in_kbuckets += 1;
                                            println!("[DEBUG KAD]   K-bucket entry: Peer: {}, Status: {:?}", entry.node.key.preimage(), entry.status);
                                            k_buckets_str.push_str(&format!("  - Peer: {}, Status: {:?}\n", entry.node.key.preimage(), entry.status));
                                        }
                                    }
                                    status_data.num_known_peers = known_peers_in_kbuckets;
                                    status_data.k_buckets_info = format!("K-Buckets ({} total entries):\n{}", known_peers_in_kbuckets, k_buckets_str);
                                    println!("[DEBUG KAD] Shared status updated. num_known_peers: {}", status_data.num_known_peers);
                                }
                                kad::Event::InboundRequest { request } => {
                                    println!("Kademlia inbound request: {:?}", request);
                                }
                                _ => {}
                            }
                        }
                        SwarmEvent::Behaviour(MyBehaviourEvent::Mdns(mdns::Event::Discovered(list))) => {
                            for (peer_id, multiaddr) in list {
                                println!("[DEBUG MDNS] Discovered peer: {peer_id} at {multiaddr}");
                                // 添加到 Kademlia 路由表 (仍然重要)
                                swarm.behaviour_mut().kademlia.add_address(&peer_id, multiaddr.clone());
                                println!("[DEBUG MDNS] Attempted to add {peer_id} to Kademlia.");

                                // 主动尝试连接到这个新发现的节点
                                // 这样做可以更快地触发 Identify, Ping, 和 Kademlia 的连接验证
                                println!("[DEBUG MDNS] Actively dialing discovered peer: {multiaddr}");
                                if let Err(e) = swarm.dial(multiaddr.clone()) {
                                    println!("[DEBUG MDNS] Error dialing {multiaddr}: {e:?}");
                                }
                            }
                        }
                        SwarmEvent::Behaviour(MyBehaviourEvent::Mdns(mdns::Event::Expired(list))) => {
                            for (peer_id, _multiaddr) in list {
                                println!("mDNS discover peer has expired: {peer_id}");
                            }
                        }
                        SwarmEvent::ConnectionEstablished { peer_id, endpoint, num_established, concurrent_dial_errors, .. } => {
                            println!("[DEBUG CONN] Connection established with: {} on {:?}, num_established: {}, concurrent_errors: {:?}",
                                     peer_id, endpoint.get_remote_address(), num_established,concurrent_dial_errors);
                            swarm.behaviour_mut().kademlia.add_address(&peer_id, endpoint.get_remote_address().clone());
                            println!("[DEBUG CONN] Attempted to add connected peer {peer_id} to Kademlia.");
                        }
                        SwarmEvent::ConnectionClosed { peer_id, cause, .. } => {
                            println!("Connection closed with: {} (Reason: {:?})", peer_id, cause);
                        }
                        SwarmEvent::IncomingConnection { local_addr, send_back_addr, .. } => {
                            println!("Incoming connection from {} to {}", send_back_addr, local_addr);
                        }
                        SwarmEvent::IncomingConnectionError { local_addr, send_back_addr, error, .. } => {
                             eprintln!("Incoming connection error from {} to {}: {}", send_back_addr, local_addr, error);
                        }
                        SwarmEvent::OutgoingConnectionError { peer_id, error, .. } => {
                             eprintln!("Outgoing connection error to {:?}: {}", peer_id, error);
                        }
                        SwarmEvent::ExpiredListenAddr { address, .. } => {
                             println!("Expired listen address: {}", address);
                        }
                        SwarmEvent::ListenerClosed { addresses, reason, .. } => {
                             eprintln!("Listener closed for {:?}, reason: {:?}", addresses, reason);
                        }
                        SwarmEvent::ListenerError { error, .. } => { // listener_id might be removed in some versions
                             eprintln!("Listener error: {}", error);
                        }
                        _ => {}
                    }
                }
            }
        }
    });

    // --- Actix-Web 服务器 (不变) ---
    match inti_config() {
        Ok(config) => {
            let web_app_status = web::Data::new(shared_status.clone());
            println!(
                "Starting Actix-Web server on http://127.0.0.1:{:?}",
                config.http_port
            );
            HttpServer::new(move || {
                App::new()
                    .app_data(web_app_status.clone())
                    .service(get_status)
            })
            // .bind(("0.0.0.0", config.http_port))?
            // 随机端口
            .bind(("0.0.0.0", 0))?
            .run()
            .await?;

            println!(
                "{} {}",
                Paint::yellow("请求服务端-端口:"),
                Paint::green(&config.http_port.to_string())
            );
        }
        Err(err) => {
            println!("读取配置文件失败：{}", err);
            println!(
                "{}",
                Paint::yellow("新旧配置文件冲突，请备份原有数据并重新运行")
            );
        }
    }
    Ok(())
}

// 3. Actix-Web 处理器
#[get("/status")]
async fn get_status(data: web::Data<SharedP2pStatus>) -> impl Responder {
    let status = data.lock().unwrap().clone();
    HttpResponse::Ok().json(status)
}
