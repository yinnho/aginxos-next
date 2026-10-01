//! wifi.conf 写方共享件（#64 多网回退表，2026-10-01）。
//!
//! `/etc/wifi.conf` 是平铺 KEY=VALUE：`ssid=`/`psk=` 是**当前网**，
//! `alt1_ssid=`/`alt1_psk=`…（1..=[`ALT_MAX`]）是**有序回退表**——读方是
//! rootfs/libexec/aginx/net-rejoin 的回退腿（当前网两轮败后按槽序各试
//! 一次，赢家换位升为当前网）。本 crate 只做写侧的一件事：**换当前网而
//! 不丢表**——`ssid=`/`psk=` 两行原位换值（首见为准，重复行收敛为一条），
//! 其余行（alt 槽、注释）逐字保留；被换下的旧当前网降级追加为下一空槽
//! 的 alt 对（表满则覆写最后一槽），死网复活那天它还在册。
//!
//! 与 sh 读方的格式契约同源注释在三个消费点（net-rejoin / 本文件 /
//! wifi.conf.example），改动须三处同批。
//!
//! 纯文本变换、零依赖、无机器事实（D14）。

/// 回退表槽上限。net-rejoin 的 `MAXALT` 同值——写满即覆写最后一槽，
/// 表不无界生长。
pub const ALT_MAX: u32 = 4;

/// 旧 conf + 新当前网 → 新 conf 文本。规则见 crate 文档；要点：
/// - 无 `ssid=`/`psk=` 行（含空文件）→ 两行置顶补齐；
/// - 旧当前网与新股同名（含改密码）→ 只换值，不降级（幂等）；
/// - 降级只在旧对齐全（ssid+psk 都非空）时发生——半行残档不值得救。
pub fn rewrite(existing: &str, ssid: &str, psk: &str) -> String {
    let old = current_pair(existing);
    let demote = match old {
        (Some(os), Some(op))
            if !os.is_empty() && !op.is_empty() && os != ssid =>
        {
            Some((next_slot(existing), os, op))
        }
        _ => None,
    };

    let mut out = String::new();
    let mut have_ssid = false;
    let mut have_psk = false;
    for line in existing.lines() {
        if let Some((slot, _, _)) = &demote {
            // 覆写槽：旧 altN 两行让位，降级对统一追加在文末
            if line.starts_with(&format!("alt{slot}_ssid="))
                || line.starts_with(&format!("alt{slot}_psk="))
            {
                continue;
            }
        }
        if line.starts_with("ssid=") {
            if !have_ssid {
                out.push_str("ssid=");
                out.push_str(ssid);
                out.push('\n');
                have_ssid = true;
            }
        } else if line.starts_with("psk=") {
            if !have_psk {
                out.push_str("psk=");
                out.push_str(psk);
                out.push('\n');
                have_psk = true;
            }
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    if !have_ssid {
        out.insert_str(0, &format!("ssid={ssid}\n"));
    }
    if !have_psk {
        // 无 psk 行=文件本就缺：紧贴置顶的 ssid 行之后
        let at = out.find('\n').map_or(0, |i| i + 1);
        out.insert_str(at, &format!("psk={psk}\n"));
    }
    if let Some((slot, os, op)) = demote {
        out.push_str(&format!("alt{slot}_ssid={os}\n"));
        out.push_str(&format!("alt{slot}_psk={op}\n"));
    }
    out
}

/// 首个 `ssid=`/`psk=` 行的值（CR 剥离）。缺行=None。
fn current_pair(existing: &str) -> (Option<&str>, Option<&str>) {
    fn value_of(l: &str, n: usize) -> &str {
        l[n..].trim_end_matches('\r')
    }
    let ssid = existing.lines().find(|l| l.starts_with("ssid=")).map(|l| value_of(l, 5));
    let psk = existing.lines().find(|l| l.starts_with("psk=")).map(|l| value_of(l, 4));
    (ssid, psk)
}

/// 下一空槽：1..=ALT_MAX 里第一个没有任何 `altN_` 行的；全占则
/// ALT_MAX（覆写最后一槽——最新降级者比老槽更有回退价值）。
fn next_slot(existing: &str) -> u32 {
    (1..=ALT_MAX)
        .find(|n| !existing.lines().any(|l| l.starts_with(&format!("alt{n}_"))))
        .unwrap_or(ALT_MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_file_is_the_plain_pair() {
        // 与 n7-l0/pair 全流程的历史断言逐字节同形
        assert_eq!(rewrite("", "A", "p"), "ssid=A\npsk=p\n");
    }

    #[test]
    fn old_current_demotes_to_first_free_slot() {
        let out = rewrite("ssid=old\npsk=op\n", "new", "np");
        assert_eq!(out, "ssid=new\npsk=np\nalt1_ssid=old\nalt1_psk=op\n");
    }

    #[test]
    fn existing_alts_ride_through_and_demote_takes_next_free() {
        let existing = "ssid=old\npsk=op\n\n# ops note\nalt1_ssid=keep1\nalt1_psk=k1\nalt3_ssid=keep3\nalt3_psk=k3\n";
        let out = rewrite(existing, "new", "np");
        assert_eq!(
            out,
            "ssid=new\npsk=np\n\n# ops note\nalt1_ssid=keep1\nalt1_psk=k1\nalt3_ssid=keep3\nalt3_psk=k3\nalt2_ssid=old\nalt2_psk=op\n"
        );
    }

    #[test]
    fn same_ssid_is_idempotent_no_self_demote() {
        // 改密码形状：同网重配只换 psk，不产生指向自己的 alt 槽
        let out = rewrite("ssid=A\npsk=old\nalt1_ssid=B\nalt1_psk=b\n", "A", "new");
        assert_eq!(out, "ssid=A\npsk=new\nalt1_ssid=B\nalt1_psk=b\n");
    }

    #[test]
    fn full_table_overwrites_last_slot() {
        let mut existing = String::from("ssid=old\npsk=op\n");
        for n in 1..=ALT_MAX {
            existing.push_str(&format!("alt{n}_ssid=f{n}\nalt{n}_psk=p{n}\n"));
        }
        let out = rewrite(&existing, "new", "np");
        assert!(out.contains("alt3_ssid=f3"), "未动槽保留: {out}");
        assert!(out.contains("alt4_ssid=old"), "满表覆写最后一槽: {out}");
        assert!(!out.contains("alt4_ssid=f4"), "旧 alt4 让位: {out}");
        // 槽总数不超上限
        let n = out.lines().filter(|l| l.starts_with("alt") && l.ends_with(&format!("=old"))).count();
        assert_eq!(n, 1);
    }

    #[test]
    fn half_written_old_pair_is_not_demoted() {
        // 旧档只有 ssid 没有 psk——降级会造出一个永远连不上的 alt，弃
        let out = rewrite("ssid=old\n", "new", "np");
        assert_eq!(out, "ssid=new\npsk=np\n");
    }

    #[test]
    fn duplicate_pair_lines_collapse_to_one() {
        // 病档双写：首见为准收敛为一对；首见旧对（x/y）照常降级进下一空槽
        let out = rewrite("ssid=x\npsk=y\nssid=x2\npsk=y2\nalt1_ssid=B\nalt1_psk=b\n", "A", "p");
        assert_eq!(
            out,
            "ssid=A\npsk=p\nalt1_ssid=B\nalt1_psk=b\nalt2_ssid=x\nalt2_psk=y\n"
        );
    }

    #[test]
    fn crlf_values_are_cleaned_on_demote() {
        let out = rewrite("ssid=old\r\npsk=op\r\n", "new", "np");
        assert_eq!(out, "ssid=new\npsk=np\nalt1_ssid=old\nalt1_psk=op\n");
    }
}
