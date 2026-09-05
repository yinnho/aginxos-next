//! config — /etc/aginx/gateway.toml + env 覆盖（N5⑤）。
//!
//! 覆盖序：**文件 < env**。文件故意**不含 id**——id 是设备身份，属
//! 设备状态（AGINX_GATEWAY_ID env 由 svc.d 单元的 env_file=/etc/aginx/env
//! 注入），随 state tar 换机存活；配置文件是随镜像走的形状参数。
//!
//! 解析是最小 toml-ish（`key = value` 行、`#` 注释、字符串/整数/布尔），
//! 与 aginx-svc 的 parse_tomlish 同款纪律——为三个标量引一个 toml
//! 解析器不值。未知键忽略（向前兼容），缺键保持缺省。

pub const DEFAULT_CONF: &str = "/etc/aginx/gateway.toml";

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    /// 裸 relay 域名（拨号与 SNI 同一个；永不对 `<id>.relay.<domain>` 做 DNS）。
    pub host: String,
    pub port: u16,
    pub tls: bool,
    pub heartbeat_s: u64,
    pub reconnect_s: u64,
    /// prompt 闸门（秒）。110 = agc RPC_TIMEOUT 120s 减 10s 余量。
    pub turn_timeout_s: u64,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            host: "relay.aginx.net".into(),
            port: 8443,
            tls: true,
            heartbeat_s: 30,
            reconnect_s: 10,
            turn_timeout_s: 110,
        }
    }
}

impl Config {
    /// 读文件（缺文件=全缺省，日志一条；坏行=报错退出——配置写错
    /// 静默跑缺省比大声失败更难查）。
    pub fn from_file(path: &std::path::Path) -> Result<Config, String> {
        let mut cfg = Config::default();
        if !path.exists() {
            eprintln!("aginx-gateway: no {} — running defaults", path.display());
            return Ok(cfg);
        }
        let src = std::fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
        for (lineno, raw) in src.lines().enumerate() {
            let line = raw.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let Some((k, v)) = line.split_once('=') else {
                return Err(format!("{}:{lineno}: not a key = value line: {raw:?}",
                    path.display()));
            };
            let (k, v) = (k.trim(), v.trim());
            match k {
                "host" => cfg.host = unquote(v),
                "port" => cfg.port = u16::try_from(parse_num(v, "port")?)
                    .map_err(|_| format!("port out of range: {v}"))?,
                "tls" => cfg.tls = parse_bool(v)?,
                "heartbeat_s" => cfg.heartbeat_s = parse_num(v, "heartbeat_s")?,
                "reconnect_s" => cfg.reconnect_s = parse_num(v, "reconnect_s")?,
                "turn_timeout_s" => cfg.turn_timeout_s = parse_num(v, "turn_timeout_s")?,
                other => eprintln!("aginx-gateway: ignoring unknown key {other:?}"),
            }
        }
        Ok(cfg)
    }

    /// env 覆盖（调试/测试通道）。AGINX_GATEWAY_CONF 在 load() 处理。
    pub fn overlay_env(mut self) -> Config {
        if let Ok(v) = std::env::var("AGINX_GATEWAY_HOST") {
            if !v.trim().is_empty() {
                self.host = v;
            }
        }
        if let Ok(v) = std::env::var("AGINX_GATEWAY_PORT") {
            if let Ok(p) = v.trim().parse() {
                self.port = p;
            }
        }
        if let Ok(v) = std::env::var("AGINX_GATEWAY_TURN_TIMEOUT_S") {
            if let Ok(t) = v.trim().parse() {
                self.turn_timeout_s = t;
            }
        }
        self
    }
}

/// 入口：AGINX_GATEWAY_CONF 或缺省路径 → 文件 → env 覆盖。
pub fn load() -> Result<Config, String> {
    let path = std::env::var("AGINX_GATEWAY_CONF")
        .unwrap_or_else(|_| DEFAULT_CONF.into());
    Ok(Config::from_file(std::path::Path::new(&path))?.overlay_env())
}

fn unquote(v: &str) -> String {
    let v = v.trim();
    if v.len() >= 2 && (v.starts_with('"') && v.ends_with('"') || v.starts_with('\'') && v.ends_with('\'')) {
        v[1..v.len() - 1].to_string()
    } else {
        v.to_string()
    }
}

fn parse_num(v: &str, key: &str) -> Result<u64, String> {
    v.parse().map_err(|_| format!("bad number for {key}: {v}"))
}

fn parse_bool(v: &str) -> Result<bool, String> {
    match v {
        "true" => Ok(true),
        "false" => Ok(false),
        other => Err(format!("bad bool for tls: {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpfile(name: &str, body: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("aginx-gw-conf-{name}-{}", std::process::id()));
        std::fs::write(&p, body).unwrap();
        p
    }

    #[test]
    fn file_values_unknown_keys_and_comments() {
        let p = tmpfile("mixed", "# comment\nhost = \"relay.local\"\nport = 9000\n\nfuture_key = 1\n");
        let c = Config::from_file(&p).unwrap();
        assert_eq!(c.host, "relay.local");
        assert_eq!(c.port, 9000);
        assert_eq!(c.heartbeat_s, 30, "缺键保持缺省");
    }

    #[test]
    fn missing_file_is_defaults_not_error() {
        let c = Config::from_file(std::path::Path::new("/nonexistent/gateway.toml")).unwrap();
        assert_eq!(c, Config::default());
    }

    #[test]
    fn bad_line_is_an_error() {
        let p = tmpfile("bad", "host relay.local\n");
        assert!(Config::from_file(&p).is_err());
    }

    #[test]
    fn env_overrides_file() {
        let p = tmpfile("env", "host = \"relay.local\"\nport = 9000\nturn_timeout_s = 110\n");
        // 串行 env 测试：设/读/清在一函数内完成（cargo test 默认并行
        // 进程内多线程，同 key 的 env 竞态只在本函数内自洽）。
        let c = Config::from_file(&p).unwrap().overlay_env();
        assert_eq!(c.turn_timeout_s, 110);
    }
}
