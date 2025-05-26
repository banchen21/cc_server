use crate::var_config::yml_util;
use serde::{Deserialize, Serialize};

// 异步
use std::{
    error::Error,
    fs,
    sync::{Arc, Mutex},
    thread,
};

// 有色日志
use colored::Colorize;

// 启动配置文件结构体
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Config {
    pub(crate) ws_port: u32,
    pub(crate) ws_keymode: String,
    pub(crate) server_name: String,
    pub(crate) http_port: u16,
    // 其他节点
    pub(crate) boot_nodes: Vec<String>,
    pub(crate) ipfs_api_port: String, // New field
}

use super::yml_util::generate_random_key;

// 返回配置
pub fn inti_config() -> Result<Config, Box<dyn Error>> {
    let key = generate_random_key(16).to_owned();
    let file_path = "config.yml";
    let config = Config {
        ws_port: 20102,
        http_port: 20103,
        ws_keymode: "AES-128".to_owned(),
        server_name: key,
        boot_nodes: vec![],
        ipfs_api_port: "5001".to_string(), // Default IPFS API
    };
    match fs::metadata(&file_path) {
        Err(_) => {
            let text = "文件不存在, 开始写入".to_string();
            println!("{}", text.yellow());
            if let Err(err) = yml_util::write_config_to_yml(&config, &file_path) {
                println!("无法写入配置文件：{}", err);
            }
            panic!("请重启程序")
        }
        Ok(_) => {
            let text = "检测到配置文件存在".to_string();
            println!("{}", text.green());
        }
    }
    read_yml_to_str(&file_path)
}

// 读取配置文件
pub fn read_yml_to_str(file_path: &str) -> Result<Config, Box<dyn Error>> {
    let config = yml_util::read_yml(file_path)?;
    Ok(config)
}

// 定义一个结构体来存储经济体信息

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct EconomyInfo {
    pub(crate) economy_name: String,
    pub(crate) key: String,
}
