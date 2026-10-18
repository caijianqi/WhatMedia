#!/usr/bin/env bash
# 转码性能基准。
#
# 目的不是"跑个分"，是给后续每次 release 一个**可比较的锚点**：
# 退化 > 10% 就阻断（DEVELOPMENT-PLAN §八）。所以口径必须写死在脚本里，
# 不能靠人记着"上次好像用的是 veryfast"。
#
# 用法：
#   scripts/bench.sh                          # 默认 20s 素材，输出到 stdout
#   scripts/bench.sh --json out.json          # 机器可读，便于入库对比
#   DURATION=60 scripts/bench.sh              # 更长素材，数字更稳
#
# 依赖：ffmpeg / ffprobe（FFMPEG= 可指定路径）。
# 缺 libx265 时 HDR10 用例会跳过而不是失败 —— 基准表留空比填假数字好。

set -euo pipefail

FFMPEG="${FFMPEG:-ffmpeg}"
FFPROBE="${FFPROBE:-ffprobe}"
DURATION="${DURATION:-20}"
WORK="${WORK:-$(mktemp -d)}"
JSON_OUT=""

while [ $# -gt 0 ]; do
  case "$1" in
    --json) JSON_OUT="$2"; shift 2;;
    *) echo "未知参数 $1" >&2; exit 2;;
  esac
done

mkdir -p "$WORK"
command -v "$FFMPEG" >/dev/null || { echo "找不到 ffmpeg" >&2; exit 1; }

CHAIN='zscale=t=linear:npl=100,format=gbrpf32le,zscale=p=bt709,tonemap=linear:desat=0,zscale=t=bt709:m=bt709:r=tv,format=yuv420p'

rows=()          # "用例|结果|倍速|秒"
has_encoder() { "$FFMPEG" -hide_banner -encoders 2>/dev/null | grep -qE "[[:space:]]$1[[:space:]]"; }

now() { date +%s%N; }

# 跑一条 ffmpeg 命令，记录耗时。结果写进 rows。
# $1=用例名，$2..=完整 ffmpeg 参数（**调用方自己写 `-i`**）。
#
# 为什么不在这里固定 `-i $src`：硬件解码参数（`-hwaccel`）必须在 `-i` 之前，
# 编码器参数必须在之后 —— 顺序即正确性（见 crates/transcoder/src/args.rs）。
# 把参数整体交给调用方，基准跑的就是生产代码同一种排布。
bench() {
  local name="$1"; shift
  if [ "$#" -eq 0 ]; then rows+=("$name|跳过||"); return; fi
  local t0 t1 rc=0 err="$WORK/.err.$$"
  t0=$(now)
  "$FFMPEG" -hide_banner -loglevel error -y "$@" >/dev/null 2>"$err" || rc=$?
  t1=$(now)
  if [ "$rc" -ne 0 ]; then
    # 区分"片源/参数有问题"与"硬件不可用"。后者正是降级链要接住的场景，
    # 基准表里把它标出来，比笼统写个"失败"有用得多。
    if grep -qiE 'device creation failed|cannot load|failed to initialise|no such device' "$err"; then
      rows+=("$name|硬件不可用（降级场景）||")
    else
      rows+=("$name|失败（exit $rc）||")
    fi
    rm -f "$err"
    return
  fi
  rm -f "$err"
  local secs
  secs=$(awk -v a="$t0" -v b="$t1" 'BEGIN{printf "%.2f", (b-a)/1000000000}')
  local speed
  speed=$(awk -v d="$DURATION" -v s="$secs" 'BEGIN{printf "%.2f", (s>0? d/s : 0)}')
  rows+=("$name|完成|${speed}x|${secs}s")
}

say() { printf '%s\n' "$*"; }

say "素材时长  ${DURATION}s"
say "工作目录  $WORK"
say ""

# ── 素材 ────────────────────────────────────────────────────────
# 合成素材而不是下载样本：任何人任何架构都能跑出同一口径，基准才可比。
say "生成素材…"
# 只要视频：转码基准测的是编码吞吐，音轨只会给结果掺入无关变量。
"$FFMPEG" -hide_banner -loglevel error -y -f lavfi \
  -i "testsrc2=size=1920x1080:rate=24:duration=$DURATION" \
  -c:v libx264 -preset ultrafast -pix_fmt yuv420p "$WORK/src_1080p_h264.mp4"
say "  1080p H264 ✓"

if has_encoder libx265; then
  "$FFMPEG" -hide_banner -loglevel error -y -f lavfi \
    -i "testsrc2=size=3840x2160:rate=24:duration=$DURATION" \
    -c:v libx265 -preset ultrafast -x265-params "hdr-opt=1:repeat-headers=1:colorprim=bt2020:transfer=smpte2084:colormatrix=bt2020nc" \
    -pix_fmt yuv420p10le -tag:v hvc1 "$WORK/src_4k_hevc10_hdr10.mp4"
  say "  4K HEVC 10bit HDR10 ✓"
else
  say "  4K HEVC 10bit HDR10 ✗（无 libx265，HDR 用例跳过）"
fi

say ""
say "跑基准…"

# ── 用例 ────────────────────────────────────────────────────────
S1080="$WORK/src_1080p_h264.mp4"
S4K="$WORK/src_4k_hevc10_hdr10.mp4"

# 素材不存在时整体跳过（缺 libx265 的机器上 4K HDR 用例自动消失）。
if [ -f "$S1080" ]; then
  bench "Remux 1080p H264 → TS（不重编码）" \
-i "$S1080" -map 0:v:0 -c copy -f mpegts pipe:1

  bench "软转 1080p → 1080p H264 (veryfast/crf23)" \
    -i "$S1080" -map 0:v:0 -c:v libx264 -preset veryfast -crf 23 -an -f null -

  bench "软转 1080p → 720p H264 + 缩帧" \
    -i "$S1080" -map 0:v:0 -vf scale=1280:720 -c:v libx264 -preset veryfast -crf 23 -an -f null -

  if has_encoder h264_qsv; then
    # `-hwaccel qsv` 必须在 `-i` 之前 —— 写反了 ffmpeg 会直接报
    # "cannot be applied to output url"，这正是生产代码里最容易踩的那一条。
    bench "硬转 QSV 1080p → 1080p H264" \
      -hwaccel qsv -i "$S1080" -map 0:v:0 -c:v h264_qsv -an -f null -
  else
    rows+=("硬转 QSV 1080p → 1080p H264|跳过（无 h264_qsv）||")
  fi

  if has_encoder h264_nvenc; then
    bench "硬转 NVENC 1080p → 1080p H264" \
      -hwaccel cuda -i "$S1080" -map 0:v:0 -c:v h264_nvenc -preset p4 -an -f null -
  else
    rows+=("硬转 NVENC 1080p → 1080p H264|跳过（无 h264_nvenc）||")
  fi
fi

if [ -f "$S4K" ]; then
  bench "软转 4K HEVC10 → 1080p H264（不做 tone mapping）" \
    -i "$S4K" -map 0:v:0 -vf scale=1920:1080 -c:v libx264 -preset veryfast -crf 23 -an -f null -

  bench "软转 4K HDR10 → 1080p SDR（tone mapping）" \
    -i "$S4K" -map 0:v:0 -vf "scale=1920:1080,$CHAIN" -c:v libx264 -preset veryfast -crf 23 -an -f null -
fi

# ── 输出 ────────────────────────────────────────────────────────
say ""
printf '%-46s %-18s %10s %10s\n' "用例" "结果" "倍速" "耗时"
printf '%-46s %-18s %10s %10s\n' "----------------------------------------------" "------------------" "----------" "----------"
for r in "${rows[@]}"; do
  IFS='|' read -r a b c d <<<"$r"
  printf '%-46s %-18s %10s %10s\n' "$a" "$b" "$c" "$d"
done

if [ -n "$JSON_OUT" ]; then
  {
    printf '{\n  "ffmpeg": "%s",\n  "duration_s": %s,\n  "cases": [\n' \
      "$("$FFMPEG" -version 2>/dev/null | head -1 | cut -d' ' -f3)" "$DURATION"
    first=1
    for r in "${rows[@]}"; do
      IFS='|' read -r a b c d <<<"$r"
      [ "$first" -eq 1 ] || printf ',\n'
      first=0
      speed="${c%x}"; [ -z "$speed" ] && speed=null
      secs="${d%s}";  [ -z "$secs" ] && secs=null
      printf '    {"case": "%s", "result": "%s", "speed_x": %s, "seconds": %s}' \
        "$a" "$b" "$speed" "$secs"
    done
    printf '\n  ]\n}\n'
  } >"$JSON_OUT"
  say ""
  say "JSON 已写入 $JSON_OUT"
fi

say ""
say "倍速 = 素材时长 / 实际耗时。> 1x 表示比实时快，能撑住一路播放。"
say "素材留在 $WORK"
