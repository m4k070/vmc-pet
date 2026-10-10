#!/usr/bin/env python3
"""css-gesture spike — 結果を「目で判定できる」HTML にする。

判定したいのは:
  1. 8個のジェスチャーが読み分けられるか（混同しやすい組はどれか）
  2. 確率分布の混合が意味のある中間状態になるか
  3. confidence が低いときの見た目が「迷い」に見えるか
  4. score 軸が動きの差として知覚できるか

使い方:
  python3 build_viewer.py --results results-clef.json --out index.html
"""
from __future__ import annotations

import argparse
import json
from pathlib import Path

# 決定論的な写像。ここが「生成」を担う層で、モデルは触らない。
def to_css(row: dict, theta: float = 0.30) -> dict:
    energy = row.get("energy") or 0.0
    approach = row.get("approach") or 0.0
    emit = row.get("emit") or 0.0
    conf = row.get("gesture_confidence") or 0.0
    return {
        "speed": round(1.55 - 0.22 * energy, 3),      # energy 0->4 で 1.55s -> 0.67s
        "amp": round(0.45 + 0.28 * energy, 3),        # 0.45 -> 1.57
        "scale": round(0.88 + 0.09 * approach, 3),    # 0.88 -> 1.24
        "glow": round(emit, 3),
        "raw": row.get("gesture"),
        "gesture": row.get("gesture") if emit >= theta else "idle",
        "gated": emit < theta,
        "theta": theta,
        "conf": round(conf, 3),
    }


def bar(probs: dict, labels: list[str]) -> str:
    total = sum(probs.values()) or 1.0
    parts = []
    for k in labels:
        v = probs.get(k, 0.0)
        parts.append(
            f'<span class="seg" style="width:{100 * v / total:.1f}%" '
            f'title="{k} {v:.3f}"><i class="swatch s-{k}"></i></span>'
        )
    return f'<div class="bar">{"".join(parts)}</div>'


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--results", default="results-clef.json")
    ap.add_argument("--compare", default="", help="並べて比べるもう1つの results (任意)")
    ap.add_argument("--out", default="index.html")
    ap.add_argument("--css", default="gestures.css")
    ap.add_argument("--theta", type=float, default=0.30,
                    help="noul の発火しきい値。0.5 だと Clef は全部 idle になる（実測）")
    args = ap.parse_args()

    here = Path(__file__).parent
    data = json.loads((here / args.results).read_text())
    gestures = data["gestures"]
    css = (here / args.css).read_text()
    cmp_data = json.loads((here / args.compare).read_text()) if args.compare else None
    cmp_by_key = {r["key"]: r for r in cmp_data["rows"]} if cmp_data else {}

    cards = []
    for row in data["rows"]:
        if "error" in row:
            cards.append(f'<div class="card err"><h3>{row["key"]}</h3><p>{row["error"]}</p></div>')
            continue
        c = to_css(row, args.theta)
        other = cmp_by_key.get(row["key"])
        other_html = ""
        if other and "error" not in other:
            oc = to_css(other, args.theta)
            other_html = f"""
      <div class="cmp">
        <div class="cmp-label">{cmp_data['model']}</div>
        <div class="stage">
          <div class="body g-{oc['gesture']}" style="--speed:{oc['speed']};--amp:{oc['amp']};--scale:{oc['scale']};--glow:{oc['glow']}"></div>
        </div>
        <div class="cmp-meta">{oc['gesture']} / e{other.get('energy') or 0:.1f} a{other.get('approach') or 0:.1f} emit {other.get('emit') or 0:.2f}</div>
      </div>"""

        cards.append(f"""
  <div class="card">
    <h3>{row['key']} <span class="tag">{data['model']}</span></h3>
    <p class="state">{row['state']}</p>
    <div class="row">
      <div class="stage">
        <div class="body g-{c['gesture']}" style="--speed:{c['speed']};--amp:{c['amp']};--scale:{c['scale']};--glow:{c['glow']}"></div>
      </div>{other_html}
    </div>
    <div class="meta">
      <b>{c['gesture']}</b>{f' <span class="gated">(emit {row.get("emit") or 0:.2f} &lt; θ {c["theta"]:.2f} → idle に落とした。モデルの選択は {c["raw"]})</span>' if c['gated'] else ''}
      &nbsp;conf {c['conf']:.2f}
    </div>
    <dl>
      <dt>energy</dt><dd>{row.get('energy') or 0:.2f} / 4 <span class="fx">→ duration {c['speed']:.2f}s, amp {c['amp']:.2f}</span></dd>
      <dt>approach</dt><dd>{row.get('approach') or 0:.2f} / 4 <span class="fx">→ scale {c['scale']:.2f}</span></dd>
      <dt>emit</dt><dd>{row.get('emit') or 0:.3f} <span class="fx">→ glow</span></dd>
      <dt>latency</dt><dd>{row.get('latency_s')}s</dd>
    </dl>
    {bar(row.get('gesture_probs', {}), gestures)}
    <div class="legend">{' '.join(f'<span><i class="swatch s-{g}"></i>{g}</span>' for g in gestures)}</div>
  </div>""")

    html = f"""<!doctype html>
<html lang="ja"><head><meta charset="utf-8">
<title>css-gesture spike</title>
<style>
:root {{ color-scheme: light dark; }}
* {{ box-sizing: border-box; }}
body {{ margin: 0; color: var(--foreground, #ddd); font-family: inherit; }}
h1 {{ font-size: 15px; margin: 0 0 4px; }}
.sub {{ font-size: 12px; color: var(--muted-foreground, #999); margin: 0 0 14px; line-height: 1.5; }}
.grid {{ display: grid; grid-template-columns: repeat(auto-fill, minmax(280px, 1fr)); gap: 12px; }}
.card {{ background: var(--card, rgba(127,127,127,.08)); border: 1px solid var(--border, rgba(127,127,127,.25));
  border-radius: 10px; padding: 12px; }}
.card.err {{ border-color: #a33; }}
h3 {{ font-size: 13px; margin: 0 0 4px; font-weight: 600; }}
.tag {{ font-size: 10px; font-weight: 400; color: var(--muted-foreground, #999); border: 1px solid var(--border, #555);
  border-radius: 4px; padding: 0 4px; margin-left: 4px; }}
.state {{ font-size: 11px; color: var(--muted-foreground, #999); margin: 0 0 8px; min-height: 28px; }}
.row {{ display: flex; gap: 8px; align-items: center; }}
.stage {{ width: 118px; height: 118px; display: grid; place-items: center; flex: 0 0 auto; }}
.cmp .stage {{ width: 92px; height: 92px; }}
.cmp {{ display: grid; justify-items: center; gap: 2px; }}
.cmp-label, .cmp-meta {{ font-size: 9px; color: var(--muted-foreground, #999); text-align: center; }}
.cmp .body {{ width: 40px; height: 40px; border-radius: 12px; }}
.meta {{ font-size: 12px; margin: 6px 0 8px; }}
.gated {{ color: var(--muted-foreground, #999); font-size: 10px; }}
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
<h1>css-gesture spike — 決定モデルの出力が「読める動き」になるか</h1>
<p class="sub">左が <b>{data['model']}</b> の結果。{f"右に <b>{cmp_data['model']}</b> を並べた。" if cmp_data else ""}
判定したいのは4点: ①8個のジェスチャーが読み分けられるか ②分布の混合が意味のある中間になるか
③confidence が低いとき「迷い」に見えるか ④ score 軸が動きの差として知覚できるか。<br>
<b>θ（emit の発火しきい値）= {args.theta:.2f}</b> を使っている。素直に 0.5 にすると Clef は
<b>8件すべてが idle になった</b>（明確な場面でも emit は 0.49 止まり）。しきい値は推測せずデータから決める必要がある、
という較正の実測例。emit &lt; θ の件は idle に落とし、モデルが選んだジェスチャーは括弧内に残した。</p>
<div class="grid">{''.join(cards)}</div>
</body></html>"""

    out = here / args.out
    out.write_text(html)
    print(f"-> {out}  ({len(cards)} カード)")
    for row in data["rows"]:
        if "error" in row:
            continue
        c = to_css(row, args.theta)
        suffix = f"  (gated, raw={c['raw']})" if c["gated"] else ""
        print(f"   {row['key']:12} {c['gesture']:9} speed={c['speed']:.2f} amp={c['amp']:.2f} "
              f"scale={c['scale']:.2f} glow={c['glow']:.3f}{suffix}")


if __name__ == "__main__":
    main()
