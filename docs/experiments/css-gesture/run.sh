#!/usr/bin/env bash
# css-gesture spike — サーバ起動〜問い合わせ〜停止を1本で。
#
# 使い方:
#   bash run.sh                  # Laya (0.42B, 質問が独立, 速い)
#   MODEL=clef bash run.sh       # Clef-Flash (9B, 質問を jointly に判断)
#   NGPU=99 bash run.sh          # GPU オフロード
#
# 注意: /health はモデルロード中でも 200 を返す。ログの "listening on http" を待つこと。
# 注意: この環境では terminal(background=true) が機能しないため、起動〜停止を1本にまとめる。
set -uo pipefail

MODELS_DIR=${MODELS_DIR:-$HOME/models}
LLAMA_BIN=${LLAMA_BIN:-$HOME/.local/share/llama.cpp/llama-b11539/llama-server}
MODEL=${MODEL:-laya}
NGPU=${NGPU:-0}
PORT=${PORT:-8897}
HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
LOG=$(mktemp)

case "$MODEL" in
  laya) M="$MODELS_DIR/laya/Laya-Q8_0.gguf"; EXTRA=();;
  clef) M="$MODELS_DIR/clef-flash/Clef-Flash-Q4_K_M.gguf"
        EXTRA=(--mmproj "$MODELS_DIR/clef-flash/mmproj-Clef-Flash-Q8_0.gguf" -c 8192 -b 4096 -ub 4096);;
  *) echo "未知の MODEL: $MODEL (laya|clef)"; exit 1;;
esac

[ -x "$LLAMA_BIN" ] || { echo "llama-server が無い: $LLAMA_BIN"; exit 1; }
[ -f "$M" ]         || { echo "モデルが無い: $M"; exit 1; }

GPU_ARGS=(); [ "$NGPU" != "0" ] && GPU_ARGS=(-ngl "$NGPU")

cd "$(dirname "$LLAMA_BIN")" || exit 1
"$LLAMA_BIN" -m "$M" "${EXTRA[@]}" "${GPU_ARGS[@]}" --host 127.0.0.1 --port "$PORT" > "$LOG" 2>&1 &
SRV=$!
echo "server pid=$SRV model=$MODEL ngl=$NGPU"

READY=0
for i in $(seq 1 420); do
  grep -q "listening on http" "$LOG" 2>/dev/null && { echo "ready after ${i}s"; READY=1; break; }
  grep -q -E "failed to load|error loading" "$LOG" 2>/dev/null && { echo "LOAD FAILED"; tail -25 "$LOG"; kill "$SRV"; exit 1; }
  sleep 1
done
[ "$READY" = "1" ] || { echo "NOT READY"; tail -25 "$LOG"; kill "$SRV" 2>/dev/null; exit 1; }

cd "$HERE" || exit 1
python3 spike.py --base "http://127.0.0.1:$PORT" --model "$MODEL" --out "results-$MODEL.json"
RC=$?

kill "$SRV" 2>/dev/null; sleep 1
echo "server stopped"
exit $RC
