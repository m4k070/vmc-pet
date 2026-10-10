//! 姿勢(ジェスチャー)層の読みやすさを、**自分の描画で実測する**道具。
//!
//! `vmc-pet --probe-pose <ディレクトリ>`。ウィンドウは開かず、測った値を標準出力へ、
//! 確認用の PNG をディレクトリへ書き出して終了する。
//!
//! 測るのは3つ。どれも「動いている絵では読めない」ので、**位相を固定して**測る
//! (`docs/experiments/css-gesture.md` の「アニメを0%に凍結して測る」に相当)。
//!
//! 1. **側面(帯)が読める下限の角度** … pitch を振り、1セルぶんの帯の高さを画素で測る
//! 2. **roll(画面内の傾き)が読める下限の角度** … 体の縁が何画素動くかを測る
//! 3. **状態 → 姿勢** … 内部状態の代表点で、8ジェスチャーの重みと姿勢を出す
//!
//! さらに、本物の体(Lenia と粒子)を走らせて内部状態と姿勢の対応を確かめる。

use std::fs;
use std::path::Path;

use crate::app::Pet;
use crate::persistence::MemoryStore;
use crate::render::{DotGrid, PoseProjection};
use crate::shell::{PointerInput, Surface};
use vmc_pet_body::gesture::{
    conflict_of, dominant, resolve, weights, ExpressionInputs, GESTURES, GESTURE_COUNT,
    SLAB_THICKNESS_CELLS,
};
use vmc_pet_body::{Field, ResolvedPose, TouchEcho};

/// 確認用の画像の大きさ(論理ピクセル)。アプリと同じ。
const SURFACE: usize = 384;
/// ドットグリッドの解像度。アプリと同じ。
const GRID: usize = 32;

/// 姿勢層の入口。`main.rs` から呼ぶ。
pub fn run(directory: &Path) -> Result<(), String> {
    fs::create_dir_all(directory).map_err(|error| format!("{}: {error}", directory.display()))?;
    // 1枚ずつの生の画像は、リポジトリに置くには大きすぎる(無圧縮で 440KB/枚)。
    // `raw/` に出し、確認用の一覧画像は ImageMagick で作る(作り方は
    // docs/experiments/expression-layer.md に書いてある)。`raw/` は .gitignore 済み。
    let raw = directory.join("raw");
    fs::create_dir_all(&raw).map_err(|error| format!("{}: {error}", raw.display()))?;
    println!("surface {SURFACE}x{SURFACE}px, grid {GRID}x{GRID} ({}px/cell)",
        SURFACE / GRID);
    println!("slab thickness {} cells", SLAB_THICKNESS_CELLS);

    wall_threshold(&raw)?;
    roll_threshold(&raw)?;
    cohesion_radius()?;
    state_table();
    vocabulary_images(&raw)?;
    pinned_poses(&raw)?;
    body_states(&raw)?;
    Ok(())
}

/// 7. `--pose <名前>` で固定したときの見え方(**本物の体**で描く)。
///
/// 語彙の確認画像(節4)は丸い形で描くが、実際に見るのは Lenia の塊なので、
/// 固定した姿勢と本物の体を重ねた絵も残す。`Pet::pin_pose` と `set_pose_phase` の
/// 経路そのものの確認にもなる。
fn pinned_poses(directory: &Path) -> Result<(), String> {
    println!("\n## 7. --pose で固定したときの見え方(本物の O2u、位相 90 度)");
    for (index, gesture) in GESTURES.iter().enumerate() {
        let mut pet = Pet::with_memory_store("O2u", MemoryStore::disabled())
            .map_err(|error| format!("failed to start O2u: {error}"))?;
        pet.advance_steps(300);
        pet.pin_pose(index);
        pet.set_pose_phase(std::f32::consts::FRAC_PI_2);
        let resolved = pet.tick_expression(0.0);
        println!(
            "  {} : 駆動 {} / pitch {:.1}度 / yaw {:.1}度 / roll {:.1}度 / 上下 {:+.2} cell / 大きさ {:.2}",
            gesture.name,
            format!("{:?}", pet.pose_mode()),
            resolved.pitch_deg,
            resolved.yaw_deg,
            resolved.roll_deg,
            resolved.offset_cells.1,
            resolved.scale,
        );
        let canvas = render_pet_posed(&pet, &resolved);
        let path = directory.join(format!("pinned-{index}-{}.png", gesture.name));
        write_png(&path, &canvas)?;
    }
    Ok(())
}

/// ペットの場を、与えた姿勢で描く。
fn render_pet_posed(pet: &Pet, pose: &ResolvedPose) -> Vec<u8> {
    let empty = TouchEcho::new(GRID, GRID);
    let grid = DotGrid::new(GRID, GRID);
    let mut canvas = vec![0u8; SURFACE * SURFACE * 4];
    grid.draw_posed(
        pet.field_view(),
        empty.view(),
        None,
        (0.0, 0.0),
        &PoseProjection::from_pose(pose),
        pose.visibility,
        &mut canvas,
        SURFACE as u32,
        SURFACE as u32,
    );
    canvas
}

/// 6. 「まとまり」を測る半径を決める。
///
/// 姿勢層の入力である「まとまり」は、重心から `radius` セル以内にある総量の割合。
/// 半径の選び方で値の出方が変わるので、出荷している体で実測して決める。
/// 健康な体が 1 に近く、崩れた体がはっきり下がる半径を採る。
fn cohesion_radius() -> Result<(), String> {
    println!("\n## 6. 「まとまり」を測る半径(実測して決める)");
    let radii = [6.0f32, 8.0, 10.0, 12.0, 14.0];
    println!("| 体 | 状態 | {}", radii.iter().map(|r| format!("r={r:.0}")).collect::<Vec<_>>().join(" | "));
    println!("|---|---|{}", "---|".repeat(radii.len()));
    for code in ["O2u", "OG2g", "S1s", "2S1v", "particles:1091"] {
        let mut pet = Pet::with_memory_store(code, MemoryStore::disabled())
            .map_err(|error| format!("failed to start {code}: {error}"))?;
        pet.advance_steps(300);
        let healthy: Vec<f32> = radii.iter().map(|r| pet.field_view().cohesion(*r)).collect();
        println!("| {code} | 健康 | {} |", format_values(&healthy));
        // 体の周りを順に叩いて崩す(クリックは慣れで弱まるので、場所を変えながら)
        for round in 0..3 {
            for step in 0..10 {
                let angle = (round * 10 + step) as f32 * 0.628;
                let at = vmc_pet_body::CellPos {
                    x: (16.0 + 6.0 * angle.cos()).rem_euclid(GRID as f32) as usize,
                    y: (16.0 + 6.0 * angle.sin()).rem_euclid(GRID as f32) as usize,
                };
                pet.on_pointer(PointerInput::Pressed {
                    x: (at.x * SURFACE / GRID) as f64 + 6.0,
                    y: (at.y * SURFACE / GRID) as f64 + 6.0,
                });
                pet.advance_steps(20);
            }
        }
        let scattered: Vec<f32> = radii.iter().map(|r| pet.field_view().cohesion(*r)).collect();
        println!("| {code} | 崩した後 | {} |", format_values(&scattered));
    }
    Ok(())
}

fn format_values(values: &[f32]) -> String {
    values
        .iter()
        .map(|value| format!("{value:.2}"))
        .collect::<Vec<_>>()
        .join(" | ")
}

/// 中央の1行だけを点灯させた場。**帯(側面)の高さを測る**のに使う。
/// 横幅いっぱいにすると、上端の帯が必ず出る。
fn bar_field() -> Field {
    let mut field = Field::new(GRID, GRID);
    field.map(|_x, y, _value| if y == GRID / 2 { 1.0 } else { 0.0 });
    field
}

/// 体の形に近い丸。確認用の画像に使う(Orbium は直径およそ20セル)。
fn disc_field(radius: f32) -> Field {
    let mut field = Field::new(GRID, GRID);
    let centre = GRID as f32 / 2.0;
    field.map(|x, y, _value| {
        let dx = x as f32 + 0.5 - centre;
        let dy = y as f32 + 0.5 - centre;
        let distance = (dx * dx + dy * dy).sqrt();
        if distance < radius {
            1.0 - (distance / radius) * 0.35
        } else {
            0.0
        }
    });
    field
}

/// 姿勢を掛けて1枚描く。echo と色素は空(姿勢そのものを見るため)。
fn render(field: &Field, pose: &ResolvedPose) -> Vec<u8> {
    let echo = TouchEcho::new(GRID, GRID);
    let grid = DotGrid::new(GRID, GRID);
    let mut canvas = vec![0u8; SURFACE * SURFACE * 4];
    grid.draw_posed(
        field.view(),
        echo.view(),
        None,
        (0.0, 0.0),
        &PoseProjection::from_pose(pose),
        pose.visibility,
        &mut canvas,
        SURFACE as u32,
        SURFACE as u32,
    );
    canvas
}

/// 不透明ピクセルを含む矩形 `(min_x, min_y, max_x, max_y)`。
fn bounds(canvas: &[u8]) -> Option<(usize, usize, usize, usize)> {
    let (mut min_x, mut min_y) = (SURFACE, SURFACE);
    let (mut max_x, mut max_y) = (0usize, 0usize);
    for y in 0..SURFACE {
        for x in 0..SURFACE {
            if canvas[(y * SURFACE + x) * 4 + 3] == 0 {
                continue;
            }
            min_x = min_x.min(x);
            min_y = min_y.min(y);
            max_x = max_x.max(x);
            max_y = max_y.max(y);
        }
    }
    (min_x <= max_x).then_some((min_x, min_y, max_x, max_y))
}

/// 1. 側面(帯)が読める下限。
///
/// 1セルぶんの帯の高さは「厚み × sin(pitch)」。これが**ドットの直径**を超えると、
/// 縁が厚みのある板として読める。厚みを無視した帯の予測と、描画から測った値を並べる。
fn wall_threshold(directory: &Path) -> Result<(), String> {
    println!("\n## 1. 側面(帯)が読める下限 — 横一列の板を pitch で傾ける");
    println!("位相は 0 度に固定(振れの無い姿勢で測る)");
    let field = bar_field();
    let flat = render(&field, &ResolvedPose::IDENTITY);
    let (_, flat_top, _, flat_bottom) = bounds(&flat).ok_or("the bar must be drawn")?;
    let dot_diameter = (flat_bottom - flat_top + 1) as f32;
    println!("ドットの直径(姿勢なし) = {dot_diameter}px");
    println!();
    println!("| pitch | 帯の予測 (厚み×sin) | 描画の高さ | 帯の実測 | 帯/ドット径 |");
    println!("|---|---|---|---|---|");
    let mut first_readable = None;
    for pitch in [0.0f32, 5.0, 10.0, 15.0, 20.0, 25.0, 30.0, 35.0, 40.0, 45.0] {
        let pose = ResolvedPose {
            pitch_deg: pitch,
            ..ResolvedPose::IDENTITY
        };
        let canvas = render(&field, &pose);
        let (_, top, _, bottom) = bounds(&canvas).ok_or("the bar must be drawn")?;
        let height = (bottom - top + 1) as f32;
        let band = height - dot_diameter;
        let predicted =
            SLAB_THICKNESS_CELLS * pitch.to_radians().sin() * (SURFACE / GRID) as f32;
        let ratio = band / dot_diameter;
        println!(
            "| {pitch:.0}度 | {predicted:.1}px | {height:.0}px | {band:.0}px | {ratio:.2} |"
        );
        if first_readable.is_none() && ratio >= 1.0 {
            first_readable = Some(pitch);
        }
        let path = directory.join(format!("pitch-{:02}.png", pitch as u32));
        write_png(&path, &canvas)?;
    }
    match first_readable {
        Some(pitch) => println!(
            "\n**帯がドット1個ぶん以上になる最初の角度 = {pitch:.0}度**(描画から測った値)"
        ),
        None => println!("\n帯がドット1個ぶんに届く角度は 45度以下に無い"),
    }
    Ok(())
}

/// 2. roll(画面内の傾き)が読める下限。
///
/// roll は板を横切らないので帯は出ない。読める根拠は「体の縁が画面内で持ち上がる」
/// ことで、その移動量 = 縁までの距離 × sin(roll)。**ドットの半径**ぶん動けば、
/// 円が円でなくなる(= 傾きとして見える)。
///
/// 描画の検算も兼ねる: 横一列の両端の高低差から、実際に何度傾いて描かれたかを出す。
fn roll_threshold(directory: &Path) -> Result<(), String> {
    println!("\n## 2. roll(画面内の傾き)が読める下限");
    let field = bar_field();
    let px_per_cell = (SURFACE / GRID) as f32;
    let dot_radius = px_per_cell * 0.5 * 0.82;
    let rim_cells = 10.0f32;
    println!(
        "体の縁までの距離 {rim_cells}セル = {}px、ドットの半径 {dot_radius:.1}px",
        rim_cells * px_per_cell
    );
    println!();
    println!("| roll | 描いた列の高低差 | 描かれた角度 | 縁の移動 (半径×sin) | ドット半径との比 |");
    println!("|---|---|---|---|---|");
    let mut first_readable = None;
    for roll in [0.0f32, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 8.0, 10.0] {
        let pose = ResolvedPose {
            roll_deg: roll,
            ..ResolvedPose::IDENTITY
        };
        let canvas = render(&field, &pose);
        let (min_x, _, max_x, _) = bounds(&canvas).ok_or("the bar must be drawn")?;
        let left = column_centre_y(&canvas, min_x).unwrap_or(0.0);
        let right = column_centre_y(&canvas, max_x).unwrap_or(0.0);
        let difference = right - left;
        let drawn_angle = (difference / (max_x - min_x) as f32).atan().to_degrees();
        let moved = rim_cells * roll.to_radians().sin() * px_per_cell;
        let ratio = moved / dot_radius;
        println!(
            "| {roll:.0}度 | {difference:.1}px | {drawn_angle:.1}度 | {moved:.1}px | {ratio:.2} |"
        );
        if first_readable.is_none() && ratio >= 1.0 {
            first_readable = Some(roll);
        }
        let path = directory.join(format!("roll-{:02}.png", roll as u32));
        write_png(&path, &canvas)?;
    }
    match first_readable {
        Some(roll) => println!(
            "\n**縁がドット半径ぶん動く最初の角度 = {roll:.0}度**(描画から測った値。spike の「数度」と同じ桁)"
        ),
        None => println!("\n10度以下ではドット半径ぶん動かない"),
    }
    Ok(())
}

/// その列で描かれているピクセルの重心の y。
fn column_centre_y(canvas: &[u8], x: usize) -> Option<f32> {
    let (mut total, mut weight) = (0.0f32, 0.0f32);
    for y in 0..SURFACE {
        let alpha = canvas[(y * SURFACE + x) * 4 + 3] as f32;
        if alpha == 0.0 {
            continue;
        }
        total += y as f32 * alpha;
        weight += alpha;
    }
    (weight > 0.0).then(|| total / weight)
}

/// 3. 状態 → 8ジェスチャーの重みと姿勢。位相 90 度(振れの山)に固定して出す。
fn state_table() {
    println!("\n## 3. 状態 → 姿勢(位相 90 度に固定)");
    let states: [(&str, ExpressionInputs); 8] = [
        (
            "元気・まとまり・いま触られた",
            ExpressionInputs { energy: 1.0, cohesion: 1.0, tempo: 1.0, unattended_seconds: 0.0 },
        ),
        (
            "元気・まとまり・しばらく放置",
            ExpressionInputs { energy: 1.0, cohesion: 1.0, tempo: 1.0, unattended_seconds: 200.0 },
        ),
        (
            "元気・ばらけた・いま触られた",
            ExpressionInputs { energy: 1.0, cohesion: 0.3, tempo: 1.0, unattended_seconds: 0.0 },
        ),
        (
            "放置で弱った・まとまり",
            ExpressionInputs { energy: 0.0, cohesion: 1.0, tempo: 1.0, unattended_seconds: 600.0 },
        ),
        (
            "弱って、がっかりもしている",
            ExpressionInputs { energy: 0.0, cohesion: 1.0, tempo: 0.6, unattended_seconds: 600.0 },
        ),
        (
            "元気が中間・ばらけた(迷い)",
            ExpressionInputs { energy: 0.5, cohesion: 0.2, tempo: 1.0, unattended_seconds: 10.0 },
        ),
        (
            "元気・よくまとまった・放置(基準)",
            ExpressionInputs { energy: 0.9, cohesion: 0.95, tempo: 1.0, unattended_seconds: 120.0 },
        ),
        (
            "弱ってばらけた",
            ExpressionInputs { energy: 0.2, cohesion: 0.4, tempo: 0.8, unattended_seconds: 300.0 },
        ),
    ];
    println!("| 状態 | 主役 | 上位3 | 葛藤 | pitch | yaw | roll | 上下(cell) | 大きさ |");
    println!("|---|---|---|---|---|---|---|---|---|");
    for (name, inputs) in states {
        let weights = weights(&inputs);
        let pose = vmc_pet_body::Pose::mix(&GESTURES, &weights);
        let conflict = conflict_of(&pose);
        let resolved = resolve(&pose, conflict, std::f32::consts::FRAC_PI_2);
        let top = dominant(&weights);
        let mut order: Vec<usize> = (0..GESTURE_COUNT).collect();
        order.sort_by(|a, b| weights[*b].partial_cmp(&weights[*a]).unwrap());
        let top3 = order[0..3]
            .iter()
            .map(|index| format!("{} {:.0}%", GESTURES[*index].name, weights[*index] * 100.0))
            .collect::<Vec<_>>()
            .join(" / ");
        println!(
            "| {name} | {} | {top3} | {conflict:.2} | {:.1}度 | {:.1}度 | {:.1}度 | {:+.2} | {:.2} |",
            GESTURES[top].name,
            resolved.pitch_deg,
            resolved.yaw_deg,
            resolved.roll_deg,
            resolved.offset_cells.1,
            resolved.scale,
        );
    }
}

/// 4. 語彙の8ジェスチャーを、体の形で描いて書き出す。位相 90 度(振れの山)。
fn vocabulary_images(directory: &Path) -> Result<(), String> {
    println!("\n## 4. 語彙の確認用画像(位相 90 度、丸い体)");
    let field = disc_field(10.0);
    for (index, gesture) in GESTURES.iter().enumerate() {
        let pose = resolve(&gesture.pose, conflict_of(&gesture.pose), std::f32::consts::FRAC_PI_2);
        let canvas = render(&field, &pose);
        let path = directory.join(format!("gesture-{}-{}.png", index, gesture.name));
        write_png(&path, &canvas)?;
        println!(
            "  {} : pitch {:.1}度, yaw {:.1}度, roll {:.1}度, 上下 {:+.2} cell, 大きさ {:.2}, 明滅 {:.2}",
            gesture.name,
            pose.pitch_deg,
            pose.yaw_deg,
            pose.roll_deg,
            pose.offset_cells.1,
            pose.scale,
            pose.visibility,
        );
    }
    Ok(())
}

/// 5. 本物の体を走らせて、内部状態 → 姿勢の対応を確かめる。
fn body_states(directory: &Path) -> Result<(), String> {
    println!("\n## 5. 本物の体を走らせたときの内部状態と姿勢");
    for code in ["O2u", "OG2g", "S1s", "particles:1091"] {
        let mut pet = Pet::with_memory_store(code, MemoryStore::disabled())
            .map_err(|error| format!("failed to start {code}: {error}"))?;
        println!("\n### {code}");
        println!("| ステップ | 秒 | energy | まとまり | テンポ | 放置(秒) | 主役 | pitch | yaw | roll | 上下(cell) |");
        println!("|---|---|---|---|---|---|---|---|---|---|---|");
        let steps = if code.starts_with("particles") { 1200 } else { 3000 };
        for step in 0..=steps {
            let touched = step % 900 == 0;
            if touched {
                pet.on_pointer(PointerInput::Pressed { x: 192.0, y: 192.0 });
            }
            pet.advance_steps(1);
            pet.tick_expression(1.0 / 15.0);
            if step % 450 != 0 {
                continue;
            }
            let (energy, cohesion, tempo, unattended) = pet.expression_inputs();
            let expression = pet.expression();
            let pose = expression.pose();
            let conflict = expression.conflict();
            let resolved_peak = resolve(&pose, conflict, std::f32::consts::FRAC_PI_2);
            let top = dominant(&expression.weights());
            println!(
                "| {step} | {:.0} | {energy:.2} | {cohesion:.2} | {tempo:.2} | {unattended:.0} | {} | {:.1}度 | {:.1}度 | {:.1}度 | {:+.2} |",
                step as f32 / 15.0,
                GESTURES[top].name,
                resolved_peak.pitch_deg,
                resolved_peak.yaw_deg,
                resolved_peak.roll_deg,
                resolved_peak.offset_cells.1,
            );
        }
        // 体の形そのものの確認用画像(姿勢なし)
        let canvas = render_from_pet(&pet);
        let path = directory.join(format!("body-{}.png", code.replace(':', "-")));
        write_png(&path, &canvas)?;
        let (_, cohesive, tempo, _) = pet.expression_inputs();
        println!("  まとまり {cohesive:.2} / テンポ {tempo:.2} のときの体(姿勢なし)を書き出した");
    }
    Ok(())
}

/// ペットの場を、姿勢を掛けずに描く。
fn render_from_pet(pet: &Pet) -> Vec<u8> {
    let empty = TouchEcho::new(GRID, GRID);
    let grid = DotGrid::new(GRID, GRID);
    let mut canvas = vec![0u8; SURFACE * SURFACE * 4];
    grid.draw_with_pigment(
        pet.field_view(),
        empty.view(),
        None,
        (0.0, 0.0),
        &mut canvas,
        SURFACE as u32,
        SURFACE as u32,
    );
    canvas
}

/// premultiplied ARGB8888 を、黒背景の RGB として PNG に書く。
///
/// 不透明度を落とさずに済むよう、アルファを掛けた値(= 黒の上に置いた色)をそのまま使う。
/// 画像を見るのは目視確認のためだけで、色そのものを測る用途には使わない。
fn write_png(path: &Path, canvas: &[u8]) -> Result<(), String> {
    let mut raw = Vec::with_capacity(SURFACE * (1 + SURFACE * 3));
    for y in 0..SURFACE {
        raw.push(0u8); // フィルタ種別: なし
        for x in 0..SURFACE {
            let pixel = (y * SURFACE + x) * 4;
            raw.extend_from_slice(&[canvas[pixel + 2], canvas[pixel + 1], canvas[pixel]]);
        }
    }
    let mut png = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    let mut header = Vec::new();
    header.extend_from_slice(&(SURFACE as u32).to_be_bytes());
    header.extend_from_slice(&(SURFACE as u32).to_be_bytes());
    header.extend_from_slice(&[8, 2, 0, 0, 0]); // 8bit・RGB・無圧縮・無フィルタ・非インタレース
    push_chunk(&mut png, b"IHDR", &header);
    push_chunk(&mut png, b"IDAT", &zlib_stored(&raw));
    push_chunk(&mut png, b"IEND", &[]);
    fs::write(path, png).map_err(|error| format!("{}: {error}", path.display()))
}

fn push_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let mut crc_input = Vec::with_capacity(4 + data.len());
    crc_input.extend_from_slice(kind);
    crc_input.extend_from_slice(data);
    out.extend_from_slice(&crc32(&crc_input).to_be_bytes());
}

/// zlib の「無圧縮(defleat stored)」ストリーム。依存を増やさないための最小実装。
fn zlib_stored(data: &[u8]) -> Vec<u8> {
    let mut out = vec![0x78, 0x01];
    for (index, chunk) in data.chunks(65_535).enumerate() {
        let last = (index + 1) * 65_535 >= data.len();
        out.push(if last { 1 } else { 0 });
        let length = chunk.len() as u16;
        out.extend_from_slice(&length.to_le_bytes());
        out.extend_from_slice(&(!length).to_le_bytes());
        out.extend_from_slice(chunk);
    }
    if data.is_empty() {
        out.extend_from_slice(&[1, 0, 0, 0xff, 0xff]);
    }
    out.extend_from_slice(&adler32(data).to_be_bytes());
    out
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for byte in data {
        crc ^= *byte as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xedb8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for byte in data {
        a = (a + *byte as u32) % 65_521;
        b = (b + a) % 65_521;
    }
    (b << 16) | a
}
