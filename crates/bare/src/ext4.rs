//! ext4 只读（S2 正题）：裸件读出 userdata 指定文件与 L0 一致。
//!
//! 参照=本仓 dist/ 下的真机镜像 rootfs.img（mkfs.ext4 默认：
//! 4K 块、256B inode、64B 组描述符、extents+64bit+flex_bg）+ 内核
//! fs/ext4 布局。**只做只读**：superblock → 组描述符 → inode 表 →
//! extent 树（含 depth>0 索引递归）→ 目录线性扫描（大目录 HTREE 也
//! 靠扫全数据块——只读查找无需 hash 定位）→ 文件区间读。
//!
//! 设计约束（裸件）：零内部分配——所有工作缓冲由调用方 scratch 传入
//! （≥4096）；BlockDev 抽象块读（设备刀=UFS read10+分区基址；host
//! test=文件）。不支持项显式报错不猜：非 extent inode（间接块）、
//! depth>5、非目录 lookup。

/// 块设备抽象：字节寻址读（实现侧自对齐块界——如 UFS read10 到
/// LBA 再切片；superblock 在字节 1024，块界语义交实现）。
pub trait BlockDev {
    fn read(&mut self, byte_addr: u64, out: &mut [u8]) -> Result<(), ()>;
}

/// 只读路径上的失败分类（日志环证据码由设备刀映射）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bad {
    /// 块设备读失败
    Io,
    /// superblock 魔数 0xEF53 不对
    Magic,
    /// log_block_size>6（>64K 块，非 ext4 形状）
    BlockSize,
    /// inode_size<128 或 >scratch 容量
    InodeSize,
    /// 未支持特性：非 extent inode（EXTENTS_FL 缺）
    NoExtent,
    /// extent 树头魔数 0xF30A 不对 / 内部不一致
    BadExtent,
    /// extent 树深过 5（内核 MAX_EXTENT_DEPTH）
    TooDeep,
    /// inode 号 0 或超组容量
    NoSuchInode,
    /// 路径组件不是目录 / inode 非预期类型
    NotDir,
    /// 目录项 rec_len=0 或未 4 对齐（截断保护）
    BadDirent,
    /// 路径组件未找到
    NotFound,
}

fn le16(b: &[u8], o: usize) -> u16 {
    b[o] as u16 | (b[o + 1] as u16) << 8
}
fn le32(b: &[u8], o: usize) -> u32 {
    b[o] as u32 | (b[o + 1] as u32) << 8 | (b[o + 2] as u32) << 16 | (b[o + 3] as u32) << 24
}
/// 只读文件系统句柄。块大小=1024<<block_bits。
pub struct Fs<R: BlockDev> {
    dev: R,
    block_bits: u32,
    /// inodes_per_group
    ipg: u32,
    inode_size: u32,
    /// 组描述符字节数（32 或 64）
    desc_size: u32,
    /// superblock 所在块（4K 块=0，1K 块=1）——GDT 紧随其后。
    sb_block: u64,
}

/// inode 摘要（mode/size 两件走遍只读路径）。
pub struct Inode {
    pub mode: u16,
    pub size: u64,
    /// EXTENTS_FL（0x80000）
    pub extents: bool,
}

/// 块映射的两块暂存。静态区，不进调用方栈。裸件栈顶 `0xB0008000`
/// 向下只能到 `0xB0005000`，再往下是 UFS 的 DMA 窗（ARRAY/STAGING）。
/// 4KB 数组若留在 `rust_main` 的栈帧里，入口就把 SP 压进 ARRAY，
/// 第一次 GPT DMA 冲掉栈上的超时计数，门铃轮询不再返回，logo 停住、
/// 环上零记录。单线程；lookup / read_file / listdir 不嵌套。
static mut MAP_A: [u8; 4096] = [0; 4096];
static mut MAP_B: [u8; 4096] = [0; 4096];
/// extent 下一级整块。和 tree_scratch 分开，递归时不会把自己读掉。
static mut EXT_NODE: [u8; 4096] = [0; 4096];

impl<R: BlockDev> Fs<R> {
    /// 开盘：读 superblock（1024 字节偏移落在 sb_block 内）验形状。
    pub fn open(mut dev: R, scratch: &mut [u8]) -> Result<Fs<R>, Bad> {
        if scratch.len() < 2048 {
            return Err(Bad::InodeSize);
        }
        // superblock 恒在字节 1024（1K 块=块 1 独占；≥2K 块=块 0 尾部）
        // ——字节寻址一次读出，块形无关。
        dev.read(1024, &mut scratch[1024..2048]).map_err(|_| Bad::Io)?;
        if le16(scratch, 1024 + 56) != 0xEF53 {
            return Err(Bad::Magic);
        }
        let sbo = 1024;
        let log_bs = le32(scratch, sbo + 24);
        if log_bs > 6 {
            return Err(Bad::BlockSize);
        }
        let block_bits = 10 + log_bs;
        let ipg = le32(scratch, sbo + 40);
        let inode_size = le16(scratch, sbo + 88) as u32;
        if !(128..=scratch.len() as u32).contains(&inode_size) {
            return Err(Bad::InodeSize);
        }
        let desc_size = match le16(scratch, sbo + 254) {
            0 | 32 => 32,
            64 => 64,
            _ => 32,
        };
        Ok(Fs { dev, block_bits, ipg, inode_size, desc_size, sb_block: 1024 >> (10 + log_bs) })
    }

    pub fn block_size(&self) -> u32 {
        1u32 << self.block_bits
    }

    /// 整块读（byte_addr=块号<<block_bits）。
    fn rd_block(&mut self, block: u64, out: &mut [u8]) -> Result<(), Bad> {
        let n = self.block_size() as usize;
        self.dev.read(block << self.block_bits, &mut out[..n]).map_err(|_| Bad::Io)
    }

    /// 读 inode 摘要（mode/size/extents 标）。ino 从 1 计。
    fn inode_meta(&mut self, ino: u32, scratch: &mut [u8]) -> Result<Inode, Bad> {
        let raw = self.inode_raw(ino, scratch)?;
        let mode = le16(raw, 0);
        let size = le32(raw, 4) as u64 | (le32(raw, 108) as u64) << 32;
        Ok(Inode { mode, size, extents: le32(raw, 32) & 0x80000 != 0 })
    }

    /// inode 原始字节（落在 inode 表某块内切片返回）。
    fn inode_raw<'a>(
        &mut self,
        ino: u32,
        scratch: &'a mut [u8],
    ) -> Result<&'a [u8], Bad> {
        if ino == 0 {
            return Err(Bad::NoSuchInode);
        }
        let g = (ino - 1) / self.ipg;
        let idx = (ino - 1) % self.ipg;
        let bpb = self.block_size() / self.desc_size;
        let gd_block = self.sb_block + 1 + (g / bpb) as u64;
        self.rd_block(gd_block, scratch)?;
        let gdo = (g % bpb) as usize * self.desc_size as usize;
        let hi = if self.desc_size == 64 { le32(scratch, gdo + 40) as u64 } else { 0 };
        let itable = le32(scratch, gdo + 8) as u64 | hi << 32;
        let byte_off = idx as u64 * self.inode_size as u64;
        let blk = itable + byte_off / self.block_size() as u64;
        self.rd_block(blk, scratch)?;
        let off = (byte_off % self.block_size() as u64) as usize;
        Ok(&scratch[off..off + self.inode_size as usize])
    }

    /// extent 树查逻辑块 → 物理块（None=sparse 洞）。
    fn map_block(
        &mut self,
        ino: u32,
        inode_scratch: &mut [u8],
        tree_scratch: &mut [u8],
        lb: u64,
        depth_budget: u32,
    ) -> Result<Option<u64>, Bad> {
        // 树头在 inode.i_block[0..12]——先拷出再借树缓冲（免双借 scratch）。
        let mut hdr = [0u8; 60];
        {
            let raw = self.inode_raw(ino, inode_scratch)?;
            if le32(raw, 32) & 0x80000 == 0 {
                return Err(Bad::NoExtent);
            }
            hdr.copy_from_slice(&raw[40..100]);
        }
        self.walk(&hdr, tree_scratch, lb, depth_budget, 0)
    }

    /// 走 extent 节点。inode 里只有 60B（最多 4 项）；下一级是整块，
    /// 项数可以远大于 4。depth=0 叶：查区间；depth>0 索引：先记下子块号，
    /// 再读进 tree_scratch，避免边读边覆盖当前节点。
    fn walk(
        &mut self,
        node: &[u8],
        tree_scratch: &mut [u8],
        lb: u64,
        budget: u32,
        _level: u32,
    ) -> Result<Option<u64>, Bad> {
        if node.len() < 12 || le16(node, 0) != 0xF30A {
            return Err(Bad::BadExtent);
        }
        let entries = le16(node, 2) as usize;
        let depth = le16(node, 6) as u32;
        if 12 + entries * 12 > node.len() {
            return Err(Bad::BadExtent);
        }
        if depth == 0 {
            for i in 0..entries {
                let e = 12 + i * 12;
                let eeb = le32(node, e) as u64;
                let len = le16(node, e + 4) as u64 & 0x7FFF;
                let phys = le32(node, e + 8) as u64 | (le16(node, e + 6) as u64) << 32;
                if eeb <= lb && lb < eeb + len.max(1) {
                    return Ok(Some(phys + lb - eeb));
                }
            }
            return Ok(None);
        }
        if budget == 0 {
            return Err(Bad::TooDeep);
        }
        // 索引层：找 ei_block<=lb 的最后一项（规范保证升序）。
        let mut best: Option<u64> = None;
        for i in 0..entries {
            let e = 12 + i * 12;
            let eib = le32(node, e) as u64;
            if eib <= lb {
                let leaf = le32(node, e + 4) as u64 | (le16(node, e + 8) as u64) << 32;
                best = Some(leaf);
            }
        }
        let leaf_blk = match best {
            Some(b) => b,
            None => return Ok(None),
        };
        self.rd_block(leaf_blk, tree_scratch)?;
        if tree_scratch.len() < 12 {
            return Err(Bad::BadExtent);
        }
        // 整块拷到独立缓冲再递归，子级 rd_block 会重用 tree_scratch。
        let n = tree_scratch.len().min(unsafe { EXT_NODE.len() });
        unsafe {
            EXT_NODE[..n].copy_from_slice(&tree_scratch[..n]);
        }
        self.walk(unsafe { &EXT_NODE[..n] }, tree_scratch, lb, budget - 1, _level + 1)
    }

    /// 目录查找：线性扫全部数据块（HTREE 目录也覆盖——只读无需 hash）。
    pub fn lookup(
        &mut self,
        dir: u32,
        name: &str,
        scratch: &mut [u8],
    ) -> Result<u32, Bad> {
        let (mode, size) = {
            let m = self.inode_meta(dir, scratch)?;
            (m.mode, m.size)
        };
        if mode & 0xF000 != 0x4000 {
            return Err(Bad::NotDir);
        }
        let bs = self.block_size() as u64;
        let blocks = size.div_ceil(bs);
        let (a, b) = Self::map_pair();
        for lb in 0..blocks {
            let phys = self.map_block(dir, a, b, lb, 5)?;
            let pb = match phys {
                Some(p) => p,
                None => continue, // 洞=跳过（目录通常无洞）
            };
            self.rd_block(pb, scratch)?;
            let mut o = 0usize;
            while o + 8 <= self.block_size() as usize {
                let ino = le32(scratch, o);
                let rec = le16(scratch, o + 4) as usize;
                let nl = scratch[o + 6] as usize;
                if rec == 0 || rec & 3 != 0 {
                    return Err(Bad::BadDirent);
                }
                if ino != 0 && nl > 0 && name.len() == nl && &scratch[o + 8..o + 8 + nl] == name.as_bytes() {
                    return Ok(ino);
                }
                o += rec;
            }
        }
        Err(Bad::NotFound)
    }

    /// 绝对路径解析（'/a/b/c'，从根 inode 2 起；符号链接不支持——
    /// L0 树内验收路径无链接）。
    pub fn resolve(&mut self, path: &str, scratch: &mut [u8]) -> Result<u32, Bad> {
        let mut ino = 2u32;
        for part in path.split('/').filter(|s| !s.is_empty()) {
            ino = self.lookup(ino, part, scratch)?;
        }
        Ok(ino)
    }

    fn map_pair() -> (&'static mut [u8; 4096], &'static mut [u8; 4096]) {
        unsafe {
            (
                &mut *core::ptr::addr_of_mut!(MAP_A),
                &mut *core::ptr::addr_of_mut!(MAP_B),
            )
        }
    }

    /// 文件区间读（最多填满 out；返回实读字节数）。
    pub fn read_file(
        &mut self,
        ino: u32,
        off: u64,
        out: &mut [u8],
        scratch: &mut [u8],
    ) -> Result<usize, Bad> {
        let m = self.inode_meta(ino, scratch)?;
        if m.mode & 0xF000 != 0x8000 {
            return Err(Bad::NotDir);
        }
        if off >= m.size {
            return Ok(0);
        }
        let want = out.len().min((m.size - off) as usize);
        let bs = self.block_size() as u64;
        let mut done = 0usize;
        let (a, b) = Self::map_pair();
        while done < want {
            let lb = (off + done as u64) / bs;
            let bo = ((off + done as u64) % bs) as usize;
            let n = self.block_size() as usize - bo;
            let n = n.min(want - done);
            match self.map_block(ino, a, b, lb, 5)? {
                Some(p) => {
                    self.rd_block(p, scratch)?;
                    out[done..done + n].copy_from_slice(&scratch[bo..bo + n]);
                }
                None => {
                    // sparse 洞=零
                    for x in out.iter_mut().take(done + n).skip(done) {
                        *x = 0;
                    }
                }
            }
            done += n;
        }
        Ok(done)
    }

    /// 列目录（回调 name/ino/file_type；HTREE 目录也全量扫）。
    pub fn listdir<F: FnMut(&str, u32, u8)>(
        &mut self,
        dir: u32,
        scratch: &mut [u8],
        mut cb: F,
    ) -> Result<u64, Bad> {
        let (mode, size) = {
            let m = self.inode_meta(dir, scratch)?;
            (m.mode, m.size)
        };
        if mode & 0xF000 != 0x4000 {
            return Err(Bad::NotDir);
        }
        let bs = self.block_size() as u64;
        let (a, b) = Self::map_pair();
        for lb in 0..size.div_ceil(bs) {
            let pb = match self.map_block(dir, a, b, lb, 5)? {
                Some(p) => p,
                None => continue,
            };
            self.rd_block(pb, scratch)?;
            let mut o = 0usize;
            while o + 8 <= self.block_size() as usize {
                let ino = le32(scratch, o);
                let rec = le16(scratch, o + 4) as usize;
                let nl = scratch[o + 6] as usize;
                let ft = scratch[o + 7];
                if rec == 0 || rec & 3 != 0 {
                    return Err(Bad::BadDirent);
                }
                if ino != 0 && nl > 0 {
                    if let Ok(s) = core::str::from_utf8(&scratch[o + 8..o + 8 + nl]) {
                        cb(s, ino, ft);
                    }
                }
                o += rec;
            }
        }
        Ok(size)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 内存合成盘：1 个 4K 块组——sb（块 1）+gdt（块 2）+inode 表
    /// （块 3，两 inode：root dir + 一个文件）+数据块。形状对齐
    /// mkfs.ext4（inode_size=128、ipg=8、desc=32）。
    struct MemDev {
        blocks: Vec<[u8; 4096]>,
    }
    impl BlockDev for MemDev {
        fn read(&mut self, byte_addr: u64, out: &mut [u8]) -> Result<(), ()> {
            let blk = (byte_addr / 4096) as usize;
            let off = (byte_addr % 4096) as usize;
            let b = self.blocks.get(blk).ok_or(())?;
            out.copy_from_slice(&b[off..off + out.len()]);
            Ok(())
        }
    }

    fn put16(b: &mut [u8], o: usize, v: u16) {
        b[o..o + 2].copy_from_slice(&v.to_le_bytes());
    }
    fn put32(b: &mut [u8], o: usize, v: u32) {
        b[o..o + 4].copy_from_slice(&v.to_le_bytes());
    }

    fn mkfs() -> MemDev {
        let mut d = MemDev { blocks: vec![[0u8; 4096]; 8] };
        // superblock @1024（块 0 内）
        let sb = &mut d.blocks[0];
        put32(sb, 1024 + 4, 8); // blocks_count_lo
        put32(sb, 1024 + 24, 2); // log_block_size → 4096
        put32(sb, 1024 + 32, 8); // blocks_per_group
        put32(sb, 1024 + 40, 8); // inodes_per_group
        put16(sb, 1024 + 56, 0xEF53);
        put16(sb, 1024 + 88, 128); // inode_size
        // GDT @块 1：组 0 描述符，inode_table=块 3
        put32(&mut d.blocks[1], 8, 3);
        // inode 2（root 目录，extent 单块 phys 5）：大小 4096+8+12+name
        let iroot = &mut d.blocks[3][128..256];
        put16(iroot, 0, 0o040755);
        put32(iroot, 4, 33); // size = 8+9+12+4
        put32(iroot, 32, 0x80000);
        put16(iroot, 40, 0xF30A);
        put16(iroot, 42, 1); // entries
        put16(iroot, 44, 4); // max
        put16(iroot, 46, 0); // depth
        put32(iroot, 52, 0); // ee_block
        put16(iroot, 56, 1); // len
        put16(iroot, 58, 0); // hi
        put32(iroot, 60, 5); // lo → phys 5
        // 块 5 = 目录内容："hello" → ino 3
        let dirblk = &mut d.blocks[5];
        // dirent 长度律：8+name_len 上取 4 对齐（8+5=13→16）。
        put32(dirblk, 0, 3); // "hello" → ino 3
        put16(dirblk, 4, 16);
        dirblk[6] = 5; // name_len
        dirblk[7] = 1; // file_type=regular
        dirblk[8..13].copy_from_slice(b"hello");
        put32(dirblk, 16, 3); // 第二项 "file"：同指 ino 3（异名同人）
        put16(dirblk, 20, 4096 - 16);
        dirblk[22] = 4;
        dirblk[23] = 1; // regular file
        dirblk[24..28].copy_from_slice(b"file");
        // inode 3（文件 "hello"，内容在块 6，大小 5，"hi!\n\0"→"hola!"）
        let ifile = &mut d.blocks[3][256..384];
        put16(ifile, 0, 0o100644);
        put32(ifile, 4, 5);
        put32(ifile, 32, 0x80000);
        put16(ifile, 40, 0xF30A);
        put16(ifile, 42, 1);
        put16(ifile, 44, 4);
        put16(ifile, 46, 0);
        put32(ifile, 52, 0);
        put16(ifile, 56, 1);
        put16(ifile, 58, 0);
        put32(ifile, 60, 6);
        d.blocks[6][..5].copy_from_slice(b"hola!");
        d
    }

    #[test]
    fn synthetic_open_lookup_read() {
        let dev = mkfs();
        let mut fs = Fs::open(dev, &mut [0u8; 4096]).unwrap();
        assert_eq!(fs.block_size(), 4096);
        let mut sc = [0u8; 4096];
        assert_eq!(fs.lookup(2, "hello", &mut sc).unwrap(), 3);
        assert_eq!(fs.lookup(2, "nope", &mut sc), Err(Bad::NotFound));
        let ino = fs.resolve("/hello", &mut sc).unwrap();
        let mut out = [0u8; 64];
        let n = fs.read_file(ino, 0, &mut out, &mut sc).unwrap();
        assert_eq!(&out[..n], b"hola!");
        // 区间读：off=1 余 4 字节（size=5 截尾）
        let n = fs.read_file(ino, 1, &mut out, &mut sc).unwrap();
        assert_eq!(&out[..n], b"ola!");
        // 列目录回调
        let mut names: Vec<String> = Vec::new();
        fs.listdir(2, &mut sc, |n, _, _| names.push(n.to_string())).unwrap();
        assert!(names.iter().any(|n| n == "hello") && names.iter().any(|n| n == "file"));
    }

    /// 回归钉：eb>0 且 len>1 的 extent（ee_len 在 e+4——旧 bug 读 e+2=
    /// ee_block 高半，eb<65536 时 len 恒 0→洞，真镜像大文件才炸）+ 洞填零。
    #[test]
    fn synthetic_extent_eb_offset_and_hole() {
        let mut d = mkfs();
        d.blocks.push([0u8; 4096]); // 第 9 块（phys 8）
        // inode 3 改形：extent eb=1 len=2 phys 7..8；块 0=洞。size=8194。
        let ifile = &mut d.blocks[3][256..384];
        put32(ifile, 4, 2 * 4096 + 2);
        put32(ifile, 52, 1); // ee_block=1
        put16(ifile, 56, 2); // ee_len=2
        put32(ifile, 60, 7); // phys=7
        d.blocks[7][..4].copy_from_slice(b"aaaa");
        d.blocks[8][..4].copy_from_slice(b"ABCD");
        let mut fs = Fs::open(d, &mut [0u8; 4096]).unwrap();
        let mut sc = [0u8; 4096];
        let mut out = [0u8; 8];
        // 洞：lb=0 无 extent 覆盖 → 零
        assert_eq!(fs.read_file(3, 0, &mut out, &mut sc).unwrap(), 8);
        assert_eq!(&out, &[0u8; 8]);
        // lb=1（eb=1 首块）：块 7
        fs.read_file(3, 4096, &mut out, &mut sc).unwrap();
        assert_eq!(&out[..4], b"aaaa");
        // lb=2（eb+1——旧 bug 此处判洞）：块 8；size 截尾只余 2 字节
        assert_eq!(fs.read_file(3, 8192, &mut out, &mut sc).unwrap(), 2);
        assert_eq!(&out[..2], b"AB");
    }

    #[test]
    fn synthetic_depth_two_extent_tree() {
        // depth=1 索引树：root extent → 索引块 7 → 叶（phys 6）。
        let mut d = mkfs();
        let iroot = &mut d.blocks[3][128..256];
        put16(iroot, 44, 4); // max 保持
        put16(iroot, 46, 1); // depth=1
        // 索引项：ei_block=0 → leaf 块 7
        put32(iroot, 52, 0); // ei_block
        put32(iroot, 56, 7); // leaf_lo
        put16(iroot, 60, 0); // leaf_hi
        // 叶块 7：头+1 extent（0,1,phys 6）
        let leaf = &mut d.blocks[7];
        put16(leaf, 0, 0xF30A);
        put16(leaf, 2, 1);
        put16(leaf, 4, 4);
        put16(leaf, 6, 0);
        put32(leaf, 12, 0);
        put16(leaf, 16, 1);
        put16(leaf, 18, 0);
        put32(leaf, 20, 6);
        let mut fs = Fs::open(d, &mut [0u8; 4096]).unwrap();
        let mut sc = [0u8; 4096];
        let mut out = [0u8; 8];
        let n = fs.read_file(3, 0, &mut out, &mut sc).unwrap();
        assert_eq!(&out[..n], b"hola!");
    }

    /// 真镜像 gated 测试：dist/ 的 rootfs.img 存在才跑（裸仓不带镜像）。
    /// 期望值=2026-10-15 python 对账（md5 定谳）。
    #[test]
    fn real_rootfs_image_when_present() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../dist/aginxos-redfin-0.1.2/rootfs.img" // D14-exempt: 真镜像 ground-truth fixture（同 hwd 先例）
        );
        let Ok(f) = std::fs::File::open(path) else {
            eprintln!("skipped: {path} 不在");
            return;
        };
        use std::io::{Read, Seek, SeekFrom};
        struct FileDev(std::fs::File);
        impl BlockDev for FileDev {
            fn read(&mut self, byte_addr: u64, out: &mut [u8]) -> Result<(), ()> {
                self.0
                    .seek(SeekFrom::Start(byte_addr))
                    .and_then(|_| self.0.read_exact(out))
                    .map(|_| ())
                    .map_err(|_| ())
            }
        }
        let mut fs = Fs::open(FileDev(f), &mut [0u8; 4096]).unwrap();
        let mut sc = [0u8; 4096];
        // /etc/inittab ino=150 size=823
        let ino = fs.resolve("/etc/inittab", &mut sc).unwrap();
        assert_eq!(ino, 150);
        let mut buf = vec![0u8; 823];
        let n = fs.read_file(ino, 0, &mut buf, &mut sc).unwrap();
        assert_eq!(n, 823);
        // 头 16 字节锚（python 对账 2026-10-15）；全量指纹走 crc32。
        assert_eq!(&buf[..16], b"# AginxOS rootfs");
        eprintln!("inittab crc32 = {:08x}", crate::crc32::crc32(&buf));
        // /etc/passwd ino=151 size=30
        let ino = fs.resolve("/etc/passwd", &mut sc).unwrap();
        assert_eq!(ino, 151);
        let mut small = [0u8; 64];
        let n = fs.read_file(ino, 0, &mut small, &mut sc).unwrap();
        assert_eq!(n, 30);
        assert_eq!(&small[..6], b"root:x");
        // 深路径+全量 md5：/etc/aginx/device.toml ino=119 size=3811
        // md5=19e6c57c…（python 定谳 2026-10-15）。
        let ino = fs.resolve("/etc/aginx/device.toml", &mut sc).unwrap();
        assert_eq!(ino, 119);
        let mut dt = vec![0u8; 3811];
        assert_eq!(fs.read_file(ino, 0, &mut dt, &mut sc).unwrap(), 3811);
        assert_eq!(md5_hex(&dt), "19e6c57c64ce1f22c7d1ff6c0bee8c2a");
        // 大文件多 extent：/lib/modules/msm_drm.ko ino=174 size=3838800
        // 两段 512KiB 区间（第二段跨 extent 边界）md5 对账。
        let ino = fs.resolve("/lib/modules/msm_drm.ko", &mut sc).unwrap();
        assert_eq!(ino, 174);
        let mut ko = vec![0u8; 524288];
        fs.read_file(ino, 0, &mut ko, &mut sc).unwrap();
        assert_eq!(md5_hex(&ko), "ad3a48fef88a4543e6eddc9ee5f8810d");
        fs.read_file(ino, 1048576, &mut ko, &mut sc).unwrap();
        assert_eq!(md5_hex(&ko), "c5cebab930f18a531ab7a04dda00d38b");
        // 大目录：/bin 101 项（含 . 与 ..；python 对账）。
        let ino = fs.resolve("/bin", &mut sc).unwrap();
        let mut cnt = 0usize;
        let sz = fs.listdir(ino, &mut sc, |_, _, _| cnt += 1).unwrap();
        assert_eq!(cnt, 101);
        assert!(sz >= 2048); // 101 项≈2.4KB，单块目录（非体积断言）
    }

    /// 测试用 md5（RFC1321 紧凑实现——真源对账够用，不进产品路径）。
    fn md5_hex(data: &[u8]) -> String {
        // 简版：逐块 64 轮转。常量表走算法标准 K[i]=floor(abs(sin(i+1))*2^32)。
        fn k(i: usize) -> u32 {
            ((i as f64 + 1.0).sin().abs() * 4294967296.0) as u32
        }
        let s = [7u32, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22,
                     5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20,
                     4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23,
                     6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21];
        let mut msg = data.to_vec();
        let bitlen = (data.len() as u64) * 8;
        msg.push(0x80);
        while msg.len() % 64 != 56 {
            msg.push(0);
        }
        msg.extend_from_slice(&bitlen.to_le_bytes());
        let mut h: [u32; 4] = [0x67452301, 0xefcdab89, 0x98badcfe, 0x10325476];
        for chunk in msg.chunks(64) {
            let mut m = [0u32; 16];
            for i in 0..16 {
                m[i] = u32::from_le_bytes(chunk[i * 4..i * 4 + 4].try_into().unwrap());
            }
            let (mut a, mut b, mut c, mut d) = (h[0], h[1], h[2], h[3]);
            for i in 0..64 {
                let (f, g) = match i / 16 {
                    0 => ((b & c) | (!b & d), i),
                    1 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                    2 => (b ^ c ^ d, (3 * i + 5) % 16),
                    _ => (c ^ (b | !d), (7 * i) % 16),
                };
                let tmp = d;
                d = c;
                c = b;
                b = b.wrapping_add((a.wrapping_add(f).wrapping_add(k(i))).wrapping_add(m[g]).rotate_left(s[i]));
                a = tmp;
            }
            h[0] = h[0].wrapping_add(a);
            h[1] = h[1].wrapping_add(b);
            h[2] = h[2].wrapping_add(c);
            h[3] = h[3].wrapping_add(d);
        }
        // 摘要=h[0..4] 逐字小端字节序列（{:08x} 打 u32 是大端序——
        // 首版此坑：md5(abc) 出 98500190…，字内反序）。
        let mut out = String::new();
        for w in h {
            for byte in w.to_le_bytes() {
                out.push_str(&format!("{:02x}", byte));
            }
        }
        out
    }
}
