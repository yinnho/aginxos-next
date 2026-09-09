# aginx-tts

离线语音合成 CLI（vits-melo-tts-zh_en，冻结 bionic-static 件）。文本 →
WAV（0x7fffffff 哨兵语义，M42a）；fp32 model.onnx 是真权重（tarball 的
int8 是 133B lfs 指针，M42e 收据）。模型树随包走，provision 幂等
symlink /var/models/tts/vits-melo-tts-zh_en（C9）。

## 验证

- `aginx-voice --say 你好` 出声
- `test -d /var/models/tts/vits-melo-tts-zh_en`（symlink 跟随进树）

## 回滚

`aginx-pkg rollback aginx-tts`；无 .prev（首装）时 `sync` 重装即回。
