use crate::p2p_actor::{P2pActorCommand, P2pActorCommandSender, P2pRegisterUser};
use actix_web::{HttpResponse, Responder, get, post, web};
use log::error;
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

// Actix-Web Handler - modified to use actor messages
#[get("/status")]
async fn get_status(p2p_cmd_tx: web::Data<P2pActorCommandSender>) -> impl Responder {
    // Create a oneshot channel for the response
    let (response_tx, response_rx) = oneshot::channel();

    // Send the GetStatus command to the P2P actor
    let cmd = P2pActorCommand::GetStatus(response_tx);
    if let Err(e) = p2p_cmd_tx.send(cmd).await {
        error!("Failed to send GetStatus command to P2P actor: {}", e);
        return HttpResponse::InternalServerError().body("P2P actor communication error");
    }

    // Await the response from the actor
    match response_rx.await {
        Ok(status) => HttpResponse::Ok().json(status),
        Err(e) => {
            error!("Failed to receive status from P2P actor: {}", e);
            HttpResponse::InternalServerError().body("Failed to get status from P2P actor")
        }
    }
}

#[derive(Serialize, Deserialize, Debug)]
struct RegisterInfo {
    key: String,
}

// 用户注册
#[post("/register")]
pub async fn register_info(
    p2p_cmd_tx: web::Data<P2pActorCommandSender>,
    query: web::Query<RegisterInfo>,
) -> impl Responder {
    let register_info = RegisterInfo {
        key: query.key.clone(),
    };
    // Create a oneshot channel for the response
    let (response_tx, response_rx) = oneshot::channel();

    let cmd = P2pActorCommand::RegisterUser(response_rx);
    if let Err(e) = p2p_cmd_tx.send(cmd).await {
        error!("Failed to send GetStatus command to P2P actor: {}", e);
        return HttpResponse::InternalServerError().body("P2P actor communication error");
    }
    match response_tx.send(P2pRegisterUser(register_info.key)) {
        Ok(status) => HttpResponse::Ok().json(status),
        Err(e) => {
            error!("Failed to receive status from P2P actor: {:?}", e);
            HttpResponse::InternalServerError().body("Failed to get status from P2P actor")
        }
    }
}

// 用户上传文件
#[post("/{key}/upload")]
pub async fn upload_file(payload: web::Payload) -> impl Responder {
    HttpResponse::Ok()
}

// 用户下载文件
#[get("/download")]
pub async fn download_file(payload: web::Payload) -> impl Responder {
    HttpResponse::Ok()
}

// ws连接
