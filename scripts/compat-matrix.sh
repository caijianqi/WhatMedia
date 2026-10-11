#!/usr/bin/env bash
# 播放兼容性矩阵回归脚本。
#
# 用法：scripts/compat-matrix.sh
#
# 跑 `crates/playback/tests/compat_matrix.rs`，把每个客户端 × 每条语料的
# 决策结果和加权覆盖率打成一张表。这份表是 P3 DoD「常见家庭格式覆盖率」
# 的判定依据 —— 每次改播放决策逻辑都应该跑一遍并把输出贴进 PR。
#
# 语料在 test-media/compat/cases.jsonl。加语料的规则写在那个文件的头部注释里。
#
# **为什么单独有个脚本而不是只用 cargo test**：覆盖率是个"多少算够"的判断题，
# 光看 ok/FAILED 不够 —— 需要能看到"是哪几条从 Direct Play 掉到了 Transcode"。
#
# 注意：本机没有 ffmpeg 不影响本脚本 —— 决策是纯函数，不碰进程。

set -euo pipefail

cd "$(dirname "$0")/.."

echo "== 兼容性矩阵 =="
echo
echo "语料：test-media/compat/cases.jsonl"
echo "条数：$(grep -cvE '^\s*(#|$)' test-media/compat/cases.jsonl)"
echo "权重合计：$(grep -oE '"weight": *[0-9]+' test-media/compat/cases.jsonl | grep -oE '[0-9]+$' | awk '{s+=$1} END {print s+0}')"
echo

cargo test -p whatmedia-playback --test compat_matrix -- --nocapture --test-threads=1 2>&1 \
  | grep -E '^\[(web|mpv|android|apple|minimal)\]|覆盖率|^test result' \
  || true

echo
echo "== 覆盖率口径 =="
cat <<'EOS'
  · 桌面端看 Direct Play（转码服务器不该被桌面客户端打爆）
  · Web 端看「无需重编码」= Direct Play + Remux —— 浏览器不认 MKV，
    纯 Direct Play 率在 Web 端天然不可能高，用它做跨端指标是自欺欺人
  · 综合指标取两端「无需重编码」的平均，DoD 阈值 70%
  · 全部按语料的 weight 加权，不按条数 —— 4K 原盘和 1080p H264 不是同等重要
EOS
