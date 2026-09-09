# aginx-ocr

离线文字识别 CLI（PP-OCRv5 mobile int8，冻结 bionic-static 件）。图片
路径进 → stdout `text<TAB>conf` 每行；rot-auto 竖握常态（M45 收据：
auto 两轮 det + rec）。模型三件随包走，provision 幂等 symlink
/var/models/ocr（C9）。

## 验证

- `aginx-voice` 念读一轮出文本
- /var/models/ocr/{det,rec}.onnx 在位（symlink 跟随进树）

## 回滚

`aginx-pkg rollback aginx-ocr`；无 .prev（首装）时 `sync` 重装即回。
