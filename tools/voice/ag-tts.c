// ag-tts — M42d 本地语音合成 CLI（bionic-static）。
// 用法: ag-tts <text> [out.wav]   一次性：合成一句退出，stdout 打印 wav 路径
//       ag-tts --serve [out.wav]  常驻（M42e）：stdin 一行文本一次合成，写
//                                  out.wav 后 stdout 回一行 "OK <rate> <n>"
//                                  / "ERR <why>"；EOF 退出。模型只加载一次
//                                  ——装载 ~3.8s 是短句延迟的大头（M42e 收据）。
// 模型 /var/models/tts/ 下三种，AG_TTS_KIND 选（默认 vits，M42e 拍板 melo
// 为产品嘴——快 5 倍且中英混排全念；kokoro 的 zh 前端整词吞 Latin 是实测
// 收据：念「AginxOS 2026 TEL 138」只剩数字出声，2026-09-04）：
//   vits    vits-melo-tts-zh_en（快，中英混——默认）
//   kokoro  kokoro-int8-multi-lang-v1_1（质量好，慢，zh 前端吞 Latin 词；
//           AG_TTS_SID 默认 8=中文女声）
//   matcha  matcha-icefall-zh-baker（快，纯中文）
//   AG_TTS_DIR 仍可整目录覆盖（此时 KIND 由 env 定，路径按 kind 缺省拼）。
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "c-api.h"

int main(int argc, char **argv) {
    int serve = argc >= 2 && strcmp(argv[1], "--serve") == 0;
    if (argc < 2 || (serve && argc > 3) || (!serve && argc > 3)) {
        fprintf(stderr, "usage: ag-tts <text> [out.wav] | ag-tts --serve [out.wav]\n");
        return 2;
    }
    const char *kind_s = getenv("AG_TTS_KIND");
    // 默认 vits（melo）：/etc/aginx/env 只到 unit，adb 直呼和 fresh boot
    // 没有 env——默认必须就是产品嘴，否则混排文本的 Latin 词被 kokoro 吞。
    const char *kind = kind_s && *kind_s ? kind_s : "vits";
    const char *defdir = !strcmp(kind, "vits") ? "/var/models/tts/vits-melo-tts-zh_en"
                      : !strcmp(kind, "matcha") ? "/var/models/tts/matcha-icefall-zh-baker"
                      : "/var/models/tts/kokoro-int8-multi-lang-v1_1";
    const char *dir = getenv("AG_TTS_DIR");
    if (!dir || !*dir) dir = defdir;
    const char *sid_s = getenv("AG_TTS_SID");
    int sid = sid_s && *sid_s ? atoi(sid_s) : 8;
    // 语速 0.9（用户收据 2026-09-04：1.2× 太快）——M42e 压延迟时提到 1.2×，
    // melo 常驻后延迟已不是瓶颈，听感优先；AG_TTS_SPEED 可覆盖。
    const char *speed_s = getenv("AG_TTS_SPEED");
    float speed = speed_s && *speed_s ? strtof(speed_s, NULL) : 0.9f;
    const char *out = serve
        ? (argc > 2 ? argv[2] : "/tmp/ag-tts-serve.wav")
        : (argc > 2 ? argv[2] : "/tmp/ag-tts-out.wav");

    char model[512], voices[512], tokens[512], espeak[512], dictd[512], lex[512], fsts[768], fars[512], vocoder[512];
    snprintf(model, sizeof model, "%s/model.int8.onnx", dir);
    snprintf(voices, sizeof voices, "%s/voices.bin", dir);
    snprintf(tokens, sizeof tokens, "%s/tokens.txt", dir);
    snprintf(espeak, sizeof espeak, "%s/espeak-ng-data", dir);
    snprintf(dictd, sizeof dictd, "%s/dict", dir);
    snprintf(lex, sizeof lex, "%s/lexicon.txt", dir);
    snprintf(fsts, sizeof fsts, "%s/number-zh.fst,%s/date-zh.fst,%s/phone-zh.fst", dir, dir, dir);
    snprintf(vocoder, sizeof vocoder, "%s/hifigan_v2.onnx", dir);

    SherpaOnnxOfflineTtsConfig config;
    memset(&config, 0, sizeof config);
    // A76 钉核下默认 2 线程=2 核；AG_TTS_THREADS 可加宽（M42e 速度实验）。
    const char *thr_s = getenv("AG_TTS_THREADS");
    config.model.num_threads = thr_s && *thr_s ? atoi(thr_s) : 2;
    if (!strcmp(kind, "vits")) {
        // melo: tarball 里的 model.int8.onnx 是 133B 的 git-lfs 指针（release
        // 打包事故），真身只有 fp32 model.onnx（170MB）——vits 本来就小，
        // fp32 也远快于 kokoro。前端 MeloTtsLexicon 只要 lexicon+tokens 自足
        // （vits 不读 dict_dir）。陷阱（M42e 设备收据）：vits 绝不能设
        // data_dir——impl 里 AddBlank 隔位插 0 被 data_dir 非空门住跳过，
        // 序列错乱出怪音。规则用 melo 自带三 fst + new_heteronym.fst。
        snprintf(model, sizeof model, "%s/model.onnx", dir);
        snprintf(fsts, sizeof fsts, "%s/number.fst,%s/date.fst,%s/phone.fst", dir, dir, dir);
        snprintf(fars, sizeof fars, "%s/new_heteronym.fst", dir);
        config.model.vits.model = model;
        config.model.vits.lexicon = lex;
        config.model.vits.tokens = tokens;
        config.model.vits.noise_scale = 0.667f;
        config.model.vits.noise_scale_w = 0.8f;
        config.model.vits.length_scale = 1.0f;
        config.rule_fsts = fsts;
        config.rule_fars = fars;
    } else if (!strcmp(kind, "matcha")) {
        snprintf(model, sizeof model, "%s/model-steps-3.onnx", dir);
        config.model.matcha.acoustic_model = model;
        config.model.matcha.vocoder = vocoder;
        config.model.matcha.lexicon = lex;
        config.model.matcha.tokens = tokens;
        config.model.matcha.data_dir = espeak;
        config.model.matcha.dict_dir = dictd;
        config.model.matcha.noise_scale = 0.667f;
        config.model.matcha.length_scale = 1.0f;
    } else {
        // kokoro（lexicon 用 -zh 变体）
        snprintf(lex, sizeof lex, "%s/lexicon-zh.txt", dir);
        config.model.kokoro.model = model;
        config.model.kokoro.voices = voices;
        config.model.kokoro.tokens = tokens;
        config.model.kokoro.data_dir = espeak;
        config.model.kokoro.dict_dir = dictd;
        config.model.kokoro.lexicon = lex;
        config.model.kokoro.length_scale = 1.0f;
        config.model.kokoro.lang = "zh";
        config.rule_fsts = fsts;
    }
    config.max_num_sentences = 1;

    const SherpaOnnxOfflineTts *tts = SherpaOnnxCreateOfflineTts(&config);
    if (!tts) { fprintf(stderr, "ag-tts: create tts failed\n"); return 1; }

    SherpaOnnxGenerationConfig gen;
    memset(&gen, 0, sizeof gen);
    gen.sid = sid;
    gen.speed = speed;

    if (serve) {
        char line[4096];
        while (fgets(line, sizeof line, stdin)) {
            line[strcspn(line, "\r\n")] = 0;
            if (!line[0]) { printf("ERR empty\n"); fflush(stdout); continue; }
            const SherpaOnnxGeneratedAudio *audio =
                SherpaOnnxOfflineTtsGenerateWithConfig(tts, line, &gen, NULL, NULL);
            if (!audio || audio->n == 0) {
                if (audio) SherpaOnnxDestroyOfflineTtsGeneratedAudio(audio);
                printf("ERR generate\n"); fflush(stdout); continue;
            }
            if (!SherpaOnnxWriteWave(audio->samples, audio->n, audio->sample_rate, out)) {
                printf("ERR write\n"); fflush(stdout);
            } else {
                printf("OK %d %d\n", audio->sample_rate, audio->n); fflush(stdout);
            }
            SherpaOnnxDestroyOfflineTtsGeneratedAudio(audio);
        }
        SherpaOnnxDestroyOfflineTts(tts);
        return 0; // stdin EOF — 宿主退了，别变僵尸
    }

    const SherpaOnnxGeneratedAudio *audio =
        SherpaOnnxOfflineTtsGenerateWithConfig(tts, argv[1], &gen, NULL, NULL);
    if (!audio || audio->n == 0) { fprintf(stderr, "ag-tts: generate failed\n"); return 1; }

    if (!SherpaOnnxWriteWave(audio->samples, audio->n, audio->sample_rate, out)) {
        fprintf(stderr, "ag-tts: write failed: %s\n", out);
        return 1;
    }
    printf("%s\n", out);
    SherpaOnnxDestroyOfflineTtsGeneratedAudio(audio);
    SherpaOnnxDestroyOfflineTts(tts);
    return 0;
}
