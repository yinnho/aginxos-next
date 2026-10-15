# 无 Linux 内核全路线方案（2026-10-15 立，待用户审）

## 0. 目标与方法

- **目标**：redfin 日常运行面不跑 Linux 内核——开机后由我方代码（Rust `no_std`
  + 少量汇编）直接驱动硬件，最终承载 aginx 对外服务。
- **方法**：Linux 内核 / vendor 模块 / bootloader = 「已把这台机器驱动起来的参考
  答案」。逐个驱动拆成寄存器级配方，用我方代码重写；**Linux 只当说明书，不当
  运行时**。
  - 有源码的（ufshcd、spmi-pmic-arb、smb5、qpnp-qg、dwc3、qcacld 等）→ 读源码对账；
  - 无源码的（bootloader 交接残态、闭源 vendor .ko、固件接口）→ REA/Ghidra 逆向。
    先例：UFSDxe 逆向揪出「交接留 PowerDown、须 SSU ACTIVE」（#492）。
- **参照源**：`.local/device/redfin/kernel-src/redbull/`（LineageOS
  android_kernel_google_redbull @2af530696，10-15 自 /tmp 迁入；与在役
  4.19.278-ab10812814 同谱系**非逐字节同源**——闭源/差异处以二进制逆向为准）；
  `.local/device/redfin/vendor-modules/`（300+ .ko）；factory bootloader（FBPK→xbl/abl）。

## 1. 阶段总表

| 阶段 | 内容 | 验收 |
|---|---|---|
| **S0 底座骨架** | Rust no_std 化；异常向量；UFS 日志环；MMU+cache；GIC+定时器中断；PSCI 多核 | 见 §2 逐刀 |
| **S1 电源完整** | ka v3（喂狗经 MISC APID、解暂停 0x1043）；充电控制；电量计（aginx-fg 算法搬入）；温控。**S1-1 实测新增前置：Type-C/TCPM 面未起则 USBIN 不在位（TRUTH §12）——喂狗/解暂停都在其后** | 常驻 ≥1 天电量不降；日志里 0x1006=TAPER/FULLON |
| **S2 存储完整** | UFS 多 LUN/错误恢复/链路功耗；ext4 只读 | 裸件读出 userdata 指定文件与 L0 一致 |
| **S3 USB 网络** | dwc3 设备模式 → CDC-NCM → smoltcp TCP/IP → 控制通道 | Mac 一根线 ping 通、连上控制口 |
| **S4 Wi-Fi** | PIL（TZ 认证加载 WLAN 固件到 MSA）→ QRTR/GLINK+QMI（WLFW 握手）→ 802.11/WPA | 裸件连 AP、ping 通；**体量最大，周~月级** |
| **S5 跑 aginx** | Linux syscall 兼容层（最小子集：文件/socket/线程/时间），musl 静态二进制原样跑——守「OS 只装配、四项目独立」 | aginx 对外服务、relay 往返通 |

次序依据记忆「电→存储→网络→ssh/relay→脑」；**S0 先行**——无异常向量与日志，
后续每刀只能靠时延仪器猜（10-14 夜 8 轮中 2 轮栽在自家寄存器踩踏上）。

## 2. S0 详细设计

### 2.1 已核实的机况（不再假设）

| 事实 | 值 | 出处 |
|---|---|---|
| 入口/落点 | 0xA7E80000，入口=镜像+0，**x0=DTB 指针** | TRUTH.md §1（ABL 逆向） |
| 载荷格式 | LZ4 frame（BD=0x70）/gzip/raw arm64 Image（text_offset 0x80000） | TRUTH.md §1 |
| 交接态 | EL1、MMU/D-cache off（非缓存世界，DMA 免维护） | TRUTH §7、u 系实测 |
| GIC | GICv3 @0x17a00000 | DT `interrupt-controller@17a00000` |
| PSCI | 1.0，`method="smc"`，8 核 `enable-method="psci"` | DT |
| 计时 | CNTVCT/CNTFRQ EL1 可读未陷 | u3g 起实测 |
| 可写 scratch | boot_a 内 LBA 25096 起（校验后 GPT 名查 start+16384、界检） | #496 |

### 2.2 代码布局（守 D14：机器事实只在 devices/）

- `crates/bare/`（平台，零机器字符串）：入口/向量/MMU/GICv3/PSCI/定时器/UFSHCI
  通用驱动/SPMI arb v5 通用驱动/GPT/日志环格式——全部参数化。
- `devices/redfin/bare/`（机器数据）：`device.toml` 增 `[bare]` 段（各 MMIO 基址、
  PPID、scratch 规则）→ 构建期 `build.rs` 生成 Rust 常量模块；运行期再用 x0 DTB
  交叉核对（不一致→日志报警）。
- 打包沿用 `build-bare.sh`（zig/LZ4/boot 头/kl4 回环自检）；目标
  `aarch64-unknown-none`，链接基址 0xA7E80000。
- 全局寄存器律 → Rust 下由编译器管；汇编只留入口与向量桩。

### 2.3 逐刀（每刀一个可刷探针+验收）

| 刀 | 内容 | 验收（仪器） |
|---|---|---|
| **S0-1** | Rust 入口复刻 u4j（SSU 唤醒+GPT 双 CRC），证明工具链/链接/打包 | 时延码 2s 过（老仪器，最后一次主用） |
| **S0-2** | **UFS 日志环**：boot_a scratch 起 256 块环形 4K 记录（魔数/序号/戳/级别/文本）；L0 侧只读解析脚本 | 常驻态记录序号连续增长，L0 解析出文本 |
| **S0-3** | **异常向量**：同步/SError/IRQ 桩 → 把 ESR/ELR/FAR/CurrentEL 记进日志环后复位 | 故意触发非对齐访问 → L0 读出 ESR=对齐错误；顺带定谳 EL（挂账「EL 判定」闭） |
| **S0-4** | **MMU+cache**：恒等映射，RAM=Normal WB、MMIO=Device-nGnRnE；DMA 区单独非缓存（或照 UFSDxe 显式 cache 维护） | 开 cache 后 GPT 双 CRC 仍中；日志记录读盘耗时对比 |
| **S0-5** | **GICv3+通用定时器中断**：忙等→WFI | 日志心跳按定时器节拍；功耗刀前置 |
| **S0-6** | **PSCI CPU_ON** 拉起副核 | 7 个副核各写心跳进日志 |

S0-2 落地后时延编码仪器退役（UFS 起来之前的早期失败仍用它兜底）。

### 2.4 风险

- EL 未定谳（EL1 推定）：影响 GIC 用物理/虚拟接口、PSCI 走 SMC 还是 HVC——
  S0-3 先读 CurrentEL 定谳再做 S0-5/6。
- 开 cache 后 DMA 一致性：UFSDxe 开 cache 跑、带显式维护（FUN_133dc）——照抄策略。
- 日志环写盘频率与 UFS 寿命：常驻态限速（如 ≥10s 一条非紧急记录）。
- 每次上机仍是「刷 boot_a→验→回 L0」环；L0 保持救生艇不动。

## 3. 待用户裁决（已按推荐默认）

1. 「自己的脚本」= Rust `no_std` + 少量汇编（默认）。
2. 对外通道先 USB 网络（S3），Wi-Fi（S4）后补（默认）。
3. S5 走 Linux syscall 兼容层（默认）。
