#!/usr/bin/env bash
# HDR→SDR tone mapping 真机验证。
#
# 为什么要有这个脚本：zscale/tonemap 滤镜链此前只在单元测试里"长得对"，
# 从未在真实 ffmpeg 上跑过。滤镜名拼错、zimg 没编进去、参数顺序不对，
# 单元测试全都不会红 —— 它们只断言字符串内容。真跑才会暴露。
#
# 用法：
#   scripts/tonemap-check.sh                 # 用 PATH 里的 ffmpeg
#   FFmpeg=/opt/ffmpeg/bin scripts/tonemap-check.sh
#
# 退出码 0 = 链可用；1 = 不可用（会在 R5S 上表现为"HDR 片源转码直接失败"）。

set -euo pipefail

FFMPEG="${FFMPEG:-ffmpeg}"
FFPROBE="${FFPROBE:-ffprobe}"
WORK="${WORK:-$(mktemp -d)}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# 与 crates/hardware/src/accel.rs 里 SoftwareAccel::tonemap_filter 保持一致。
# 下面有一道漂移检查：源码改了而这里没改，脚本会直接红。
CHAIN='zscale=t=linear:npl=100,format=gbrpf32le,zscale=p=bt709,tonemap=linear:desat=0,zscale=t=bt709:m=bt709:r=tv,format=yuv420p'

say() { printf '%s\n' "$*"; }
fail() { printf 'FAIL  %s\n' "$*" >&2; exit 1; }

mkdir -p "$WORK" || fail "建不了工作目录 $WORK"

command -v "$FFMPEG" >/dev/null || fail "找不到 $FFMPEG，设置 FFMPEG= 或把它加进 PATH"
command -v "$FFPROBE" >/dev/null || fail "找不到 $FFPROBE"

# ── 0. 漂移检查 ────────────────────────────────────────────────
# 源码里的滤镜链改了，这个脚本的硬编码就会过期 —— 宁可红，不要静默失真。
if ! grep -qF 'zscale=t=linear:npl=100' "$ROOT/crates/hardware/src/accel.rs"; then
  fail "accel.rs 里的 tone mapping 链已变更，请同步 scripts/tonemap-check.sh 的 CHAIN"
fi

# 字段取不到时返回空串，而不是让 pipefail 把整个脚本掀翻 ——
# "取不到"是后面要判断的断言，不是脚本自身的错误。
probe_field() { # $1=文件 $2=字段
  { "$FFPROBE" -v quiet -show_streams -print_format json "$1" 2>/dev/null \
    | tr -d '\n' | grep -o "\"$2\": *\"[^\"]*\"" | head -1 | cut -d'"' -f4; } || true
}

say "ffmpeg    $("$FFMPEG" -version 2>/dev/null | head -1 | cut -d' ' -f1-3)"
say "工作目录  $WORK"

# ── 1. 造一段 HDR10（PQ）片源 ──────────────────────────────────
# 不用下载样本：lavfi 合成 + 打 PQ/bt2020 标记足够验证滤镜链本身。
HDR="$WORK/hdr10.mp4"
"$FFMPEG" -hide_banner -loglevel error -y \
  -f lavfi -i "testsrc2=size=1280x720:rate=30:duration=3" \
  -c:v libx265 -preset ultrafast \
  -x265-params "hdr-opt=1:repeat-headers=1:colorprim=bt2020:transfer=smpte2084:colormatrix=bt2020nc" \
  -pix_fmt yuv420p10le -tag:v hvc1 "$HDR" \
  || fail "造 HDR10 片源失败（这台 ffmpeg 没有 libx265？）"

transfer="$(probe_field "$HDR" color_transfer)"
[ "$transfer" = "smpte2084" ] || fail "片源不是 PQ（color_transfer=$transfer），造片源那步有问题"
say "片源      HDR10 / PQ / bt2020 ✓"

# ── 2. 跑 tone mapping 链 ──────────────────────────────────────
SDR="$WORK/sdr.mp4"
"$FFMPEG" -hide_banner -loglevel error -y -i "$HDR" \
  -vf "$CHAIN" -c:v libx264 -preset ultrafast -crf 28 "$SDR" \
  || fail "tone mapping 链跑不起来：滤镜链在这个 ffmpeg 构建里不可用"
say "链可用    tone map 成功出片 ✓"

out_transfer="$(probe_field "$SDR" color_transfer)"
out_pix="$(probe_field "$SDR" pix_fmt)"
[ "$out_transfer" = "bt709" ] || fail "输出不是 SDR（color_transfer=$out_transfer）"
say "输出      $out_pix / $out_transfer ✓"

# ── 3. scale + tonemap 串在一起 ────────────────────────────────
# args.rs 是把滤镜用逗号拼起来的，必须验证"缩放 + 色调映射"能共存。
"$FFMPEG" -hide_banner -loglevel error -y -ss 0 -i "$HDR" -map 0:v:0 \
  -vf "scale=640:360,$CHAIN" -c:v libx264 -preset veryfast -crf 23 \
  -f mp4 -movflags "+frag_keyframe+empty_moov+default_base_moof" \
  -max_muxing_queue_size 4096 "$WORK/pipe.mp4" \
  || fail "scale + tonemap 组合失败（缩放与色调映射不能串在同一条 -vf 里）"
say "组合可用  scale=640:360 → tonemap ✓"

# ── 4. HLG 片源走同一条链 ──────────────────────────────────────
HLG="$WORK/hlg.mp4"
if "$FFMPEG" -hide_banner -loglevel error -y \
     -f lavfi -i "testsrc2=size=640x360:rate=30:duration=2" \
     -c:v libx265 -preset ultrafast \
     -x265-params "colorprim=bt2020:transfer=arib-std-b67:colormatrix=bt2020nc" \
     -pix_fmt yuv420p10le "$HLG" 2>/dev/null; then
  hlg_transfer="$(probe_field "$HLG" color_transfer)"
  if [ "$hlg_transfer" = "arib-std-b67" ]; then
    "$FFMPEG" -hide_banner -loglevel error -y -i "$HLG" -vf "$CHAIN" \
      -c:v libx264 -preset ultrafast -crf 28 "$WORK/hlg_sdr.mp4" \
      || fail "HLG 片源走同一条链失败"
    say "HLG       arib-std-b67 → bt709 ✓"
  fi
fi

say ""
say "PASS  HDR→SDR tone mapping 链在本机可用"
say "      产物留在 $WORK（如需保留请自行复制，脚本不清理）"
