# aginx-asr

离线语音识别 CLI（sherpa-onnx zipformer int8，冻结 bionic-static 件）。
stdin/argv WAV → stdout 文本；模型树随包走（files/models/asr），装机后
provision 幂等 symlink /var/models/asr → pkgfiles 树（C9）。

## 验证

- `aginx-voice --hear <wav>` 出文本
- `test -d /var/models/asr`（symlink 跟随进树）

## 回滚

`aginx-pkg rollback aginx-asr`；无 .prev（首装）时 `sync` 重装即回。
