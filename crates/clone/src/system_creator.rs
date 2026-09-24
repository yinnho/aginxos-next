//! 系统分身种子 — clone-creator（克隆大师）随二进制分发。
//!
//! 个人安装版没有中心服务器替你预装分身，"制造分身"这个能力本身必须是
//! 开箱即有的：daemon 启动时若未注册 clone-creator，就把内嵌的定义层走
//! [`crate::write_files_to_workspace`] 同款正规安装管线（kernel 的
//! `clone_install_files`）装上。此后所有分身都由它生成——不手工摆文件。
//!
//! 定义层拷贝自 opencarrier-clones/clone-creator（同格式规范），仅改
//! template.json 品牌字段。**上游进化时重新拷贝 + 调整这里的文件清单即可；
//! 但 2026-08-25 起 staging + [CLONE_INSTALL] 标记安装是 aginx-carrier
//! 侧的分叉**（opencarrier 无 clone_marker handler，其上游仍教 clone_install
//! 工具）——重新拷贝时必须保留 staging/标记教学，勿被上游旧流程覆盖。
//! `system_creator_files_passes_install_format` 测试保证资产与安装期硬校验
//! （[`crate::validate_install_format`]）不打架。

/// 系统分身名（registry / workspace 目录名）。
pub const SYSTEM_CREATOR_NAME: &str = "clone-creator";

/// 系统身份「我」的种子（第二系统分身，2026-08-26）。
///
/// 与 clone-creator 同款 boot 种子机制；定位见 assets/me/SOUL.md——
/// 主人的统一身份（对内总管/对外门面）。它的超能力全部来自定义层 +
/// runtime 的 gateway_hub 工具（contacts_list/contact_prompt，经
/// flows/hub 声明注入），内核零特判。
pub const SYSTEM_ME_NAME: &str = "me";

/// 「我」的内嵌定义层文件清单。
pub fn system_me_files() -> std::collections::BTreeMap<String, Vec<u8>> {
    const FILES: &[(&str, &str)] = &[
        ("template.json", include_str!("../assets/me/template.json")),
        ("SOUL.md", include_str!("../assets/me/SOUL.md")),
        (
            "system_prompt.md",
            include_str!("../assets/me/system_prompt.md"),
        ),
        ("profile.md", include_str!("../assets/me/profile.md")),
        ("MEMORY.md", include_str!("../assets/me/MEMORY.md")),
        ("EVOLUTION.md", include_str!("../assets/me/EVOLUTION.md")),
        (
            "flows/hub/flow.md",
            include_str!("../assets/me/flows/hub/flow.md"),
        ),
    ];
    FILES
        .iter()
        .map(|(p, c)| (p.to_string(), c.as_bytes().to_vec()))
        .collect()
}

/// 内嵌定义层文件清单（path → 内容）。顺序无关，装入 BTreeMap 后交给
/// `clone_install_files`。
pub fn system_creator_files() -> std::collections::BTreeMap<String, Vec<u8>> {
    const FILES: &[(&str, &str)] = &[
        (
            "template.json",
            include_str!("../assets/clone-creator/template.json"),
        ),
        (
            "SOUL.md",
            include_str!("../assets/clone-creator/SOUL.md"),
        ),
        (
            "system_prompt.md",
            include_str!("../assets/clone-creator/system_prompt.md"),
        ),
        (
            "profile.md",
            include_str!("../assets/clone-creator/profile.md"),
        ),
        (
            "MEMORY.md",
            include_str!("../assets/clone-creator/MEMORY.md"),
        ),
        (
            "EVOLUTION.md",
            include_str!("../assets/clone-creator/EVOLUTION.md"),
        ),
        (
            "knowledge/celebrity-distillation.md",
            include_str!("../assets/clone-creator/knowledge/celebrity-distillation.md"),
        ),
        (
            "knowledge/clone-best-practices.md",
            include_str!("../assets/clone-creator/knowledge/clone-best-practices.md"),
        ),
        (
            "knowledge/extraction-framework.md",
            include_str!("../assets/clone-creator/knowledge/extraction-framework.md"),
        ),
        (
            "knowledge/personality-ethics.md",
            include_str!("../assets/clone-creator/knowledge/personality-ethics.md"),
        ),
        (
            "knowledge/personality-extraction.md",
            include_str!("../assets/clone-creator/knowledge/personality-extraction.md"),
        ),
        (
            "knowledge/plugin-tools.md",
            include_str!("../assets/clone-creator/knowledge/plugin-tools.md"),
        ),
        (
            "knowledge/tool-catalog.md",
            include_str!("../assets/clone-creator/knowledge/tool-catalog.md"),
        ),
        (
            "flows/clone-generate/flow.md",
            include_str!("../assets/clone-creator/flows/clone-generate/flow.md"),
        ),
        (
            "flows/skill-design/flow.md",
            include_str!("../assets/clone-creator/flows/skill-design/flow.md"),
        ),
        (
            "agents/agent-designer.md",
            include_str!("../assets/clone-creator/agents/agent-designer.md"),
        ),
        (
            "agents/personality-extractor.md",
            include_str!("../assets/clone-creator/agents/personality-extractor.md"),
        ),
        (
            "agents/quality-reviewer.md",
            include_str!("../assets/clone-creator/agents/quality-reviewer.md"),
        ),
        (
            "agents/skill-designer.md",
            include_str!("../assets/clone-creator/agents/skill-designer.md"),
        ),
    ];
    FILES
        .iter()
        .map(|(p, c)| (p.to_string(), c.as_bytes().to_vec()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 资产漂移闸门：内嵌定义层必须过安装期硬校验（skills/ 根目录、
    /// description 缺失等），否则 daemon 启动种子会在 `clone_install_files`
    /// 里被自己拒收。
    #[test]
    fn system_creator_files_passes_install_format() {
        let files = system_creator_files();
        assert_eq!(files.len(), 19, "文件清单数量与拷贝时不一致，检查 assets/");

        let errors = crate::validate_install_format(&files).expect("validate panicked");
        assert!(
            errors.is_empty(),
            "内嵌 clone-creator 定义层未过安装校验：\n- {}",
            errors.join("\n- ")
        );

        // template.json 必须可解析且 name 对齐（spawn_agent 用它注册）。
        let tpl = crate::parse_template_manifest_lenient(
            std::str::from_utf8(&files["template.json"]).unwrap(),
        )
        .expect("template.json unparseable");
        assert_eq!(tpl.name, SYSTEM_CREATOR_NAME);

        // default_flow 指向的 flow 必须真的在（否则装完 default_flow 兜底空转）。
        assert!(files.contains_key("flows/clone-generate/flow.md"));
    }

    /// 「我」种子资产漂移闸门（同上，独立清单独立断言）。
    #[test]
    fn system_me_files_passes_install_format() {
        let files = system_me_files();
        assert_eq!(files.len(), 7, "「我」文件清单数量与拷贝时不一致，检查 assets/me/");

        let errors = crate::validate_install_format(&files).expect("validate panicked");
        assert!(
            errors.is_empty(),
            "内嵌「我」定义层未过安装校验：\n- {}",
            errors.join("\n- ")
        );

        let tpl = crate::parse_template_manifest_lenient(
            std::str::from_utf8(&files["template.json"]).unwrap(),
        )
        .expect("template.json unparseable");
        assert_eq!(tpl.name, SYSTEM_ME_NAME);
        assert!(files.contains_key("flows/hub/flow.md"));
    }
}
