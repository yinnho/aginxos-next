//! Local tar 安装源（M30）：`agent install --file <tar>`。
//!
//! AginxOS 手机上化身先走本地包（duphub auth 等 M36 sidecar）：tar 是
//! 分身定义层的平铺快照（与 dup 工作区同构：flows/、knowledge/、
//! profile.md、SOUL.md …）。gzip 魔数嗅探，`.tar` 与 `.tar.gz` 都收。
//! 读成 `BTreeMap<相对路径, bytes>` 后走与 DupHub 拉取完全相同的
//! `clone_install_files` 正规管线（含 validate_install_format 硬闸），
//! 本模块不做格式裁决——只做传输层安全（路径逃逸/大小帽）。

use std::collections::BTreeMap;
use std::io::Read;
use std::path::Path;

use anyhow::{Context, Result, bail};

/// 单成员上限：定义层是文本为主的 KB-MB 级内容，256 MiB 是防套娃的
/// 帽子不是配额。
const MEMBER_MAX: u64 = 256 * 1024 * 1024;

/// Read a clone definition-layer tar (.tar or .tar.gz) into a files map.
pub fn read_clone_tar(path: &Path) -> Result<BTreeMap<String, Vec<u8>>> {
    let raw = std::fs::File::open(path).with_context(|| format!("打开 {}", path.display()))?;
    let mut magic = [0u8; 2];
    let mut reader: Box<dyn Read> = if raw.metadata()?.len() >= 2 {
        use std::io::{Seek, SeekFrom};
        let mut file = raw;
        let _ = file.read_exact(&mut magic);
        file.seek(SeekFrom::Start(0))?;
        if magic == [0x1f, 0x8b] {
            Box::new(flate2::read::GzDecoder::new(file))
        } else {
            Box::new(file)
        }
    } else {
        Box::new(raw)
    };

    let mut files = BTreeMap::new();
    let mut archive = tar::Archive::new(&mut reader);
    for entry in archive.entries().context("读 tar 条目失败")? {
        let mut entry = entry.context("读 tar 条目失败")?;
        let header = entry.header();
        if header.entry_type() != tar::EntryType::Regular {
            continue; // 目录隐式创建，符号链接不入定义层
        }
        let rel = entry.path().context("读 tar 路径失败")?.to_path_buf();
        let Some(rel) = rel.to_str().map(|s| s.trim_start_matches("./")).map(str::to_string) else {
            bail!("tar 内含非 UTF-8 路径");
        };
        if rel.is_empty() {
            continue;
        }
        let top = rel.split('/').next().unwrap_or(&rel);
        if top == ".dup" || rel == ".dup" {
            continue; // 历史目录不随包走（重装保留本机 .dup/）
        }
        // macOS bsdtar 默认把 xattr/resource fork 存成 `._<name>` AppleDouble
        // 成员——定义层从不想要它们（开发机是 macOS，实测混进包里）。
        let base = rel.rsplit('/').next().unwrap_or(&rel);
        if base.starts_with("._") || base == ".DS_Store" {
            continue;
        }
        let components = Path::new(&rel).components();
        let mut safe = true;
        for c in components {
            if matches!(c, std::path::Component::ParentDir | std::path::Component::RootDir) {
                safe = false;
                break;
            }
        }
        if !safe {
            bail!("tar 内含不安全路径（拒绝）: {rel}");
        }
        if header.size()? > MEMBER_MAX {
            bail!("tar 成员过大（>{} MiB）: {rel}", MEMBER_MAX / 1024 / 1024);
        }
        let mut buf = Vec::with_capacity(header.size()? as usize);
        entry.read_to_end(&mut buf).with_context(|| format!("读成员 {rel}"))?;
        files.insert(rel, buf);
    }
    if files.is_empty() {
        bail!("tar 里没有可用文件（空包或全被跳过）");
    }
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn scratch_dir(tag: &str) -> std::path::PathBuf {
        static SEQ: AtomicUsize = AtomicUsize::new(0);
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "clone-tar-test-{}-{n}-{tag}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn build_tar(path: &Path, files: &[(&str, &str)]) {
        let out = fs::File::create(path).unwrap();
        let mut b = tar::Builder::new(out);
        for (name, content) in files {
            let mut h = tar::Header::new_gnu();
            h.set_size(content.len() as u64);
            h.set_mode(0o644);
            h.set_cksum();
            b.append_data(&mut h, name, content.as_bytes()).unwrap();
        }
        b.finish().unwrap();
    }

    #[test]
    fn reads_plain_and_skips_dup_history() {
        let dir = scratch_dir("plain");
        let t = dir.join("c.tar");
        build_tar(&t, &[
            ("profile.md", "# p"),
            ("._profile.md", "macOS AppleDouble junk"),
            ("flows/a/flow.md", "---\nname: a\ndescription: 做事\n---\nbody"),
            (".dup/state.json", "{\"evil\": true}"),
        ]);
        let files = read_clone_tar(&t).unwrap();
        assert_eq!(files.len(), 2, "dup history + AppleDouble skipped: {files:?}");
        let flow = String::from_utf8(files["flows/a/flow.md"].clone()).unwrap();
        assert_eq!(flow, "---\nname: a\ndescription: 做事\n---\nbody");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn rejects_traversal_member() {
        let dir = scratch_dir("evil");
        let t = dir.join("evil.tar");
        // append_data 会拒绝 `..` 路径，恶意成员得走原始 header 写入
        // （set_cksum 前先把路径字节直接拷进 name 字段）。
        let mut out = fs::File::create(&t).unwrap();
        {
            let mut b = tar::Builder::new(&mut out);
            let content = b"nope";
            let mut h = tar::Header::new_gnu();
            h.set_size(content.len() as u64);
            h.set_mode(0o644);
            h.set_entry_type(tar::EntryType::Regular);
            let name = b"../escape.txt";
            h.as_old_mut().name[..name.len()].copy_from_slice(name);
            h.set_cksum();
            b.append(&h, std::io::Cursor::new(content)).unwrap();
            b.finish().unwrap();
        }
        drop(out);
        assert!(read_clone_tar(&t).is_err());
        fs::remove_dir_all(&dir).ok();
    }
}
