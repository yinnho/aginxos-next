# aginx-file

文件面工具 CLI（D13 改姓自 `agf`，2026-09-26）。双消费面：

- **母体桥**：runtime 的 agf_bridge spawn `aginx-file tool <name>`，
  stdin 喂入参 JSON（含 `_ctx` 身份与预解析路径），stdout 收 D1 信封
  （`{"ok":true,"data":…}`）。工具名 file_read/file_write/file_list/
  file_convert/image_analyze——flow 冻结，不随包改名。
- **人面 CLI**：`aginx-file read/write/ls/convert <path>`（router `files` 组）。

缺包时母体的文件工具全部报错（stuck 断判会在 6 轮全失败后终止）；
本包是 agent 读文件的地板依赖之一。

## 验证

- `aginx-file read /etc/hostname` 出内容
- `ag agent send me '读 /etc/hostname'` 走通（桥spawn 真身）
- `ls /var/lib/aginx/stamps` 见 aginx-file

## 回滚

`aginx-pkg rollback aginx-file`
