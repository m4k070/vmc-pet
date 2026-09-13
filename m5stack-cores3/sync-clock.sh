#!/usr/bin/env bash
# M5Stack の時計(BM8563)を、この PC の時計(UTC の Unix 時刻)に合わせる。
#
#   ./sync-clock.sh [ポート]    # 既定は /dev/ttyACM0
#
# 世話の予測は「1日のうちの何時か」を学ぶので、M5Stack の時計が実際の時刻と
# ずれていると、PC と同じ時刻が別の「何時」になる(src/clock.rs 参照)。
# ファームウェアは `time <Unix 秒>` の1行を受け取ると RTC を書き換え、合わせる前に
# 何秒ずれていたかを返す。ポートを開くだけでは M5Stack はリセットされない。
set -euo pipefail

port="${1:-/dev/ttyACM0}"
reply_timeout_seconds=5

stty -F "$port" 115200 raw -echo -hupcl
# 返事を取りこぼさないよう、送る前に読む側を開いておく
exec 3<"$port"
printf 'time %s\n' "$(date +%s)" >"$port"

if ! timeout "$reply_timeout_seconds" \
    grep -m1 -E 'clock set|could not set the clock|no RTC|unrecognized serial line' <&3; then
    echo "no reply from $port within ${reply_timeout_seconds}s (is the firmware up to date?)" >&2
    exit 1
fi
