//! 「世話のされ方が、外から見える振る舞いに現れているか」(legibility)を測る。
//! docs/DESIGN.md「世話のされ方が振る舞いに現れているか」参照。
//!
//! 条件の作り方(世話され続けた状態/放置された状態)は `fitness` モジュール側に
//! あり、テストからも同じものを使っている。どちらの条件でも**一切触らない** ——
//! クリックで条件を作るとクリック自体が場を乱し、コントローラの貢献と区別が
//! つかなくなるため。
//!
//! 全生物について測る。生物ごとに「弱ると動かなくなる」(S1s)などの性質が
//! 違うため、読み取りやすさも生物ごとに違う。

use vmc_pet_body::fitness::{cared_for_trajectory, legibility, neglected_trajectory};

const FIELD_SIZE: usize = 32;
const EVAL_STEPS: u32 = 900;

fn main() {
    println!("legibility = 世話のされ方が見た目に現れている度合い(0に近い=見ても分からない)");
    println!();
    for (code, name) in vmc_pet_body::list_animals().unwrap() {
        let cared_for = cared_for_trajectory(&code, FIELD_SIZE, EVAL_STEPS);
        let neglected = neglected_trajectory(&code, FIELD_SIZE, EVAL_STEPS);
        let cared_signature = cared_for.signature(FIELD_SIZE, FIELD_SIZE);
        let weak_signature = neglected.signature(FIELD_SIZE, FIELD_SIZE);
        let score = legibility(&cared_for, &neglected, FIELD_SIZE, FIELD_SIZE);

        println!("{code:6} {name}");
        println!(
            "  元気: speed={:.4} mass_dev={:.4}",
            cared_signature.mean_speed, cared_signature.mass_deviation
        );
        println!(
            "  弱り: speed={:.4} mass_dev={:.4}",
            weak_signature.mean_speed, weak_signature.mass_deviation
        );
        println!("  legibility = {score:.4}");
        println!();
    }
}
