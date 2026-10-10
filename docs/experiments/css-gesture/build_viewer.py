#!/usr/bin/env python3
"""css-gesture spike — 結果を「目で判定できる」HTML にする（θ をスライダで動かせる版）。

1回目の判定で分かったこと:
  - energy の強弱は見た目で差が分かる（軸としては機能する）
  - waiting / late_night / branch_gone は区別が難しい
  → この3件は**ちょうど θ=0.30 で idle に落ちた3件**。同じアニメーションなので区別できないのは当然で、
    「ゲートが情報を捨てている」ことが人の目で確認された形。

そこで θ を焼き込まず、ブラウザ側のスライダで動かせるようにする。どの θ でどの状態が発火するかを
見ながら、沈黙と可読性のバランスを人が決められる。

判定したいのは:
  1. 8個のジェスチャーが読み分けられるか（混同しやすい組はどれか）
  2. 確率分布の混合が意味のある中間状態になるか
  3. confidence が低いときの見た目が「迷い」に見えるか
  4. score 軸が動きの差として知覚できるか
  5. θ をどこに置くべきか

使い方:
  python3 build_viewer.py --results results-clef.json --compare results-laya.json --out index.html
"""
from __future__ import annotations

import argparse
import json
from pathlib import Path

GESTURE_ORDER = ["idle", "perk_up", "nod", "shake", "lean_in", "recoil", "settle", "flicker"]
DEFAULT_THETA = 0.30


# 決定論的な写像。ここが「生成」を担う層で、モデルは触らない。
def to_css(row: dict) -> dict:
    energy = row.get("energy") or 0.0
    approach = row.get("approach") or 0.0
    emit = row.get("emit") or 0.0
    return {
        "speed": round(1.55 - 0.22 * energy, 3),      # energy 0->4 で 1.55s -> 0.67s
        "amp": round(0.45 + 0.28 * energy, 3),        # 0.45 -> 1.57
        "scale": round(0.88 + 0.09 * approach, 3),    # 0.88 -> 1.24
        "glow": round(emit, 3),
        "raw": row.get("gesture"),
        "conf": round(row.get("gesture_confidence") or 0.0, 3),
    }


def bar(probs: dict) -> str:
    total = sum(probs.values()) or 1.0
    segs = "".join(
        f'<span class="seg" style="width:{100 * v / total:.1f}%" title="{k} {v:.3f}"></span>'
        for k, v in sorted(probs.items(), key=lambda kv: -kv[1])
    )
    return f'<div class="bar">{segs}</div>'


def body_div(c: dict, cls: str = "body") -> str:
    return (
        f'<div class="{cls} g-{c["raw"]}" data-emit="{c["glow"]}" data-raw="{c["raw"]}" '
        f'style="--speed:{c["speed"]};--amp:{c["amp"]};--scale:{c["scale"]};--glow:{c["glow"]}"></div>'
    )


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--results", default="results-clef.json")
    ap.add_argument("--compare", default="", help="並べて比べるもう1つの results (任意)")
    ap.add_argument("--out", default="index.html")
    ap.add_argument("--css", default="gestures.css")
    ap.add_argument("--theta", type=float, default=DEFAULT_THETA, help="スライダの初期値")
    args = ap.parse_args()

    here = Path(__file__).parent
    data = json.loads((here / args.results).read_text())
    css = (here / args.css).read_text()
    cmp_data = json.loads((here / args.compare).read_text()) if args.compare else None
    cmp_by_key = {r["key"]: r for r in cmp_data["rows"]} if cmp_data else {}

    # θ ごとにどの状態が発火するかを先に出す（人が判断する材料）
    emits = {r["key"]: (r.get("emit") or 0.0) for r in data["rows"] if "error" not in r}
    print("θ ごとの発火:")
    for th in (0.50, 0.30, 0.20, 0.15, 0.10, 0.05):
        fired = [k for k, e in emits.items() if e >= th]
        print(f"  θ={th:.2f}: {len(fired)}/{len(emits)}  {', '.join(fired) if fired else '(なし)'}")

    cards = []
    for row in data["rows"]:
        if "error" in row:
            cards.append(f'<div class="card err"><h3>{row["key"]}</h3><p>{row["error"]}</p></div>')
            continue
        c = to_css(row)
        other = cmp_by_key.get(row["key"])
        other_html = ""
        if other and "error" not in other:
            oc = to_css(other)
            other_html = f"""
      <div class="cmp">
        <div class="cmp-label">{cmp_data['model']}</div>
        <div class="stage sm">{body_div(oc)}</div>
        <div class="cmp-meta"><span class="cn">{oc['raw']}</span> / e{(other.get('energy') or 0):.1f} a{(other.get('approach') or 0):.1f}</div>
      </div>"""

        cards.append(f"""
  <div class="card">
    <h3>{row['key']} <span class="tag">{data['model']}</span></h3>
    <p class="state">{row['state']}</p>
    <div class="row">
      <div class="stage">{body_div(c)}</div>{other_html}
    </div>
    <div class="meta"><b class="gname">?</b><span class="gnote"></span> <span class="conf">conf {c['conf']:.2f}</span></div>
    <dl>
      <dt>energy</dt><dd>{row.get('energy') or 0:.2f} / 4 <span class="fx">→ duration {c['speed']:.2f}s, amp {c['amp']:.2f}</span></dd>
      <dt>approach</dt><dd>{row.get('approach') or 0:.2f} / 4 <span class="fx">→ scale {c['scale']:.2f}</span></dd>
      <dt>emit</dt><dd>{c['glow']:.3f} <span class="fx">→ glow</span></dd>
      <dt>latency</dt><dd>{row.get('latency_s')}s</dd>
    </dl>
    {bar(row.get('gesture_probs', {}))}
    <div class="legend">{' '.join(f'<span><i class="swatch s-{g}"></i>{g}</span>' for g in GESTURE_ORDER)}</div>
  </div>""")

    html = f"""<!doctype html>
<html lang="ja"><head><meta charset="utf-8">
<title>css-gesture spike</title>
<style>
:root {{ color-scheme: light dark; }}
* {{ box-sizing: border-box; }}
body {{ margin: 0; color: var(--foreground, #ddd); font-family: inherit; }}
h1 {{ font-size: 15px; margin: 0 0 6px; }}
.sub {{ font-size: 12px; color: var(--muted-foreground, #999); margin: 0 0 12px; line-height: 1.55; }}
.controls {{ display: flex; align-items: center; gap: 10px; flex-wrap: wrap; margin: 0 0 14px;
  padding: 10px 12px; border: 1px solid var(--border, rgba(127,127,127,.3)); border-radius: 10px; }}
.controls label {{ font-size: 12px; white-space: nowrap; }}
.controls input[type=range] {{ width: 220px; accent-color: var(--accent, #d2694a); }}
.controls .val {{ font-variant-numeric: tabular-nums; font-size: 13px; font-weight: 600; min-width: 44px; }}
.controls .count {{ font-size: 12px; color: var(--muted-foreground, #999); }}
.controls .fires {{ font-size: 11px; color: var(--muted-foreground, #999); }}
.grid {{ display: grid; grid-template-columns: repeat(auto-fill, minmax(272px, 1fr)); gap: 12px; }}
.card {{ background: var(--card, rgba(127,127,127,.08)); border: 1px solid var(--border, rgba(127,127,127,.25));
  border-radius: 10px; padding: 12px; transition: opacity .15s; }}
.card.err {{ border-color: #a33; }}
.card.silent {{ opacity: .45; }}
h3 {{ font-size: 13px; margin: 0 0 4px; font-weight: 600; }}
.tag {{ font-size: 10px; font-weight: 400; color: var(--muted-foreground, #999); border: 1px solid var(--border, #555);
  border-radius: 4px; padding: 0 4px; margin-left: 4px; }}
.state {{ font-size: 11px; color: var(--muted-foreground, #999); margin: 0 0 8px; min-height: 28px; }}
.row {{ display: flex; gap: 8px; align-items: center; }}
.stage {{ width: 118px; height: 118px; display: grid; place-items: center; flex: 0 0 auto; }}
.stage.sm {{ width: 92px; height: 92px; }}
.cmp {{ display: grid; justify-items: center; gap: 2px; }}
.cmp-label, .cmp-meta {{ font-size: 9px; color: var(--muted-foreground, #999); text-align: center; }}
.cmp .body {{ width: 40px; height: 40px; border-radius: 12px; }}
.meta {{ font-size: 12px; margin: 6px 0 8px; }}
.gnote {{ color: var(--muted-foreground, #999); font-size: 10px; }}
.conf {{ color: var(--muted-foreground, #999); font-size: 10px; }}
dl {{ display: grid; grid-template-columns: auto 1fr; gap: 2px 10px; font-size: 11px; margin: 0 0 8px; }}
dt {{ color: var(--muted-foreground, #999); }}
dd {{ margin: 0; font-variant-numeric: tabular-nums; }}
.fx {{ color: var(--muted-foreground, #999); font-size: 10px; }}
.bar {{ display: flex; height: 10px; border-radius: 3px; overflow: hidden; background: rgba(127,127,127,.18); }}
.seg {{ display: block; height: 100%; }}
.swatch {{ display: inline-block; width: 7px; height: 7px; border-radius: 2px; margin-right: 3px; vertical-align: middle; }}
.s-idle {{ background: #6b7280; }} .s-perk_up {{ background: #f59e0b; }} .s-nod {{ background: #22c55e; }}
.s-shake {{ background: #ef4444; }} .s-lean_in {{ background: #3b82f6; }} .s-recoil {{ background: #a855f7; }}
.s-settle {{ background: #0ea5e9; }} .s-flicker {{ background: #eab308; }}
.legend {{ display: flex; flex-wrap: wrap; gap: 6px; font-size: 10px; color: var(--muted-foreground, #999); margin-top: 6px; }}
{css}
</style></head>
<body>
<h1>css-gesture spike — θ を動かして「どこで発火を止めるか」を決める</h1>
<p class="sub">
1回目の判定: <b>energy の強弱は見た目で差が分かる</b>（軸として機能）。一方
<b>waiting / late_night / branch_gone は区別が難しい</b>。この3件は θ=0.30 で idle に落ちた3件で、
<b>同じアニメーションなので区別できないのは当然</b> — ゲートが情報を捨てている。<br>
θ を下げると <code>branch_gone</code> は <code>settle</code>、<code>uncertain</code> は
<code>flicker</code> として発火して見分けがつくようになる。代わりに「反応しすぎ」に近づく。
どこで止めるかを決めるのがこの画面の目的。左が <b>{data['model']}</b>、右が <b>{cmp_data['model'] if cmp_data else '-'}</b>。
</p>
<div class="controls">
  <label for="th">θ（emit の発火しきい値）</label>
  <input id="th" type="range" min="0" max="0.5" step="0.01" value="{args.theta}">
  <span class="val" id="thv">{args.theta:.2f}</span>
  <span class="count" id="cnt"></span>
  <span class="fires" id="fires"></span>
</div>
<div class="grid">{''.join(cards)}</div>
<script>
function apply(theta) {{
  document.getElementById('thv').textContent = theta.toFixed(2);
  const fired = [];
  document.querySelectorAll('.card').forEach(card => {{
    const body = card.querySelector('.stage .body');
    if (!body) return;
    const emit = parseFloat(body.dataset.emit);
    const raw = body.dataset.raw;
    const on = emit >= theta;
    body.className = 'body g-' + (on ? raw : 'idle');
    card.classList.toggle('silent', !on);
    const name = card.querySelector('.gname');
    const note = card.querySelector('.gnote');
    const key = card.querySelector('h3').textContent.trim().split(/\\s+/)[0];
    if (name) name.textContent = on ? raw : 'idle';
    if (note) note.innerHTML = on ? '' :
      ' <span>(emit ' + emit.toFixed(2) + ' &lt; ' + theta.toFixed(2) + ' → idle。モデルの選択は ' + raw + ')</span>';
    if (on) fired.push(key);
  }});
  document.getElementById('cnt').textContent = '発火 ' + fired.length + ' / 8';
  document.getElementById('fires').textContent = fired.join(', ');
}}
document.getElementById('th').addEventListener('input', e => apply(parseFloat(e.target.value)));
apply({args.theta});
</script>
</body></html>"""

    out = here / args.out
    out.write_text(html)
    print(f"-> {out}  ({len(cards)} カード, θ はスライダで操作可)")


if __name__ == "__main__":
    main()
