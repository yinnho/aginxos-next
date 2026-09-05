//! 金样本钉形（N5⑤）：wire 权威 = 生态仓 aginx/ACP.md §6。
//!
//! 样本值以字面量复制（那份文档不在本仓）；**字段名与形状**是契约。
//! 改协议 = 同批改 ACP.md + 全部说话端（agc/relay/生态网关/本网关），
//! 这里红 = 有人单边改了形状。

use aginx_gateway as gw;

fn relay_of(json: &str) -> gw::RelayMessage {
    serde_json::from_str(json).expect("golden sample must parse as a relay frame")
}

// ---- 第 0 层帧 ----

#[test]
fn golden_relay_register_and_registered() {
    // ACP.md §6 relay_register
    let m = relay_of(r#"{"type": "register", "id": "qi7o6bj5", "token": "<relay-secret>"}"#);
    assert_eq!(
        m,
        gw::RelayMessage::Register { id: "qi7o6bj5".into(), token: Some("<relay-secret>".into()) }
    );
    // 我们发的 register 序列化回同一形状（token=Some——单门鉴权必带）。
    let wire = serde_json::to_value(&m).unwrap();
    assert_eq!(wire["type"], "register");
    assert_eq!(wire["id"], "qi7o6bj5");
    assert_eq!(wire["token"], "<relay-secret>");

    // §6 relay_registered
    assert_eq!(
        relay_of(r#"{"type": "registered", "id": "qi7o6bj5", "url": "agent://qi7o6bj5.relay.aginx.net"}"#),
        gw::RelayMessage::Registered {
            id: "qi7o6bj5".into(),
            url: "agent://qi7o6bj5.relay.aginx.net".into(),
        }
    );
}

#[test]
fn golden_relay_data_to_gateway_parses() {
    // §6 relay_data_to_gateway：入站 prompt 的真实形状
    let m = relay_of(
        r#"{"type": "data", "client_id": "c_a1b2c3d4", "data": {"jsonrpc": "2.0", "id": 2, "method": "prompt", "params": {"agent": "travel-planner", "message": "只回：ok"}}}"#,
    );
    match m {
        gw::RelayMessage::Data { client_id, data } => {
            assert_eq!(client_id, "c_a1b2c3d4");
            assert_eq!(data["method"], "prompt");
            assert_eq!(data["params"]["agent"], "travel-planner");
            assert_eq!(data["params"]["message"], "只回：ok");
        }
        other => panic!("expected Data, got {other:?}"),
    }
}

#[test]
fn client_side_variants_parse_but_never_send() {
    // 现役 relay 可能旧于 HEAD：connect/connected 必须可解析（容错），
    // 收到只记日志。字段名 snake_case。
    assert_eq!(
        relay_of(r#"{"type": "connect", "target": "qi7o6bj5", "token": "<relay-secret>"}"#),
        gw::RelayMessage::Connect { target: "qi7o6bj5".into(), token: Some("<relay-secret>".into()) }
    );
    assert_eq!(
        relay_of(r#"{"type": "connected", "client_id": "c_a1b2c3d4"}"#),
        gw::RelayMessage::Connected { client_id: "c_a1b2c3d4".into() }
    );
}

// ---- 第 1 层外部协议 ----

#[test]
fn golden_directed_response_shape() {
    // §6 relay_directed_response：定向帧 = clientId + 完整 JSON-RPC 响应。
    // 我们的 initialize 应答经 Directed 包装后的形状对照（authenticated
    // 本网关恒 false——第 0 层单门，无外部鉴权态；golden 里 true 是生态
    // 网关的值，形状同、值异）。
    let init = serde_json::json!({
        "jsonrpc": "2.0", "id": 1,
        "result": gw::initialize_result(),
    });
    let mut wrapped = init.clone();
    wrapped["clientId"] = serde_json::json!("c_a1b2c3d4");
    // 字段集与类型对照 golden
    for key in ["clientId", "jsonrpc", "id", "result"] {
        assert!(wrapped.get(key).is_some(), "missing {key}");
    }
    assert_eq!(wrapped["result"]["protocolVersion"], 1, "整数 1，不是字符串");
    assert_eq!(wrapped["result"]["serverInfo"]["name"], "aginx-gateway");
}

#[test]
fn golden_chunk_notification_is_bare_broadcast() {
    // §6 external_chunk_notification / relay_broadcast_notification：
    // 无 id 无 clientId，method=chunk，text 在 params。
    let frames = gw::turn_frames_ok("好的，我来规划");
    assert_eq!(frames.len(), 2);
    match &frames[0] {
        gw::Wire::Broadcast(v) => {
            assert_eq!(v["jsonrpc"], "2.0");
            assert_eq!(v["method"], "chunk");
            assert_eq!(v["params"]["text"], "好的，我来规划");
            assert!(v.get("id").is_none(), "chunk 无 id");
            assert!(v.get("clientId").is_none(), "广播裸行");
        }
        other => panic!("chunk must be broadcast, got {other:?}"),
    }
}

#[test]
fn golden_final_result_is_bare_and_endturn() {
    // §6 external_final_result_plain：终帧无 id，stopReason=endTurn。
    let frames = gw::turn_frames_ok("ok");
    match &frames[1] {
        gw::Wire::Broadcast(v) => {
            assert_eq!(v["jsonrpc"], "2.0");
            assert_eq!(v["result"]["stopReason"], "endTurn");
            assert!(v.get("id").is_none());
            assert!(v.get("error").is_none());
        }
        other => panic!("final must be broadcast, got {other:?}"),
    }
}

#[test]
fn error_codes_follow_the_external_table() {
    // §5 外部错误表：-32601 方法/化身不存在；-32603 进程/超时/轮失败；
    // -32602 params；-32700 解析。
    let e = gw::error_frame(&serde_json::json!(5), -32601, "unknown avatar '小满'");
    match e {
        gw::Wire::Directed(v) => {
            assert_eq!(v["id"], 5);
            assert_eq!(v["error"]["code"], -32601);
            assert_eq!(v["error"]["message"], "unknown avatar '小满'");
        }
        _ => panic!("errors go directed"),
    }
}
