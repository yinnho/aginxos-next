//! `dup config` - show or set configuration (`~/.dupconfig`).
//!
//! Keys: `remote.url` (opencarrier base URL), `remote.api_key` (Bearer key).
//! Env overrides `OPENCARRIER_URL` / `OPENCARRIER_API_KEY` take precedence.

use anyhow::Result;

use crate::config::DupConfig;

const SUPPORTED: &[&str] = &["remote.url", "remote.api_key"];

pub fn run(args: &[String]) -> Result<()> {
    if args.is_empty() {
        return show();
    }
    let key = &args[0];
    if !SUPPORTED.contains(&key.as_str()) {
        println!("未知配置项: {}", key);
        println!("可配置: remote.url, remote.api_key");
        return Ok(());
    }
    if args.len() > 1 {
        set(key, &args[1])?;
    } else {
        get(key)?;
    }
    Ok(())
}

fn show() -> Result<()> {
    let config = DupConfig::load_global()?;
    let url = config.resolve_url();
    let key_set = config.resolve_api_key().is_ok();
    println!("remote.url     = {}", url);
    println!("remote.api_key = {}", if key_set { "***" } else { "(未设置)" });
    println!("  (env: OPENCARRIER_URL / OPENCARRIER_API_KEY 优先)");
    Ok(())
}

fn get(key: &str) -> Result<()> {
    let config = DupConfig::load_global()?;
    match key {
        "remote.url" => println!("{}", config.resolve_url()),
        "remote.api_key" => println!(
            "{}",
            if config.resolve_api_key().is_ok() {
                "***"
            } else {
                "(未设置)"
            }
        ),
        _ => println!("未知: {}", key),
    }
    Ok(())
}

fn set(key: &str, value: &str) -> Result<()> {
    let mut config = DupConfig::load_global()?;
    config.set(key, value)?;
    config.save_global()?;
    match key {
        "remote.url" => println!("remote.url = {}", value),
        "remote.api_key" => println!("remote.api_key 已设置"),
        _ => {}
    }
    Ok(())
}
