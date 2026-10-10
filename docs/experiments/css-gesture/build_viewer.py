#!/usr/bin/env python3
"""css-gesture spike — 姿勢パラメータ空間版のビューア。

前版までの経緯:
  - 8個の keyframe アニメーションを並べ、確率で不透明度を重ねる方式を試した
  - 結果は「半透明の図形がずれた雲」で、動きの混合にならなかった（人の判定で確認）
  - 語彙を poses.json の「姿勢パラメータ空間の点」に変え、混合を**重み付き平均（補間）**にした
  - 描画は常に1体。argmax の姿勢と混合の姿勢を切り替えて比べられるようにした

判定したいのは:
  1. 8個の姿勢が読み分けられるか（混同しやすい組はどれか）
  2. 補間した姿勢が意味のある中間になるか（重ね合わせでは雲になった。補間ならどうか）
  3. confidence が低いときの見た目が「迷い」に見えるか
  4. score 軸（energy / approach）が動きの差として知覚できるか
  5. θ をどこに置くべきか
  6. 姿勢空間にしたことによる忠実度の損失（波の形が1種類になった）が問題になるか

姿勢ベクトル → CSS 変数の写像は**ブラウザ側の JS に一本化**している（Python と二重に持つと
ずれるため）。Python は姿勢ベクトルと生の数値を data 属性で埋めるだけ。

使い方:
  python3 build_viewer.py --results results-clef.json --compare results-laya.json --out index.html
"""
from __future__ import annotations

import argparse
import json
from pathlib import Path

GESTURE_ORDER = ["idle", "perk_up", "nod", "shake", "lean_in", "recoil", "settle", "flicker"]
DEFAULT_THETA = 0.30
FACES = ('<div class="face f-front"></div><div class="face f-back"></div>'
         '<div class="face f-right"></div><div class="face f-left"></div>'
         '<div class="face f-top"></div><div class="face f-bottom"></div>')


def load_poses(path: Path) -> dict:
    d = json.loads(path.read_text())
    return {
        "gestures": d["gestures"],
        "px_per_unit": d["scale"]["px_per_unit"],
        "deg_per_conflict": d["scale"]["deg_per_conflict"],
        "dimensions": list(d["dimensions"]),
        "notes": d["notes"],
    }


def pose_argmax(probs: dict, gestures: dict, fallback: str = "idle") -> tuple[str, dict]:
    if not probs:
        return fallback, gestures[fallback]
    top = max(probs.items(), key=lambda kv: kv[1])[0]
    return top, gestures.get(top, gestures[fallback])


def pose_mean(probs: dict, gestures: dict) -> tuple[dict, dict]:
    """確率による姿勢ベクトルの重み付き平均（= 補間）。重ね合わせではない。"""
    total = sum(probs.values()) or 1.0
    dims = list(next(iter(gestures.values())).keys())
    acc = {d: 0.0 for d in dims}
    for g, p in probs.items():
        vec = gestures.get(g)
        if not vec:
            continue
        w = p / total
        for d in dims:
            acc[d] += w * vec[d]
    return acc, {d: round(acc[d], 4) for d in dims}


def bar(probs: dict) -> str:
    total = sum(probs.values()) or 1.0
    segs = "".join(
        f'<span class="seg s-{k}" style="width:{100 * v / total:.2f}%" '
        f'title="{k} {v:.3f} ({100 * v / total:.1f}%)"></span>'
        for k, v in sorted(probs.items(), key=lambda kv: -kv[1])
    )
    return f'<div class="bar">{segs}</div>'


def dist_text(probs: dict, top: int = 3) -> str:
    total = sum(probs.values()) or 1.0
    items = sorted(probs.items(), key=lambda kv: -kv[1])
    head = " · ".join(f'<b class="d-{k}">{k}</b> {100 * v / total:.0f}%' for k, v in items[:top])
    rest = len(items) - top
    return head + (f' <span class="rest">他 {rest}</span>' if rest > 0 else "")


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--results", default="results-clef.json")
    ap.add_argument("--compare", default="", help="並べて比べるもう1つの results (任意)")
    ap.add_argument("--out", default="index.html")
    ap.add_argument("--poses", default="poses.json")
    ap.add_argument("--css", default="gestures.css")
    ap.add_argument("--theta", type=float, default=DEFAULT_THETA)
    ap.add_argument("--default-mode", choices=["argmax", "mean"], default="mean",
                    help="初期表示。mean = 姿勢の補間（既定）、argmax = 一番確率の高い姿勢")
    args = ap.parse_args()

    here = Path(__file__).parent
    data = json.loads((here / args.results).read_text())
    P = load_poses(here / args.poses)
    gestures = P["gestures"]
    css = (here / args.css).read_text()
    cmp_data = json.loads((here / args.compare).read_text()) if args.compare else None
    cmp_by_key = {r["key"]: r for r in cmp_data["rows"]} if cmp_data else {}

    emits = {r["key"]: (r.get("emit") or 0.0) for r in data["rows"] if "error" not in r}
    print("θ ごとの発火:")
    for th in (0.50, 0.30, 0.20, 0.15, 0.10, 0.05):
        fired = [k for k, e in emits.items() if e >= th]
        print(f"  θ={th:.2f}: {len(fired)}/{len(emits)}  {', '.join(fired) if fired else '(なし)'}")

    cards, report = [], []
    for row in data["rows"]:
        if "error" in row:
            cards.append(f'<div class="card err"><h3>{row["key"]}</h3><p>{row["error"]}</p></div>')
            continue
        probs = row.get("gesture_probs", {})
        top, pose_top = pose_argmax(probs, gestures)
        mean, mean_r = pose_mean(probs, gestures)
        energy = row.get("energy") or 0.0
        approach = row.get("approach") or 0.0
        emit = row.get("emit") or 0.0
        report.append(f"  {row['key']:12} top={top:9} bob{mean_r['bob']:.2f} "
                      f"rot_x{mean_r['rot_x']:.1f} rot_y{mean_r['rot_y']:.1f} "
                      f"rot_z{mean_r['rot_z']:.1f} lift{mean_r['lift']:.2f} sink{mean_r['sink']:.2f} "
                      f"size{mean_r['size']:.2f} freq{mean_r['freq']:.2f} flick{mean_r['flick']:.2f} "
                      f"| 葛藤{2*min(mean_r['lift'], mean_r['sink']):.3f} "
                      f"→ roll {P['deg_per_conflict']*2*min(mean_r['lift'], mean_r['sink']):.1f}deg")

        # Laya を並べる（同じ姿勢空間で描く）
        other_html = ""
        other = cmp_by_key.get(row["key"])
        if other and "error" not in other:
            oprobs = other.get("gesture_probs", {})
            otop, _ = pose_argmax(oprobs, gestures)
            _, omean_r = pose_mean(oprobs, gestures)
            other_html = f"""
      <div class="cmp">
        <div class="cmp-label">{cmp_data['model']}</div>
        <div class="stage sm" data-role="cmp"
          data-pose="{json.dumps(omean_r).replace(chr(34), '&quot;')}"
          data-energy="{other.get('energy') or 0}" data-approach="{other.get('approach') or 0}"
          data-emit="{other.get('emit') or 0}">
          <div class="flick"><div class="body">{FACES}</div></div>
        </div>
        <div class="cmp-meta">{otop} / e{(other.get('energy') or 0):.1f} a{(other.get('approach') or 0):.1f}</div>
      </div>"""

        cards.append(f"""
  <div class="card" data-key="{row['key']}">
    <h3>{row['key']} <span class="tag">{data['model']}</span></h3>
    <p class="state">{row['state']}</p>
    <div class="row">
      <div class="cmp">
        <div class="cmp-label">{data['model']}</div>
        <div class="stage" data-role="main"
          data-pose-top="{json.dumps(pose_top).replace(chr(34), '&quot;')}"
          data-pose-mean="{json.dumps(mean_r).replace(chr(34), '&quot;')}"
          data-energy="{energy}" data-approach="{approach}" data-emit="{emit}"
          data-top="{top}">
          <div class="flick"><div class="body">{FACES}</div></div>
        </div>
      </div>{other_html}
    </div>
    <div class="meta"><b class="gname">?</b><span class="gnote"></span> <span class="conf">conf {row.get('gesture_confidence') or 0:.2f}</span></div>
    <div class="pose"></div>
    <dl>
      <dt>energy</dt><dd>{energy:.2f} / 4</dd>
      <dt>approach</dt><dd>{approach:.2f} / 4</dd>
      <dt>emit</dt><dd>{emit:.3f}</dd>
      <dt>latency</dt><dd>{row.get('latency_s')}s</dd>
    </dl>
    {bar(probs)}
    <div class="dist">{dist_text(probs)}</div>
    <div class="legend">{' '.join(f'<span><i class="swatch s-{g}"></i>{g}</span>' for g in GESTURE_ORDER)}</div>
  </div>""")

    print("\n姿勢ベクトル（補間後）:")
    for r in report:
        print(r)

    html = f"""<!doctype html>
<html lang="ja"><head><meta charset="utf-8">
<title>css-gesture spike (pose space)</title>
<style>
{css}
/* --- ビューアのレイアウト。gestures.css より後に置いて上書きする --- */
:root {{ color-scheme: light dark; }}
* {{ box-sizing: border-box; }}
body {{ margin: 0; color: var(--foreground, #ddd); font-family: inherit; }}
h1 {{ font-size: 15px; margin: 0 0 6px; }}
.sub {{ font-size: 12px; color: var(--muted-foreground, #999); margin: 0 0 12px; line-height: 1.55; }}
.controls {{ display: flex; align-items: center; gap: 10px; flex-wrap: wrap; margin: 0 0 14px;
  padding: 10px 12px; border: 1px solid var(--border, rgba(127,127,127,.3)); border-radius: 10px; }}
.controls label {{ font-size: 12px; white-space: nowrap; }}
.controls input[type=range] {{ width: 200px; accent-color: var(--accent, #d2694a); }}
.controls .val {{ font-variant-numeric: tabular-nums; font-size: 13px; font-weight: 600; min-width: 44px; }}
.controls .count, .controls .fires {{ font-size: 11px; color: var(--muted-foreground, #999); }}
.grid {{ display: grid; grid-template-columns: repeat(auto-fill, minmax(272px, 1fr)); gap: 12px; }}
.card {{ background: var(--card, rgba(127,127,127,.08)); border: 1px solid var(--border, rgba(127,127,127,.25));
  border-radius: 10px; padding: 12px; transition: opacity .15s; }}
.card.err {{ border-color: #a33; }}
.card.silent {{ opacity: .5; }}
h3 {{ font-size: 13px; margin: 0 0 4px; font-weight: 600; }}
.tag {{ font-size: 10px; font-weight: 400; color: var(--muted-foreground, #999); border: 1px solid var(--border, #555);
  border-radius: 4px; padding: 0 4px; margin-left: 4px; }}
.state {{ font-size: 11px; color: var(--muted-foreground, #999); margin: 0 0 8px; min-height: 28px; }}
.row {{ display: flex; gap: 8px; align-items: center; }}
.cmp {{ display: grid; justify-items: center; gap: 2px; }}
.cmp-label, .cmp-meta {{ font-size: 9px; color: var(--muted-foreground, #999); text-align: center; }}
.meta {{ font-size: 12px; margin: 6px 0 4px; }}
.gnote {{ color: var(--muted-foreground, #999); font-size: 10px; }}
.conf {{ color: var(--muted-foreground, #999); font-size: 10px; }}
.pose {{ font-size: 9px; color: var(--muted-foreground, #999); font-variant-numeric: tabular-nums;
  margin: 0 0 8px; line-height: 1.4; }}
dl {{ display: grid; grid-template-columns: auto 1fr; gap: 2px 10px; font-size: 11px; margin: 0 0 8px; }}
dt {{ color: var(--muted-foreground, #999); }}
dd {{ margin: 0; font-variant-numeric: tabular-nums; }}
.bar {{ display: flex; height: 12px; border-radius: 3px; overflow: hidden; background: rgba(127,127,127,.18); }}
.seg {{ display: block; height: 100%; }}
.dist {{ font-size: 10px; color: var(--muted-foreground, #999); margin-top: 5px; }}
.dist b {{ font-weight: 600; }}
.d-idle {{ color: #6b7280; }} .d-perk_up {{ color: #f59e0b; }} .d-nod {{ color: #22c55e; }}
.d-shake {{ color: #ef4444; }} .d-lean_in {{ color: #3b82f6; }} .d-recoil {{ color: #a855f7; }}
.d-settle {{ color: #0ea5e9; }} .d-flicker {{ color: #eab308; }}
.rest {{ opacity: .65; }}
.swatch {{ display: inline-block; width: 7px; height: 7px; border-radius: 2px; margin-right: 3px; vertical-align: middle; }}
.s-idle {{ background: #6b7280; }} .s-perk_up {{ background: #f59e0b; }} .s-nod {{ background: #22c55e; }}
.s-shake {{ background: #ef4444; }} .s-lean_in {{ background: #3b82f6; }} .s-recoil {{ background: #a855f7; }}
.s-settle {{ background: #0ea5e9; }} .s-flicker {{ background: #eab308; }}
.legend {{ display: flex; flex-wrap: wrap; gap: 6px; font-size: 10px; color: var(--muted-foreground, #999); margin-top: 6px; }}
.stage {{ width: 118px; height: 118px; }}
.stage .body {{ width: 42px; height: 42px; border-radius: 12px; }}
.stage.sm {{ width: 92px; height: 92px; }}
.stage.sm .body {{ width: 32px; height: 32px; border-radius: 10px; }}
</style></head>
<body>
<h1>css-gesture spike — 姿勢パラメータ空間（混合は補間、描画は1体）</h1>
<p class="sub">
語彙を <code>poses.json</code> の<code>&lbrace;bob, rot_x, rot_y, rot_z, lift, sink, size, freq, flick&rbrace;</code> の点として定義し直した。
混合は<strong>重み付き平均（補間）</strong>で1つの姿勢ベクトルにし、描画は常に<strong>1つの Cube</strong>。<br>
<strong>回転が主役</strong>: 平行移動の振幅差は人間に読めない（実測で fixed_bug と approved の縦振幅差は 1.31x しかなく静止画では読めなかった）。
Cube では yaw / pitch が「別の面が見える」離散的な変化になるので、回転として読める。<br>
<code>lift</code> / <code>sink</code> は上下の向きを分けた非負のチャネル。静的な偏りは <code>(sink - lift)</code> が担い、
符号付き平均が捨てていた <code>2*min(lift, sink)</code> を<strong>roll の振れ（度）として戻す</strong>ので、
「沈む + 浮く」が<strong>傾き（葛藤）</strong>として描かれる。<br>
前版の「確率で重ねる」方式は<strong>半透明の図形がずれた雲</strong>になり、動きの混合にならなかった（人の判定）。
<strong>表示</strong>を切り替えると、一番確率の高い姿勢（argmax）と、補間した姿勢（mean）を同じ枠で比べられる。<br>
モデル <b>{data['model']}</b> / 比較 <b>{cmp_data['model'] if cmp_data else '-'}</b>。
波の形は往復の正弦1種類に統一したので、<code>perk_up</code> の「一瞬沈んでから跳ねる」ような
波形の違いは表現できない（振幅・速度・静的偏りで近似）。
</p>
<div class="controls">
  <label for="th">θ（emit の発火しきい値）</label>
  <input id="th" type="range" min="0" max="0.5" step="0.01" value="{args.theta}">
  <span class="val" id="thv">{args.theta:.2f}</span>
  <span class="count" id="cnt"></span>
  <span class="fires" id="fires"></span>
  <label for="mode">表示</label>
  <select id="mode">
    <option value="mean"{' selected' if args.default_mode == 'mean' else ''}>mean（姿勢を補間）</option>
    <option value="argmax"{' selected' if args.default_mode == 'argmax' else ''}>argmax（一番高い姿勢）</option>
  </select>
</div>
<div class="grid">{''.join(cards)}</div>
<script>
const PX = {P['px_per_unit']};
const DPC = {P['deg_per_conflict']};
const IDLE = {json.dumps(gestures['idle'])};

/* 姿勢ベクトル + score/noul → CSS 変数。写像はここ1箇所だけに置く。
 *
 * 回転が主役（平行移動の振幅差は人間に読めないと判定されたため）:
 *   bob / rise が平行移動。rot_x(pitch) / rot_y(yaw) / rot_z(roll) が回転。
 *
 * lift / sink（poses.json の _recovery_rule）:
 *   静的な上下オフセット = (sink - lift)                    … 姿勢としての沈み／浮き
 *   葛藤 = 2*min(lift, sink)  … 符号付き平均が捨てる量そのもの
 *   → これを**縦振幅ではなく roll（rot_z）に加算する**。振幅の差は読めないが
 *     傾きは質的な差として読める。
 * 片方だけ非ゼロの単一ジェスチャーでは min=0 なので roll は増えない。
 * 混合のときだけ「葛藤が傾きとして見える」。 */
function varsFor(pose, energy, approach, emit) {{
  const bobS  = 0.70 + 0.15 * energy;     // 平行移動のスケール
  const rotS  = 0.80 + 0.10 * energy;     // 回転のスケール
  const freqM = 1.00 + 0.18 * energy;
  const sizeM = 0.94 + 0.035 * approach;
  const conflict = 2 * Math.min(pose.lift, pose.sink);
  return {{
    '--p-bob':   (pose.bob * PX * bobS).toFixed(2),
    '--p-rise':  ((pose.sink - pose.lift) * PX).toFixed(2),
    '--p-rotx':  (pose.rot_x * rotS).toFixed(2),
    '--p-roty':  (pose.rot_y * rotS).toFixed(2),
    '--p-rotz':  ((pose.rot_z + DPC * conflict) * rotS).toFixed(2),
    '--p-size':  (pose.size * sizeM).toFixed(3),
    '--p-freq':  (pose.freq * freqM).toFixed(3),
    '--p-flick': (pose.flick).toFixed(3),
    '--p-base':  (0.22 + 0.78 * emit).toFixed(3),
    '--p-glow':  (emit).toFixed(3),
  }};
}}

function poseText(pose) {{
  const f = (v, n) => v.toFixed(n);
  const conflict = 2 * Math.min(pose.lift, pose.sink);
  const net = pose.sink - pose.lift;
  const roll = pose.rot_z + DPC * conflict;
  return 'rot pitch ' + f(pose.rot_x, 1) + 'deg · yaw ' + f(pose.rot_y, 1) + 'deg · roll '
       + f(roll, 1) + 'deg'
       + (conflict > 0.02 ? '（うち葛藤 ' + f(DPC * conflict, 1) + 'deg）' : '')
       + '  /  bob ' + f(pose.bob, 2) + ' · 正味 ' + (net >= 0 ? '沈 ' : '浮 ') + f(Math.abs(net), 2)
       + ' · size ' + f(pose.size, 2) + ' · freq ' + f(pose.freq, 2) + ' · flick ' + f(pose.flick, 2);
}}

function apply(theta, mode) {{
  document.getElementById('thv').textContent = theta.toFixed(2);
  const fired = [];
  document.querySelectorAll('.card[data-key]').forEach(card => {{
    const body = card.querySelector('[data-role=main]');
    const emit = parseFloat(body.dataset.emit);
    const energy = parseFloat(body.dataset.energy);
    const approach = parseFloat(body.dataset.approach);
    const top = body.dataset.top;
    const on = emit >= theta;

    let pose, label;
    if (mode === 'argmax' && !on) {{
      pose = IDLE; label = 'idle';
    }} else if (mode === 'argmax') {{
      pose = JSON.parse(body.dataset.poseTop.replace(/&quot;/g, '"')); label = top;
    }} else {{
      pose = JSON.parse(body.dataset.poseMean.replace(/&quot;/g, '"'));
      label = on ? '補間' : '補間（沈黙）';
    }}
    const v = varsFor(pose, energy, approach, on || mode === 'mean' ? emit : 0);
    for (const k in v) body.style.setProperty(k, v[k]);

    card.classList.toggle('silent', mode === 'argmax' && !on);
    const name = card.querySelector('.gname');
    const note = card.querySelector('.gnote');
    if (name) name.textContent = label;
    if (note) note.innerHTML = (mode === 'argmax' && !on)
      ? ' <span>(emit ' + emit.toFixed(2) + ' &lt; ' + theta.toFixed(2) + ' → idle。モデルの選択は ' + top + ')</span>'
      : '';
    card.querySelector('.pose').textContent = poseText(pose);
    if (on) fired.push(card.dataset.key);
  }});
  document.querySelectorAll('[data-role=cmp]').forEach(b => {{
    const p = JSON.parse(b.dataset.pose.replace(/&quot;/g, '"'));
    const v = varsFor(p, parseFloat(b.dataset.energy), parseFloat(b.dataset.approach), parseFloat(b.dataset.emit));
    for (const k in v) b.style.setProperty(k, v[k]);
  }});
  document.getElementById('cnt').textContent = '発火 ' + fired.length + ' / 8';
  document.getElementById('fires').textContent = fired.join(', ');
}}
const th = document.getElementById('th'), md = document.getElementById('mode');
const upd = () => apply(parseFloat(th.value), md.value);
th.addEventListener('input', upd);
md.addEventListener('change', upd);
upd();
</script>
</body></html>"""

    out = here / args.out
    out.write_text(html)
    print(f"\n-> {out}  ({len(cards)} カード)")


if __name__ == "__main__":
    main()
