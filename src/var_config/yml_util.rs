use rand::Rng;
use std::fs::File;
use std::io::{Read, Write};

use crate::var_config::def_config::Config;

use md5;

pub fn generate_random_key(length: usize) -> String {
    let mut rng = rand::rng();
    let characters: Vec<char> = "abcdefghijklmnopqrstuvwxyz0123456789".chars().collect();
    let key: String = (0..length)
        .map(|_| {
            let idx = rng.random_range(0..characters.len());
            characters[idx]
        })
        .collect();
    key
}

pub fn write_config_to_yml(
    config: &Config,
    file_path: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let yaml_string = serde_yaml::to_string(config)?;
    let mut file = File::create(file_path)?;
    file.write_all(yaml_string.as_bytes())?;
    Ok(())
}
pub fn read_yml(file_path: &str) -> Result<Config, Box<dyn std::error::Error>> {
    let mut file = File::open(file_path)?;
    let mut contents = String::new();
    file.read_to_string(&mut contents)?;
    let config: Config = serde_yaml::from_str(&contents)?;
    Ok(config)
}

pub fn decrypt_name_t(name_text: String, t: String) -> String {
    let name_key = md5::compute(name_text);
    let iv = format!("{:x}", name_key) + &t;
    let hashed = md5::compute(iv);
    format!("{:x}", hashed)
}
