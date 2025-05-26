use actix::Actor;
use actix_web::{App, HttpServer, web};
use libp2p::{
    Multiaddr, StreamProtocol, SwarmBuilder, identify, identity, kad, mdns, noise, tcp, yamux,
};
use log::info;
use p2p_actor::{MyBehaviour, P2pActor, P2pActorCommand, P2pStatus};
use std::{
    error::Error,
    time::Duration,
    // Removed: sync::{Arc, Mutex},
};
use tokio::sync::mpsc; // Added for actor communication
use var_config::def_config::inti_config;
use web_server::{get_status, register_info};
use yansi::Paint;

mod p2p_actor;
mod var_config;
mod web_server;
#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    env_logger::init();

    // 初始化配置文件
    let config = match inti_config() {
        Ok(config) => config,
        Err(err) => {
            info!("读取配置文件失败：{}", err);
            info!(
                "{}",
                Paint::yellow("新旧配置文件冲突，请备份原有数据并重新运行")
            );
            panic!("{}", Paint::red("程序已退出"));
        }
    };
    let mut adds = Vec::new();
    let listen_addr_tcp = "/ip4/0.0.0.0/tcp/0".parse::<Multiaddr>()?;
    adds.push(listen_addr_tcp);
    let p2p_actor = P2pActor::new(config.clone(), adds.clone()).await;
    // --- Actix-Web Server ---
    // Pass the command sender to Actix-Web app data
    let p2p_command_tx_for_web = web::Data::new(p2p_actor);
    info!(
        "Starting Actix-Web server on http://127.0.0.1:{:?}",
        config.http_port
    );
    HttpServer::new(move || {
        App::new()
            .app_data(p2p_command_tx_for_web.clone()) // Clone for each worker
            .service(get_status)
            .service(register_info)
    })
    .bind(("0.0.0.0", config.http_port))? // Using configured port
    // .bind(("0.0.0.0", 0))? // Or random port
    .run()
    .await?;

    Ok(())
}
