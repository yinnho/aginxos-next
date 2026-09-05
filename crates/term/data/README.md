# aginx-term pinyin table (M40 aterm)

`pinyin.tsv` — one line per syllable, tab-separated: toneless syllable,
then up to 12 candidate hanzi pre-ranked by frequency (most frequent
first). 413 syllables ≈ 14 KB, embedded into the aginx-term binary via
`include_bytes!` (../src/pinyin.rs) — the 拼 key always has its data, no
runtime file, no fallback path.

Typing convention: ü is `v` (`nv` 女, `lv` 绿, `nve` 虐) — nü/lü stay
distinct from nu/lu; after j/q/x/y ü is spelled `u` (standard pinyin).

## Provenance

Derived on the host by `scripts/gen-pinyin-table.sh` (deterministic given
its two inputs; upstream files stay in /tmp, never committed):

- Readings: [mozillazg/pinyin-data](https://github.com/mozillazg/pinyin-data)
  `pinyin.txt` (MIT) — hanzi → pinyin readings; tones stripped (NFD),
  ü → v.
- Ranking: Jun Da *Modern Chinese Character Frequency List*
  (mandarintools.com / lingua.mtsu.edu) — rank order per syllable;
  characters absent from the list rank last.
- Coverage constraint: GB2312 level 1+2 hanzi only — the exact set
  `scripts/subset-cjk-font.sh` baked into `agterm-cjk.otf`, because the
  font is what can actually render a candidate.

Regenerate: `./scripts/gen-pinyin-table.sh` (writes this directory).
