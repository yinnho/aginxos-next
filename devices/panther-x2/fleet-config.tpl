# aginx 网关 config 真源模板（fleet.sh init 三注入：__ID__×2、
# __RELAY_SECRET__、__JWT_SECRET__；jwt 每台新生，id 必须小写字母数字——
# relay 拒绝连字符，同律见 crates/pair sanitize）。
[server]
access = "private"

[relay]
id = "__ID__"
domain = "relay.aginx.net"
port = 8443
use_tls = true
url = "__ID__.relay.aginx.net:8443"
heartbeat_interval = 30
reconnect_interval = 5
relay_secret = "__RELAY_SECRET__"

[auth]
jwt_secret = "__JWT_SECRET__"
