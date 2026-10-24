---
name: bash
description: bash 5.2.37 shell（claude 引擎 Bash 工具腿+dev CLI 第五件）
---

bash v5.2.37（Alpine v3.22 整树，face /var/bin/bash）。裸树只有 busybox
ash，claude 引擎的 Bash 工具腿硬找 bash——本包即其依赖。

```sh
/var/bin/bash -c 'uname -m'              # 非登录 PATH 用绝对路径
echo 'set -e; uname -m' | /var/bin/bash  # 脚本管道
```

- claude spawn env 带到它找得到时 Bash 工具即活（deps=bash 随 claude 装）。
- terminfo 在树内（TERMINFO_DIRS 由 wrapper 自带），交互编辑可用。
