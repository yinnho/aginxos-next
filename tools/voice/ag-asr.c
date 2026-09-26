// ag-asr — M42d 本地语音识别 CLI（bionic-static，voiced 子进程调用）。
// 用法: ag-asr <wav>      一次性：识别一个文件退出
//       ag-asr --serve    常驻（M42e）：stdin 一行 wav 路径一次识别，stdout
//                         回 "OK <text>" / "ERR <why>"；EOF 退出。模型只加载
//                         一次（装载 ~2s，M42e 收据）。
// 出:   一次性 stdout 一行识别文本；失败 exit 1。模型固定 /var/models/asr
//       （AG_ASR_DIR 覆盖）。
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "c-api.h"

// 识别一个 wav 路径；成功时打印 OK 行。返回 0 失败非 0。
static int recognize(const SherpaOnnxOfflineRecognizer *rec, const char *path) {
    const SherpaOnnxWave *wave = SherpaOnnxReadWave(path);
    if (!wave) { printf("ERR read\n"); fflush(stdout); return 1; }

    const SherpaOnnxOfflineStream *s = SherpaOnnxCreateOfflineStream(rec);
    SherpaOnnxAcceptWaveformOffline(s, wave->sample_rate, wave->samples, wave->num_samples);
    SherpaOnnxDecodeOfflineStream(rec, s);
    const SherpaOnnxOfflineRecognizerResult *r = SherpaOnnxGetOfflineStreamResult(s);
    printf("OK %s\n", r && r->text ? r->text : "");
    fflush(stdout);
    if (r) SherpaOnnxDestroyOfflineRecognizerResult(r);
    SherpaOnnxDestroyOfflineStream(s);
    SherpaOnnxFreeWave(wave);
    return 0;
}

int main(int argc, char **argv) {
    int serve = argc >= 2 && strcmp(argv[1], "--serve") == 0;
    if (argc < 2 || (!serve && argc > 2)) {
        fprintf(stderr, "usage: ag-asr <wav> | ag-asr --serve\n");
        return 2;
    }
    const char *dir = getenv("AG_ASR_DIR");
    if (!dir || !*dir) dir = "/var/models/asr";
    char model[512], tokens[512];
    snprintf(model, sizeof model, "%s/model.int8.onnx", dir);
    snprintf(tokens, sizeof tokens, "%s/tokens.txt", dir);

    SherpaOnnxOfflineRecognizerConfig config;
    memset(&config, 0, sizeof config);
    config.feat_config.sample_rate = 16000;
    config.feat_config.feature_dim = 80;
    config.model_config.sense_voice.model = model;
    /* auto 把安静中文听成 Oh/The/I（enchilada 2026-09-18）。产品是中文机。
     * AG_ASR_LANG 覆盖：zh / en / ja / ko / yue / auto。 */
    {
        const char *lang = getenv("AG_ASR_LANG");
        config.model_config.sense_voice.language =
            (lang && *lang) ? lang : "zh";
    }
    config.model_config.sense_voice.use_itn = 1;
    config.model_config.tokens = tokens;
    config.model_config.num_threads = 2;
    config.model_config.provider = "cpu";
    config.decoding_method = "greedy_search";

    const SherpaOnnxOfflineRecognizer *rec = SherpaOnnxCreateOfflineRecognizer(&config);
    if (!rec) { fprintf(stderr, "ag-asr: create recognizer failed\n"); return 1; }

    if (serve) {
        char line[1024];
        while (fgets(line, sizeof line, stdin)) {
            line[strcspn(line, "\r\n")] = 0;
            if (!line[0]) { printf("ERR empty\n"); fflush(stdout); continue; }
            recognize(rec, line);
        }
        SherpaOnnxDestroyOfflineRecognizer(rec);
        return 0; // stdin EOF — 宿主退了，别变僵尸
    }

    const SherpaOnnxWave *wave = SherpaOnnxReadWave(argv[1]);
    if (!wave) { fprintf(stderr, "ag-asr: read wave failed: %s\n", argv[1]); return 1; }

    const SherpaOnnxOfflineStream *s = SherpaOnnxCreateOfflineStream(rec);
    SherpaOnnxAcceptWaveformOffline(s, wave->sample_rate, wave->samples, wave->num_samples);
    SherpaOnnxDecodeOfflineStream(rec, s);
    const SherpaOnnxOfflineRecognizerResult *r = SherpaOnnxGetOfflineStreamResult(s);
    printf("%s\n", r && r->text ? r->text : "");
    if (r) SherpaOnnxDestroyOfflineRecognizerResult(r);
    SherpaOnnxDestroyOfflineStream(s);
    SherpaOnnxFreeWave(wave);
    SherpaOnnxDestroyOfflineRecognizer(rec);
    return 0;
}
