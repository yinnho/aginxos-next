// channels — iLink（微信）入站通道的 server 直调接线（2026-09-26 iLink 上机线）。
//
// carrier daemon（`aginx-carrier start`）走 wiring::boot_channels 全家；
// server 直调形态不背 carrier bin crate（host.rs 同裁决），这里只装手机的
// iLink 子集：weixin watcher + 微信工具 + DB 持久化回调 + 出站 send 注入。
// webhook 不装（daemon 形态专属，config 默认关）。
//
// 零功耗纪律：没有任何扫码会话时 SessionWatcher 只留一个 5s 目录扫描
// 线程（本机裁决不睡，代价可忽略），不起任何轮询。

use std::sync::Arc;

use carrier_kernel::kernel::CarrierKernel;
use carrier_runtime::channel_manager::ChannelManager;
use carrier_runtime::kernel_handle::KernelHandle;
use carrier_runtime::plugin::router::SenderRouter;

use carrier_ilink::models::BotTokenFile;
use carrier_memory::weixin_store::WeixinSessionRow;

/// WeixinSessionRow（DB 行）→ BotTokenFile（通道会话形状）——与
/// wiring::weixin_row_to_token_file 同构；两份手抄必漂移，改动须两边同步。
fn weixin_row_to_token_file(r: WeixinSessionRow) -> BotTokenFile {
    let ctx: std::collections::HashMap<String, String> =
        serde_json::from_str(&r.context_tokens).unwrap_or_default();
    BotTokenFile {
        channel: r.channel,
        sender_key: r.sender_key,
        bot_id: r.bot_id,
        bot_token: r.bot_token,
        baseurl: r.baseurl,
        ilink_bot_id: r.ilink_bot_id,
        user_id: r.user_id,
        expires_at: r.expires_at,
        bind_agent: r.bind_agent,
        context_tokens: ctx,
    }
}

/// 装 iLink 通道（不含 start——调用方在自己的 runtime 上下文里
/// `cm.start().await`，与 kernel boot 同一时机约定）。
pub fn boot_ilink(kernel: &Arc<CarrierKernel>) -> anyhow::Result<ChannelManager> {
    let kh: Arc<dyn KernelHandle> = kernel.clone();
    let mut cm = ChannelManager::new(kh);

    // 绑定即路由的内存路由表（真源在会话 bind_agent）
    let sender_router = Arc::new(SenderRouter::new());
    cm.set_sender_router(sender_router.clone());

    // cron 投递（last-channel 追踪）+ 通知路由存储
    {
        let store = Arc::new(kernel.memory.cron_delivery().clone());
        cm.set_cron_delivery(store);
    }
    {
        let store = Arc::new(kernel.memory.notify_store().clone());
        cm.set_notify_store(store);
    }

    cm.register("weixin", Box::new(carrier_ilink::SessionWatcher::new()));

    // 微信 iLink 工具（扫码登录/发消息/发图/发视频/状态）进工具分发器
    {
        let dispatcher = cm.tool_dispatcher();
        let mut builtin = carrier_runtime::plugin::BuiltinPlugin::new(
            "weixin".to_string(),
            "1.0.0".to_string(),
            std::path::PathBuf::new(),
        );
        builtin.register_tool(Box::new(carrier_ilink::WeixinQrLoginTool));
        builtin.register_tool(Box::new(carrier_ilink::WeixinSendMessageTool));
        builtin.register_tool(Box::new(carrier_ilink::WeixinSendImageTool));
        builtin.register_tool(Box::new(carrier_ilink::WeixinSendVideoTool));
        builtin.register_tool(Box::new(carrier_ilink::WeixinStatusTool));
        dispatcher.register(Arc::new(builtin));
    }

    // weixin_sessions DB 持久化回调——必须在 start() 之前装：watcher 的
    // start() 走 load_from_dir，回调装晚了永远读 JSON 旁路、无视 DB 表
    // （opencarrier 踩过）。
    {
        let store = kernel.memory.weixin_store().clone();
        let persist_fn: carrier_ilink::token::SessionPersistFn = Arc::new(move |tf| {
            let row = WeixinSessionRow {
                channel: tf.channel.clone(),
                sender_key: tf.sender_key.clone(),
                bot_id: tf.bot_id.clone(),
                bot_token: tf.bot_token.clone(),
                baseurl: tf.baseurl.clone(),
                ilink_bot_id: tf.ilink_bot_id.clone(),
                user_id: tf.user_id.clone(),
                expires_at: tf.expires_at,
                bind_agent: tf.bind_agent.clone(),
                context_tokens: serde_json::to_string(&tf.context_tokens).unwrap_or_default(),
            };
            if let Err(e) = store.upsert(&row) {
                tracing::warn!("Failed to persist weixin session to DB: {e}");
            }
        });
        let store2 = kernel.memory.weixin_store().clone();
        let load_fn: carrier_ilink::token::SessionsLoadFn = Arc::new(move || match store2.load_all()
        {
            Ok(rows) => rows
                .into_iter()
                .map(weixin_row_to_token_file)
                .collect::<Vec<_>>(),
            Err(e) => {
                tracing::warn!("Failed to load weixin sessions from DB: {e}");
                Vec::new()
            }
        });
        carrier_ilink::token::WEIXIN_STATE.set_persist_fns(persist_fn, load_fn);

        // 绑定即路由：会话加载与扫码注册都带 bind_agent 调 seeder——
        // 路由与绑定永远同源。UUID 形态的 bind_agent 解析成分身名。
        let router = sender_router.clone();
        let kernel_ref = kernel.clone();
        let seed_fn: carrier_ilink::token::RouteSeedFn = Arc::new(move |user_id: &str, agent: &str| {
            let agent_ref =
                if let Ok(id) = agent.parse::<carrier_types::agent::AgentId>() {
                    kernel_ref
                        .registry
                        .get(id)
                        .map(|e| e.manifest.name.clone())
                        .unwrap_or_else(|| agent.to_string())
                } else {
                    agent.to_string()
                };
            router.set_route(user_id, &agent_ref);
        });
        carrier_ilink::token::WEIXIN_STATE.set_route_seeder(seed_fn);
    }

    // 出站通道注入 kernel（cron 主动推送探针 + 富媒体投递）
    let send_fn = cm.make_channel_send_fn();
    *kernel.channel_send_fn.write().unwrap() = Some(send_fn);
    let deliver_fn = cm.make_channel_deliver_fn();
    *kernel.channel_deliver_fn.write().unwrap() = Some(deliver_fn);
    let probe = cm.make_supports_proactive_fn();
    *kernel.channel_supports_proactive_fn.write().unwrap() = Some(probe);
    // 工具分发器注入 kernel（agent 工具调用走通道工具）
    {
        let dispatcher = cm.tool_dispatcher();
        let mut guard = kernel
            .plugins
            .plugin_tool_dispatcher
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        *guard = Some(dispatcher);
    }

    Ok(cm)
}
