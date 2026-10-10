#!/usr/bin/env python3
"""css-gesture spike — 決定モデルに「どのジェスチャーを・どの強さで」を選ばせる。

決定論的な層（このスクリプトと gestures.css）は手書きで固定。決定モデルが担うのは
choice（8個から選ぶ）/ score（強度・接近）/ noul（発火するか）の確率だけ。
モデルに CSS を生成させない — それは構造上できない（output_modalities=["decisions"]）。

使い方（サーバは run.sh が面倒を見る）:
  python3 spike.py --base http://127.0.0.1:8897 --model laya --out results-laya.json
"""
from __future__ import annotations

import argparse
import json
import time
import urllib.request
from pathlib import Path

# ジェスチャー語彙（gestures.css の .g-* と1対1）
GESTURES = {
    "idle": "Resting. Slow drifting, no reaction to anything.",
    "perk_up": "Sudden attention. Briefly dips then springs upward.",
    "nod": "Agreement or acknowledgement. Two vertical dips.",
    "shake": "Refusal or disagreement. Three horizontal swings.",
    "lean_in": "Interest, leaning closer. Moves forward and grows.",
    "recoil": "Startle or aversion. Jerks backward and shrinks.",
    "settle": "Disappointment or fatigue. Sinks down and shrinks, stays down.",
    "flicker": "Uncertainty or hesitation. Irregular flickering, no clear motion.",
}

# テスト状態。デスクの相棒が反応する場面を想定した短いテキスト。
STATES = [
    ("fixed_bug",   "ついに3日詰まっていたバグが直った。テストも全部通った。"),
    ("build_fail",  "またビルドが落ちた。同じエラーが4回目。"),
    ("waiting",     "テストスイートの実行が終わるのをただ待っている。"),
    ("branch_gone", "レビュー前にブランチを消されてしまった。"),
    ("uncertain",   "次に何をすればいいのか分からない。選択肢が多すぎる。"),
    ("late_night",  "深夜3時。そろそろ寝たほうがいいのは分かっている。"),
    ("approved",    "レビューで全部OKをもらった。そのままマージできた。"),
    ("nag",         "同じ指摘を3回目にされた。もう直したはずなのに。"),
]

QUESTIONS = {
    "gesture": {
        "type": "choice",
        "instructions": "Which gesture should the pet perform right now, as its only way of responding?",
        "criteria": GESTURES,
    },
    "energy": {
        "type": "score",
        "instructions": "How much energy does the pet have for this reaction?",
        "criteria": ["dormant", "low", "normal", "high", "peaked"],
    },
    "approach": {
        "type": "score",
        "instructions": "Does this pull the pet toward the user or push it away?",
        "criteria": ["recoil", "withdraw", "hold", "lean in", "press close"],
    },
    "emit": {
        "type": "noul",
        "instructions": "Should the pet react at all right now, rather than staying idle?",
    },
}


def ask(base: str, state: str, timeout: int) -> dict:
    req = {"state": state, "questions": QUESTIONS}
    r = urllib.request.urlopen(
        urllib.request.Request(f"{base}/v1/systemone", data=json.dumps(req).encode(),
                               headers={"Content-Type": "application/json"}),
        timeout=timeout,
    )
    return json.loads(r.read())


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--base", default="http://127.0.0.1:8897")
    ap.add_argument("--model", default="?", help="記録用のモデル名")
    ap.add_argument("--out", default="results.json")
    ap.add_argument("--timeout", type=int, default=120)
    ap.add_argument("--states", default="", help="絞るなら state のキーをカンマ区切りで")
    args = ap.parse_args()

    picked = [s for s in STATES if not args.states or s[0] in args.states.split(",")]
    rows = []
    t0 = time.time()
    for key, text in picked:
        ts = time.time()
        try:
            resp = ask(args.base, text, args.timeout)
        except Exception as e:
            print(f"  {key}: 失敗 {type(e).__name__} {e}")
            rows.append({"key": key, "state": text, "error": f"{type(e).__name__}: {e}"})
            continue
        dt = time.time() - ts
        a = resp.get("answers", {})
        g = a.get("gesture", {})
        top = max(g.get("probabilities", {}).items(), key=lambda kv: kv[1]) if g.get("probabilities") else ("?", 0)
        rows.append({
            "key": key, "state": text,
            "gesture": g.get("choice"),
            "gesture_top_prob": top[1],
            "gesture_confidence": g.get("confidence"),
            "gesture_probs": g.get("probabilities", {}),
            "energy": a.get("energy", {}).get("score"),
            "energy_probs": a.get("energy", {}).get("probabilities", {}),
            "approach": a.get("approach", {}).get("score"),
            "approach_probs": a.get("approach", {}).get("probabilities", {}),
            "emit": a.get("emit", {}).get("noul"),
            "latency_s": round(dt, 3),
            "input_tokens": resp.get("usage", {}).get("input_tokens"),
            "output_tokens": resp.get("usage", {}).get("output_tokens"),
        })
        print(f"  {key:12} gesture={str(g.get('choice')):10} p={top[1]:.3f} conf={g.get('confidence', 0):.3f} "
              f"energy={a.get('energy', {}).get('score', 0):.2f} approach={a.get('approach', {}).get('score', 0):.2f} "
              f"emit={a.get('emit', {}).get('noul', 0):.3f}  {dt:.2f}s")

    out = {
        "model": args.model,
        "wall_s": round(time.time() - t0, 1),
        "gestures": list(GESTURES),
        "score_levels": {"energy": 5, "approach": 5},
        "rows": rows,
    }
    Path(args.out).write_text(json.dumps(out, ensure_ascii=False, indent=2))
    print(f"-> {args.out}  ({len(rows)} 件, {out['wall_s']}s)")


if __name__ == "__main__":
    main()
